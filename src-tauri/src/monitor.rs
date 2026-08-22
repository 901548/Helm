// ============================================================================
// monitor.rs - Linux 系统状态监控
// 周期性对活跃 SSH 会话执行 /proc 采集命令，解析 CPU/内存/负载/网速，
// 经 "sysmon" 事件推送到前端显示。
//
// 采集走独立 exec 通道（不经过交互 shell），因此不会污染终端输出。
// ============================================================================

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::ssh::{run_exec, SshManager, SessionStatus};

// ---------- 事件负载 ----------

/// 系统监控事件负载（camelCase 下发前端）
#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SysMonPayload {
    /// 会话名
    pub name: String,
    /// CPU 使用率 0-100
    pub cpu: f64,
    /// 内存总量(MB)
    pub mem_total_mb: u64,
    /// 内存已用(MB)
    pub mem_used_mb: u64,
    /// 负载 1/5/15 分钟
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    /// 网络接收速率(bytes/s)
    pub rx_bps: u64,
    /// 网络发送速率(bytes/s)
    pub tx_bps: u64,
    /// 采集是否成功
    pub ok: bool,
    /// 失败原因
    pub error: Option<String>,
}

// ---------- 采样状态 ----------

/// 单会话上一次采样值（供 CPU/网速差值计算）
#[derive(Clone)]
struct SysPrev {
    cpu_total: u64,
    cpu_idle: u64,
    rx: u64,
    tx: u64,
    last_time: Instant,
}

impl Default for SysPrev {
    fn default() -> Self {
        Self {
            cpu_total: 0,
            cpu_idle: 0,
            rx: 0,
            tx: 0,
            last_time: Instant::now(),
        }
    }
}

// ---------- 采集命令 ----------

/// 分隔符（保证不与 /proc 输出冲突）
const SEP1: &str = "===HELM1===";
const SEP2: &str = "===HELM2===";
const SEP3: &str = "===HELM3===";
const SEP4: &str = "===HELM4===";

/// 单次采集命令：一次 exec 取回四段 /proc 数据
pub fn collect_command() -> &'static str {
    "echo ===HELM1===; cat /proc/stat; echo ===HELM2===; cat /proc/meminfo; \
     echo ===HELM3===; cat /proc/loadavg; echo ===HELM4===; cat /proc/net/dev"
}

/// Windows 采集命令（P34）：经 PowerShell 一次性取回 CPU/内存/网速。
/// CPU/网速为瞬时速率（无差值），内存为 Total/Free(KB)。负载无对应项恒 0。
/// 输出仍按 SEP1-4 分段，便于复用 section() 提取。
pub fn collect_command_windows() -> &'static str {
    "powershell -NoProfile -NonInteractive -Command $os=Get-CimInstance Win32_OperatingSystem; $cpu=(Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor | Where-Object { $_.Name -eq '_Total' } | Select-Object -First 1).PercentProcessorTime; $nics=Get-CimInstance Win32_PerfFormattedData_Tcpip_NetworkInterface; $rx=($nics | Measure-Object BytesReceivedPersec -Sum).Sum; $tx=($nics | Measure-Object BytesSentPersec -Sum).Sum; Write-Output '===HELM1==='; Write-Output $cpu; Write-Output '===HELM2==='; Write-Output $os.TotalVisibleMemorySize; Write-Output $os.FreePhysicalMemory; Write-Output '===HELM3==='; Write-Output '0 0 0'; Write-Output '===HELM4==='; Write-Output $rx; Write-Output $tx"
}

// ---------- 解析(纯函数，可单测) ----------

/// 从采集输出中提取第 n 个分隔符之后的段落
fn section<'a>(text: &'a str, sep: &str, next: Option<&str>) -> &'a str {
    let Some(start) = text.find(sep) else { return "" };
    let body = &text[start + sep.len()..];
    match next {
        Some(n) => {
            let end = body.find(n).unwrap_or(body.len());
            &body[..end]
        }
        None => body,
    }
}

/// 解析 CPU 行："cpu  user nice system idle iowait irq softirq steal ..."
/// 返回 (total, idle)
fn parse_cpu_line(line: &str) -> Option<(u64, u64)> {
    let parts: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    if parts.len() < 8 {
        return None;
    }
    let idle = parts[3] + parts[4];
    let total: u64 = parts.iter().sum();
    Some((total, idle))
}

/// 解析 /proc/stat 的 cpu 聚合行
fn parse_stat(text: &str) -> Option<(u64, u64)> {
    text.lines().find(|l| l.starts_with("cpu ")).and_then(parse_cpu_line)
}

/// 从 /proc/meminfo 提取指定键的 kB 值
fn meminfo_kb(text: &str, key: &str) -> Option<u64> {
    text.lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
}

/// 解析 /proc/meminfo → (total_kb, available_kb)
/// 优先 MemAvailable，缺失时回退 MemFree+Buffers+Cached
fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let total = meminfo_kb(text, "MemTotal:")?;
    let avail = meminfo_kb(text, "MemAvailable:")
        .or_else(|| {
            let free = meminfo_kb(text, "MemFree:")?;
            let buffers = meminfo_kb(text, "Buffers:")?;
            let cached = meminfo_kb(text, "Cached:")?;
            Some(free + buffers + cached)
        })?;
    Some((total, avail))
}

/// 解析 /proc/loadavg → (load1, load5, load15)
fn parse_loadavg(text: &str) -> Option<(f64, f64, f64)> {
    let mut iter = text.split_whitespace();
    let l1 = iter.next()?.parse().ok()?;
    let l5 = iter.next()?.parse().ok()?;
    let l15 = iter.next()?.parse().ok()?;
    Some((l1, l5, l15))
}

/// 解析 /proc/net/dev → (rx_bytes, tx_bytes)，跳过 lo 与表头
fn parse_netdev(text: &str) -> (u64, u64) {
    let mut rx = 0u64;
    let mut tx = 0u64;
    for line in text.lines().skip(2) {
        let Some((iface, rest)) = line.split_once(':') else { continue };
        if iface.trim() == "lo" {
            continue;
        }
        let fields: Vec<u64> = rest
            .split_whitespace()
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        if fields.len() > 8 {
            rx += fields[0];
            tx += fields[8];
        }
    }
    (rx, tx)
}

/// 解析一次完整采集输出 → 中间数据（不含差值计算）
struct SysSample {
    cpu_total: u64,
    cpu_idle: u64,
    mem_total_kb: u64,
    mem_avail_kb: u64,
    load1: f64,
    load5: f64,
    load15: f64,
    rx: u64,
    tx: u64,
}

fn parse_sample(text: &str) -> Option<SysSample> {
    let stat = section(text, SEP1, Some(SEP2));
    let mem = section(text, SEP2, Some(SEP3));
    let load = section(text, SEP3, Some(SEP4));
    let net = section(text, SEP4, None);

    let (cpu_total, cpu_idle) = parse_stat(stat)?;
    let (mem_total_kb, mem_avail_kb) = parse_meminfo(mem)?;
    let (load1, load5, load15) = parse_loadavg(load)?;
    let (rx, tx) = parse_netdev(net);

    Some(SysSample {
        cpu_total,
        cpu_idle,
        mem_total_kb,
        mem_avail_kb,
        load1,
        load5,
        load15,
        rx,
        tx,
    })
}

/// Windows 采样（P34）：CPU/网速为瞬时速率，无差值语义
#[derive(Debug, PartialEq)]
struct WinSample {
    cpu: f64,
    mem_total_kb: u64,
    mem_free_kb: u64,
    rx_bps: u64,
    tx_bps: u64,
}

/// 解析 Windows 采集输出（SEP1=cpu%, SEP2=mem KB, SEP3=0 0 0, SEP4=rx tx B/s）
fn parse_win_sample(text: &str) -> Option<WinSample> {
    let cpu = section(text, SEP1, Some(SEP2)).trim().parse().ok()?;
    let mut mem = section(text, SEP2, Some(SEP3)).split_whitespace();
    let mem_total_kb = mem.next()?.parse().ok()?;
    let mem_free_kb = mem.next()?.parse().ok()?;
    let mut net = section(text, SEP4, None).split_whitespace();
    let rx_bps = net.next()?.parse().ok()?;
    let tx_bps = net.next()?.parse().ok()?;
    Some(WinSample {
        cpu,
        mem_total_kb,
        mem_free_kb,
        rx_bps,
        tx_bps,
    })
}

/// 由 Windows 采样直接组装负载（无差值，CPU=瞬时%，内存 = 总量-空闲，网速=瞬时 B/s）
fn build_win_payload(name: &str, s: &WinSample) -> SysMonPayload {
    SysMonPayload {
        name: name.to_string(),
        cpu: s.cpu.clamp(0.0, 100.0),
        mem_total_mb: (s.mem_total_kb / 1024) as u64,
        mem_used_mb: (s.mem_total_kb.saturating_sub(s.mem_free_kb) / 1024) as u64,
        load1: 0.0,
        load5: 0.0,
        load15: 0.0,
        rx_bps: s.rx_bps,
        tx_bps: s.tx_bps,
        ok: true,
        error: None,
    }
}

// ---------- 差值计算与负载组装 ----------

/// 由本次采样 + 上次采样计算最终负载
fn build_payload(
    name: &str,
    sample: &SysSample,
    prev: &SysPrev,
    elapsed: Duration,
) -> SysMonPayload {
    let dt = elapsed.as_secs_f64().max(0.001);
    let d_total = sample.cpu_total.saturating_sub(prev.cpu_total);
    let d_idle = sample.cpu_idle.saturating_sub(prev.cpu_idle);
    let cpu = if d_total > 0 {
        ((1.0 - d_idle as f64 / d_total as f64) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let rx_bps = (sample.rx.saturating_sub(prev.rx)) as f64 / dt;
    let tx_bps = (sample.tx.saturating_sub(prev.tx)) as f64 / dt;

    let used_kb = sample.mem_total_kb.saturating_sub(sample.mem_avail_kb);
    SysMonPayload {
        name: name.to_string(),
        cpu,
        mem_total_mb: (sample.mem_total_kb / 1024) as u64,
        mem_used_mb: (used_kb / 1024) as u64,
        load1: sample.load1,
        load5: sample.load5,
        load15: sample.load15,
        rx_bps: rx_bps as u64,
        tx_bps: tx_bps as u64,
        ok: true,
        error: None,
    }
}

// ---------- 后台任务 ----------

/// 周期采集活跃会话系统状态并 emit "sysmon" 事件。
///
/// 只在锁外执行远程命令：先短暂锁内取 exec_handle 的 Arc，再在锁外用 run_exec
/// 执行，避免长命令持锁阻塞其它 SSH 操作。
pub fn spawn_sysmon_poller(app: AppHandle, ssh: Arc<SshManager>) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        let mut prev: HashMap<String, SysPrev> = HashMap::new();
        loop {
            interval.tick().await;
            let name = ssh.active_name().await;
            let Some(name) = name else { continue };
            let is_connected = ssh.get_status(&name).await == SessionStatus::Connected;
            if !is_connected {
                continue;
            }
            let handle = ssh.exec_handle(&name).await;
            let Some(handle) = handle else { continue };
            let kind = ssh.kind_of(&name).await;

            let result = if kind == crate::config::SessionKind::Windows {
                tokio::time::timeout(
                    Duration::from_secs(5),
                    run_exec(handle, collect_command_windows()),
                )
                .await
            } else {
                tokio::time::timeout(
                    Duration::from_secs(5),
                    run_exec(handle, collect_command()),
                )
                .await
            };

            match result {
                Ok(Ok(text)) => {
                    let parsed: Option<SysMonPayload> = if kind == crate::config::SessionKind::Windows {
                        parse_win_sample(&text).map(|s| build_win_payload(&name, &s))
                    } else {
                        parse_sample(&text).map(|s| {
                            let now = Instant::now();
                            let p = prev.entry(name.clone()).or_insert_with(|| SysPrev {
                                last_time: now,
                                ..SysPrev::default()
                            });
                            let elapsed = now.duration_since(p.last_time);
                            let payload = build_payload(&name, &s, p, elapsed);
                            p.cpu_total = s.cpu_total;
                            p.cpu_idle = s.cpu_idle;
                            p.rx = s.rx;
                            p.tx = s.tx;
                            p.last_time = now;
                            payload
                        })
                    };
                    match parsed {
                        Some(payload) => {
                            let _ = app.emit("sysmon", payload);
                        }
                        None => {
                            let _ = app.emit(
                                "sysmon",
                                SysMonPayload {
                                    name: name.clone(),
                                    ok: false,
                                    error: Some("无法解析系统数据".to_string()),
                                    ..SysMonPayload::default()
                                },
                            );
                            prev.remove(&name);
                        }
                    }
                }
                Ok(Err(e)) => {
                    let _ = app.emit(
                        "sysmon",
                        SysMonPayload {
                            name: name.clone(),
                            ok: false,
                            error: Some(format!("采集失败: {}", e)),
                            ..SysMonPayload::default()
                        },
                    );
                    prev.remove(&name);
                }
                Err(_) => {
                    let _ = app.emit(
                        "sysmon",
                        SysMonPayload {
                            name: name.clone(),
                            ok: false,
                            error: Some("采集超时".to_string()),
                            ..SysMonPayload::default()
                        },
                    );
                    prev.remove(&name);
                }
            }
        }
    });
}

// ---------- 测试 ----------

#[cfg(test)]
mod tests {
    use super::*;

    const PROC_TEXT: &str = r#"===HELM1===
cpu  100 0 200 400 50 0 0 0 0 0
cpu0 25 0 50 100 12 0 0 0 0 0
===HELM2===
MemTotal:       16384000 kB
MemFree:         1024000 kB
MemAvailable:    8192000 kB
Buffers:          512000 kB
Cached:          4096000 kB
===HELM3===
0.50 0.35 0.20 1/234 5678
===HELM4===
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:  100000  1000    0    0    0     0          0         0   100000  1000    0    0    0     0          0         0
  eth0: 8388608  8192    0    0    0     0          0         0  1048576  1024    0    0    0     0          0         0
"#;

    #[test]
    fn parse_sections() {
        let sample = parse_sample(PROC_TEXT).expect("解析失败");
        assert_eq!(sample.cpu_total, 750); // 100+0+200+400+50
        assert_eq!(sample.cpu_idle, 450); // 400+50
        assert_eq!(sample.mem_total_kb, 16384000);
        assert_eq!(sample.mem_avail_kb, 8192000);
        assert_eq!(sample.load1, 0.5);
        assert_eq!(sample.load5, 0.35);
        assert_eq!(sample.load15, 0.20);
        assert_eq!(sample.rx, 8388608);
        assert_eq!(sample.tx, 1048576);
    }

    #[test]
    fn build_payload_uses_delta() {
        let s1 = parse_sample(PROC_TEXT).unwrap();
        let prev = SysPrev {
            cpu_total: 500,
            cpu_idle: 400,
            rx: 0,
            tx: 0,
            last_time: Instant::now(),
        };
        let payload = build_payload("test", &s1, &prev, Duration::from_secs(2));
        assert!(payload.ok);
        // d_total=250, d_idle=50 → cpu = (1-50/250)*100 = 80
        assert!((payload.cpu - 80.0).abs() < 0.001);
        // mem used = 16384000-8192000 = 8192000 kB = 8000 MB
        assert_eq!(payload.mem_total_mb, 16000);
        assert_eq!(payload.mem_used_mb, 8000);
        // rx 8388608 bytes / 2s = 4194304 B/s
        assert_eq!(payload.rx_bps, 4194304);
        assert_eq!(payload.tx_bps, 524288);
    }

    #[test]
    fn parse_skips_lo_interface() {
        let text = "Inter-| Receive\n face |bytes\n  lo: 500 0 0 0 0 0 0 0 500 0 0 0 0 0 0 0\n eth0: 800 0 0 0 0 0 0 0 300 0 0 0 0 0 0 0\n eth1: 200 0 0 0 0 0 0 0 100 0 0 0 0 0 0 0\n";
        let (rx, tx) = parse_netdev(text);
        assert_eq!(rx, 1000);
        assert_eq!(tx, 400);
    }

    #[test]
    fn meminfo_fallback_without_available() {
        let text = "MemTotal:       1000000 kB\nMemFree:          200000 kB\nBuffers:           50000 kB\nCached:           150000 kB\n";
        let (total, avail) = parse_meminfo(text).unwrap();
        assert_eq!(total, 1000000);
        assert_eq!(avail, 400000); // 200000+50000+150000
    }

    #[test]
    fn bad_input_yields_none() {
        assert!(parse_sample("garbage").is_none());
        assert!(parse_cpu_line("cpu").is_none());
        assert!(parse_meminfo("no data here").is_none());
        assert!(parse_loadavg("").is_none());
    }

    #[test]
    fn parse_windows_sample() {
        let text = "===HELM1===\n23\n===HELM2===\n16777216\n8388608\n===HELM3===\n0 0 0\n===HELM4===\n12345\n6789\n";
        let s = parse_win_sample(text).expect("解析失败");
        assert_eq!(s, WinSample {
            cpu: 23.0,
            mem_total_kb: 16777216,
            mem_free_kb: 8388608,
            rx_bps: 12345,
            tx_bps: 6789,
        });
        let p = build_win_payload("win", &s);
        assert!(p.ok);
        assert_eq!(p.cpu, 23.0);
        assert_eq!(p.mem_total_mb, 16384);
        assert_eq!(p.mem_used_mb, 8192);
        assert_eq!(p.rx_bps, 12345);
        assert_eq!(p.tx_bps, 6789);
        assert_eq!(p.load1, 0.0);
    }
}
