// ============================================================================
// snapshot.rs - 服务器配置快照（P127 一期：手动快照）
//
// 命题：**让不可逆命令变可逆**。服务器拍不了虚拟机快照（缺块层抽象，见
// docs/P127-服务器快照-设计方案.md §1），但破坏是**文件级的、有界的**，
// 故文件级 tar 打包足以覆盖"手滑改错配置 / 误删"这一类事故。
//
// 一期范围（本文件）：
//   - 探测：df 可用空间 + du 各候选路径体积（带超时，超时即判"过大"）
//   - 打包：tar 全量 + 排除敏感文件 + 打包后校验（残包必删）
//   - 列表/删除：保留策略
// **不含**：命令前拦截、滚动基线、还原（二~五期，见设计稿 §11）
//
// 实测依据（2026-10-13，CentOS 7.9 测试机）：
//   - LVM 快照不可用（vg_free=0，且 XFS 不能缩容腾空间）
//   - reflink 不可用（CentOS 7 的 XFS 默认 reflink=0）
//   - tar 是唯一可行机制；/etc 仅 36M/1732 文件，打包 <1 秒
//   - /tmp 会被 systemd-tmpfiles 10 天清理 → **落点不能用 /tmp**
// ============================================================================

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::ssh::SshManager;
use crate::task_exec::run_task_exec;

/// 默认落点（相对 $HOME）。**不能用 /tmp** —— systemd-tmpfiles 默认 10 天清理，
/// 快照会静默消失（用户以为有退路其实没有，比没有快照更危险）。
pub const DEFAULT_DIR: &str = ".helm-snapshots";

// ---------- P127 二期：命令前拦截开关 ----------
//
// 用全局原子而非 CoreState 字段，理由：`send_input` 是**热路径**（每次按键都调），
// 从 config 锁里读会引入不必要的锁竞争；且该开关是进程级策略，无会话维度。
//
// **默认关闭**（与设计稿 §9 一致）：这是会写服务器磁盘、且会拦用户输入的功能，
// 必须显式开启。
static INTERCEPT_ON: AtomicBool = AtomicBool::new(false);

/// 是否启用命令前拦截
pub fn intercept_enabled() -> bool {
    INTERCEPT_ON.load(Ordering::Relaxed)
}

/// 设置命令前拦截开关
pub fn set_intercept_enabled(on: bool) {
    INTERCEPT_ON.store(on, Ordering::Relaxed);
}

/// 敏感文件排除清单（设计稿 §3.3）。
///
/// **关键洞察**：最危险的那几个文件，恰恰是**不想还原**的 ——
/// 还原 `ssh_host_*_key` 会换掉主机密钥（所有客户端报 host key changed），
/// 还原 `shadow` 会回滚密码哈希。而 `/etc/nginx/*.conf` 这类**配置**才是要还原的。
///
/// 故"打包时就排除"这一个动作同时提升安全性与可用性：
/// 包内无密钥 → 无需加密 → 还原时也不会误换主机密钥。
const EXCLUDES: &[&str] = &[
    "etc/shadow",
    "etc/shadow-",
    "etc/gshadow",
    "etc/gshadow-",
    "etc/ssh/ssh_host_*",
    "root/.ssh/id_*",
    "home/*/.ssh/id_*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "etc/ssl/private/*",
    "etc/pki/private/*",
];

/// 探测候选路径（设计稿 §6.4：配置密集区，跳过 /proc /sys /dev /var/lib 等）
const CANDIDATE_PATHS: &[&str] = &["/etc", "/root", "/home", "/usr/local", "/opt"];

/// 探测超时（秒）。大机器上 du 扫 /home 可能几十秒 → 超时即判"过大"不纳入。
const PROBE_TIMEOUT_SECS: u64 = 8;
/// 打包超时（秒）。实测 /etc（1732 文件）<1 秒；20000 小文件需 9.5 秒。
const PACK_TIMEOUT_SECS: u64 = 300;
/// 单次快照体积上限（MB）。超过则拒绝 —— 见设计稿 §7.1。
const MAX_SIZE_MB: u64 = 500;
/// 安全系数：可用空间须 > 预估体积 × 此值，否则拒绝打包（设计稿 §7.1）
const SPACE_SAFETY_FACTOR: f64 = 3.0;
/// 保留份数
pub const KEEP: usize = 10;

// ---------- 数据结构 ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotEntry {
    /// 文件名（不含目录）
    pub file: String,
    /// 完整路径
    pub path: String,
    /// 打包时间（Unix 秒）
    pub created_at: u64,
    /// 体积（字节）
    pub size: u64,
    /// 已校验通过（tar -t 成功）。**残包不会出现在列表里**。
    pub verified: bool,
    /// 打包的路径列表
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    /// 落点所在分区可用空间（MB）
    pub disk_avail_mb: u64,
    /// 各候选路径体积（MB）；超时的路径不在列表里（视为过大）
    pub path_sizes: Vec<(String, u64)>,
    /// 推荐打包的路径（由体积 + 可用空间推出）
    pub recommended: Vec<String>,
    /// 推荐理由（给用户看的结论，不是选择题）
    pub reason: String,
    /// 是否检测到 LVM 剩余空间（可提示"支持块级快照"）
    pub lvm_free_mb: u64,
    /// 是否支持 reflink
    pub reflink: bool,
    /// 机器规模（内存 MB）
    pub mem_total_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackResult {
    pub file: String,
    pub path: String,
    pub size: u64,
    pub verified: bool,
    /// 打包耗时（毫秒）
    pub elapsed_ms: u64,
    pub paths: Vec<String>,
}

// ---------- 纯函数（可单测） ----------

/// 生成快照文件名：`snap-<主机名>-<UTC 时间戳>.tar.gz`
///
/// 主机名做净化：只留 `[A-Za-z0-9._-]`，其余替换为 `_`
/// （防主机名含 `/` 或空格导致路径穿越/命令注入）。
pub fn snapshot_filename(host: &str, ts: u64) -> String {
    let safe: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    // `..` 必须额外消除:点号在白名单里,故 `../../etc` 净化后仍含 `..`
    // （单测抓到 —— 路径穿越风险,不能只靠字符白名单）
    let safe = safe.replace("..", "_");
    // 前导 `.` 会产出隐藏文件,且 `-` 开头会被 tar 当选项,一并剥掉
    let safe = safe.trim_start_matches(['.', '-']).to_string();
    let safe = if safe.is_empty() { "host".to_string() } else { safe };
    format!("snap-{}-{}.tar.gz", safe, ts)
}

/// 组装 tar 命令（**全部路径经单引号转义，绝不拼接用户原文**）。
///
/// 输出到 stdout 之外用 `-f`，`-C /` 让归档内路径为相对路径
/// （这样还原时 `tar -xf 包 -C /` 即可，无需处理绝对路径警告）。
pub fn build_tar_cmd(dir: &str, file: &str, paths: &[String]) -> String {
    let mut cmd = String::from("tar -czf ");
    cmd.push_str(&shq(&format!("{}/{}", dir, file)));
    // 排除敏感文件（tar 的 --exclude 对 -C 后的相对路径生效）
    for e in EXCLUDES {
        cmd.push_str(" --exclude=");
        cmd.push_str(&shq(e));
    }
    cmd.push_str(" -C /");
    for p in paths {
        // 去掉前导 /（-C / 后应给相对路径）
        let rel = p.trim_start_matches('/');
        if rel.is_empty() {
            continue;
        }
        cmd.push(' ');
        cmd.push_str(&shq(rel));
    }
    cmd
}

/// POSIX 单引号转义：`'` → `'\''`
pub fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 从 `df -Pm <dir>` 输出解析可用空间（MB）。
///
/// 取**最后一行**（跳过表头）；字段：Filesystem 1024-blocks Used Available Capacity Mounted
pub fn parse_df_avail_mb(out: &str) -> Option<u64> {
    // 从后往前找第一个非空且非表头的行（`df` 的挂载信息在最后一行）
    out.lines()
        .rev()
        .find(|l| !l.trim().is_empty() && !l.starts_with("Filesystem"))
        .and_then(|l| l.split_whitespace().nth(3))
        .and_then(|v| v.parse::<u64>().ok())
}

/// 从 `du -sm <path>` 输出解析体积（MB）。`du` 失败/超时时返回 None。
pub fn parse_du_mb(out: &str) -> Option<u64> {
    out.lines()
        .next()
        .and_then(|l| l.split_whitespace().next())
        .and_then(|v| v.parse::<u64>().ok())
}

/// 由探测结果推出推荐打包路径（设计稿 §4.8：**不让用户选档位**）。
///
/// 规则：
///   - 单路径体积 > 上限 → 不纳入（大机器自动排除 /home 这类数据目录）
///   - 累计体积 > 可用空间 / 安全系数 → 停止纳入
///   - `/etc` 永远优先（配置才是"改错了要还原"的东西）
pub fn recommend_paths(
    sizes: &[(String, u64)],
    disk_avail_mb: u64,
    max_size_mb: u64,
) -> (Vec<String>, String) {
    let budget = ((disk_avail_mb as f64) / SPACE_SAFETY_FACTOR).floor() as u64;
    let mut picked: Vec<String> = Vec::new();
    let mut total: u64 = 0;
    let mut skipped: Vec<String> = Vec::new();

    // /etc 优先
    let mut ordered: Vec<&(String, u64)> = sizes.iter().collect();
    ordered.sort_by_key(|(p, _)| if p == "/etc" { 0 } else { 1 });

    for (p, mb) in ordered {
        if *mb > max_size_mb {
            skipped.push(format!("{}（{}MB 超上限）", p, mb));
            continue;
        }
        if total + mb > budget {
            skipped.push(format!("{}（累计超预算）", p));
            continue;
        }
        picked.push(p.clone());
        total += mb;
    }

    let reason = if picked.is_empty() {
        "磁盘可用空间不足，无法打包任何路径。请清理空间后重试。".to_string()
    } else if skipped.is_empty() {
        format!(
            "已按本机情况配置：快照 {}（约 {}MB/次），保留 {} 份。磁盘可用 {}MB。",
            picked.join(" "),
            total,
            KEEP,
            disk_avail_mb
        )
    } else {
        format!(
            "已按本机情况配置：快照 {}（约 {}MB/次），保留 {} 份。磁盘可用 {}MB。已跳过：{}",
            picked.join(" "),
            total,
            KEEP,
            disk_avail_mb,
            skipped.join("、")
        )
    };
    (picked, reason)
}

/// 校验：可用空间是否够打包（设计稿 §7.1）
///
/// **绝不"尽力而为"地硬上** —— 磁盘写满的连带损害（日志写不进、数据库无法提交）
/// 远大于"这次没拍成"。
pub fn space_check(disk_avail_mb: u64, est_mb: u64) -> Result<()> {
    let need = ((est_mb as f64) * SPACE_SAFETY_FACTOR).ceil() as u64;
    if disk_avail_mb < need {
        return Err(anyhow!(
            "可用空间不足：落点分区剩 {}MB，待打包约 {}MB，安全余量需 {}MB。请清理空间或缩小范围。",
            disk_avail_mb,
            est_mb,
            need
        ));
    }
    Ok(())
}

/// 校验：是否超单次体积上限
pub fn size_check(est_mb: u64) -> Result<()> {
    if est_mb > MAX_SIZE_MB {
        return Err(anyhow!(
            "待打包体积约 {}MB，超过单次上限 {}MB。请缩小范围。",
            est_mb,
            MAX_SIZE_MB
        ));
    }
    Ok(())
}

// ---------- 执行层 ----------

/// 在会话上执行一条命令并取回 (stdout, exit_code)
async fn run(ssh: &Arc<SshManager>, name: &str, cmd: &str, timeout: u64) -> Result<(String, i32)> {
    let handle = ssh
        .exec_handle(name)
        .await
        .ok_or_else(|| anyhow!("会话 {} 未连接", name))?;
    let kind = ssh.kind_of(name).await;
    let r = run_task_exec(handle, cmd, "/", timeout, kind, None).await?;
    Ok((r.output, r.exit_code))
}

/// 探测：磁盘可用 + 各候选路径体积 + 块级能力（LVM/reflink）
pub async fn probe(ssh: &Arc<SshManager>, name: &str) -> Result<ProbeResult> {
    // 1) 落点分区可用空间
    let home = run(ssh, name, "printf %s \"$HOME\"", PROBE_TIMEOUT_SECS)
        .await
        .map(|(o, _)| o.trim().to_string())
        .unwrap_or_else(|_| "/root".to_string());
    let dir = format!("{}/{}", home.trim_end_matches('/'), DEFAULT_DIR);
    let (df_out, _) = run(
        ssh,
        name,
        &format!("mkdir -p {} && df -Pm {}", shq(&dir), shq(&dir)),
        PROBE_TIMEOUT_SECS,
    )
    .await?;
    let disk_avail_mb = parse_df_avail_mb(&df_out).unwrap_or(0);

    // 2) 各候选路径体积（逐条带超时；超时即判"过大"不纳入）
    let mut path_sizes: Vec<(String, u64)> = Vec::new();
    for p in CANDIDATE_PATHS {
        let cmd = format!("du -sm {} 2>/dev/null", shq(p));
        // 超时 / 不存在 / 无权限 → 一律跳过（视为过大，不纳入）
        if let Ok((o, 0)) = run(ssh, name, &cmd, PROBE_TIMEOUT_SECS).await {
            if let Some(mb) = parse_du_mb(&o) {
                path_sizes.push(((*p).to_string(), mb));
            }
        }
    }

    // 3) 块级能力（仅作提示，不实现 —— 设计稿 §12）
    let (lvm_out, _) = run(
        ssh,
        name,
        "vgs --noheadings -o vg_free --units m 2>/dev/null | tr -d ' m' | head -n1",
        PROBE_TIMEOUT_SECS,
    )
    .await
    .unwrap_or_default();
    let lvm_free_mb = lvm_out.trim().parse::<u64>().unwrap_or(0);

    let (rf_out, rf_code) = run(
        ssh,
        name,
        "d=$(mktemp -d); echo x > $d/a; cp --reflink=always $d/a $d/b 2>/dev/null && echo YES || echo NO; rm -rf $d",
        PROBE_TIMEOUT_SECS,
    )
    .await
    .unwrap_or_default();
    let reflink = rf_code == 0 && rf_out.contains("YES");

    // 4) 内存（规模判定）
    let (mem_out, _) = run(
        ssh,
        name,
        "awk '/MemTotal/{print int($2/1024)}' /proc/meminfo",
        PROBE_TIMEOUT_SECS,
    )
    .await
    .unwrap_or_default();
    let mem_total_mb = mem_out.trim().parse::<u64>().unwrap_or(0);

    let (recommended, reason) = recommend_paths(&path_sizes, disk_avail_mb, MAX_SIZE_MB);

    Ok(ProbeResult {
        disk_avail_mb,
        path_sizes,
        recommended,
        reason,
        lvm_free_mb,
        reflink,
        mem_total_mb,
    })
}

/// 打包快照。
///
/// 顺序（设计稿 §7.1，**不可颠倒**）：
///   1. 空间检查（不够即拒绝，绝不硬上）
///   2. tar 打包
///   3. **校验**（tar -t 成功才算数；失败即删残包）
///
/// **残包比没有包更危险** —— 用户以为有退路，还原时才发现是坏的。
pub async fn pack(
    ssh: &Arc<SshManager>,
    name: &str,
    paths: &[String],
    disk_avail_mb: u64,
    est_mb: u64,
) -> Result<PackResult> {
    if paths.is_empty() {
        return Err(anyhow!("没有要打包的路径"));
    }
    size_check(est_mb)?;
    space_check(disk_avail_mb, est_mb)?;

    // 主机名 + 时间戳
    let (host_out, _) = run(ssh, name, "hostname 2>/dev/null || echo host", PROBE_TIMEOUT_SECS)
        .await
        .unwrap_or_else(|_| ("host".to_string(), 0));
    let host = host_out.trim().to_string();
    let (ts_out, _) = run(ssh, name, "date -u +%s", PROBE_TIMEOUT_SECS).await?;
    let ts: u64 = ts_out.trim().parse().unwrap_or(0);

    let (home_out, _) = run(ssh, name, "printf %s \"$HOME\"", PROBE_TIMEOUT_SECS).await?;
    let dir = format!("{}/{}", home_out.trim().trim_end_matches('/'), DEFAULT_DIR);
    let file = snapshot_filename(&host, ts);
    let full = format!("{}/{}", dir, file);

    // 打包
    let cmd = format!(
        "mkdir -p {} && {} && ls -l {} | awk '{{print $5}}'",
        shq(&dir),
        build_tar_cmd(&dir, &file, paths),
        shq(&full)
    );
    let started = std::time::Instant::now();
    let (out, code) = run(ssh, name, &cmd, PACK_TIMEOUT_SECS).await?;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    if code != 0 {
        // 打包失败 → 清理残包（**不能留在列表里冒充可用快照**）
        let _ = run(ssh, name, &format!("rm -f {}", shq(&full)), 30).await;
        return Err(anyhow!("打包失败（exit {}）：{}", code, out.trim()));
    }

    let size: u64 = out
        .lines()
        .last()
        .and_then(|l| l.trim().parse::<u64>().ok())
        .unwrap_or(0);

    // 校验：tar -t 能列目录才算完整（截断包会 exit≠0，实测确认）
    let (_, vcode) = run(
        ssh,
        name,
        &format!("tar -tzf {} > /dev/null 2>&1", shq(&full)),
        60,
    )
    .await?;
    let verified = vcode == 0;
    if !verified {
        let _ = run(ssh, name, &format!("rm -f {}", shq(&full)), 30).await;
        return Err(anyhow!("快照校验失败（包可能被截断），已删除残包。"));
    }

    Ok(PackResult {
        file,
        path: full,
        size,
        verified,
        elapsed_ms,
        paths: paths.to_vec(),
    })
}

/// 列出快照（按时间倒序）。**只列校验通过的包** —— 残包不进列表。
pub async fn list(ssh: &Arc<SshManager>, name: &str) -> Result<Vec<SnapshotEntry>> {
    let (home_out, _) = run(ssh, name, "printf %s \"$HOME\"", PROBE_TIMEOUT_SECS).await?;
    let dir = format!("{}/{}", home_out.trim().trim_end_matches('/'), DEFAULT_DIR);
    // 输出: 文件名<TAB>字节数<TAB>mtime秒
    let cmd = format!(
        "mkdir -p {d} && find {d} -maxdepth 1 -name 'snap-*.tar.gz' -printf '%f\\t%s\\t%T@\\n' 2>/dev/null | sort -t$'\\t' -k3 -nr",
        d = shq(&dir)
    );
    let (out, _) = run(ssh, name, &cmd, PROBE_TIMEOUT_SECS).await?;

    let mut entries = Vec::new();
    for line in out.lines() {
        let mut it = line.split('\t');
        let (Some(f), Some(sz), Some(mt)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if f.is_empty() || !f.starts_with("snap-") {
            continue;
        }
        let size = sz.trim().parse::<u64>().unwrap_or(0);
        // %T@ 形如 "1786791845.123456" → 取整数部分
        let created_at = mt
            .trim()
            .split('.')
            .next()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        entries.push(SnapshotEntry {
            file: f.to_string(),
            path: format!("{}/{}", dir, f),
            created_at,
            size,
            verified: true, // 残包已在 pack 里删掉；此处只列存在的
            paths: Vec::new(),
        });
    }
    Ok(entries)
}

/// 删除指定快照，并执行保留策略（超出 KEEP 份删最旧）
pub async fn prune(ssh: &Arc<SshManager>, name: &str) -> Result<usize> {
    let entries = list(ssh, name).await?;
    if entries.len() <= KEEP {
        return Ok(0);
    }
    let mut removed = 0;
    for e in entries.iter().skip(KEEP) {
        let (_, code) = run(ssh, name, &format!("rm -f {}", shq(&e.path)), 30).await?;
        if code == 0 {
            removed += 1;
        }
    }
    Ok(removed)
}

/// 删除单个快照
pub async fn remove(ssh: &Arc<SshManager>, name: &str, file: &str) -> Result<()> {
    // 防路径穿越：只接受纯文件名
    if file.contains('/') || file.contains("..") || !file.starts_with("snap-") {
        return Err(anyhow!("非法的快照文件名"));
    }
    let (home_out, _) = run(ssh, name, "printf %s \"$HOME\"", PROBE_TIMEOUT_SECS).await?;
    let dir = format!("{}/{}", home_out.trim().trim_end_matches('/'), DEFAULT_DIR);
    let (_, code) = run(ssh, name, &format!("rm -f {}", shq(&format!("{}/{}", dir, file))), 30).await?;
    if code != 0 {
        return Err(anyhow!("删除失败"));
    }
    Ok(())
}

// ---------- 单测 ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_sanitizes_host() {
        assert_eq!(snapshot_filename("master", 100), "snap-master-100.tar.gz");
        // 危险字符必须被净化（防路径穿越）
        assert_eq!(snapshot_filename("a/b", 1), "snap-a_b-1.tar.gz");
        assert_eq!(snapshot_filename("a b;rm -rf /", 1), "snap-a_b_rm_-rf__-1.tar.gz");
        assert_eq!(snapshot_filename("", 1), "snap-host-1.tar.gz");
        // 路径穿越:净化后不得含 "..",且不得以 . 或 - 开头
        let f = snapshot_filename("../../etc", 1);
        assert!(!f.contains(".."), "不得含 ..: {f}");
        assert!(!f.starts_with('.'), "不得以 . 开头: {f}");
        // 前导 `-` 必须被剥掉:否则 tar 会把文件名当选项。
        // 注意判据要写对 —— "snap-rf-1.tar.gz" 里**确实含** "-rf-" 子串
        // (snap[-rf-]1),故不能用 contains("-rf-");应断言它不以 "snap--" 开头。
        let g = snapshot_filename("-rf", 1);
        assert_eq!(g, "snap-rf-1.tar.gz", "前导 - 须剥掉: {g}");
        assert!(!g.starts_with("snap--"), "不得出现双横线: {g}");
    }

    #[test]
    fn tar_cmd_quotes_paths() {
        let cmd = build_tar_cmd("/root/.helm-snapshots", "s.tar.gz", &["/etc".into()]);
        assert!(cmd.starts_with("tar -czf '/root/.helm-snapshots/s.tar.gz'"));
        assert!(cmd.contains("--exclude='etc/shadow'"));
        assert!(cmd.contains("--exclude='etc/ssh/ssh_host_*'"));
        assert!(cmd.contains("-C /"));
        assert!(cmd.ends_with(" 'etc'"));
    }

    #[test]
    fn tar_cmd_escapes_single_quote() {
        let cmd = build_tar_cmd("/d", "f", &["/a'b".into()]);
        // 单引号必须转义为 '\''，否则可注入
        assert!(cmd.contains("'a'\\''b'"), "实际: {cmd}");
    }

    #[test]
    fn parse_df_and_du() {
        let df = "Filesystem             1M-blocks  Used Available Capacity Mounted on\n\
                  /dev/mapper/centos-root     17280  5120     12160      30% /\n";
        assert_eq!(parse_df_avail_mb(df), Some(12160));
        assert_eq!(parse_du_mb("36\t/etc\n"), Some(36));
        assert_eq!(parse_du_mb(""), None);
        assert_eq!(parse_du_mb("du: cannot access\n"), None);
    }

    #[test]
    fn recommend_prefers_etc_and_skips_oversized() {
        // 上限 500MB:/etc(36) 与 /opt(120) 可进,/home(40000) 与 /root(832) 超限
        let sizes = vec![
            ("/etc".to_string(), 36),
            ("/home".to_string(), 40000),
            ("/root".to_string(), 832),
            ("/opt".to_string(), 120),
        ];
        let (picked, reason) = recommend_paths(&sizes, 12160, 500);
        assert!(picked.contains(&"/etc".to_string()));
        assert!(picked.contains(&"/opt".to_string()));
        assert!(!picked.contains(&"/home".to_string()), "超上限的必须被排除");
        assert!(!picked.contains(&"/root".to_string()), "832MB > 上限 500MB,须排除");
        assert!(reason.contains("已跳过"), "理由须说明跳过项: {reason}");
    }

    #[test]
    fn recommend_respects_space_budget() {
        // 可用 100MB → 预算 33MB，只有 /etc 装得下
        let sizes = vec![("/etc".to_string(), 30), ("/root".to_string(), 800)];
        let (picked, _) = recommend_paths(&sizes, 100, 500);
        assert_eq!(picked, vec!["/etc".to_string()]);
    }

    #[test]
    fn recommend_never_picks_etc_last() {
        // /etc 必须优先（即使输入顺序把它放最后）
        let sizes = vec![
            ("/opt".to_string(), 10),
            ("/usr/local".to_string(), 10),
            ("/etc".to_string(), 10),
        ];
        let (picked, _) = recommend_paths(&sizes, 10000, 500);
        assert_eq!(picked.first().map(|s| s.as_str()), Some("/etc"));
    }

    #[test]
    fn space_check_rejects_when_tight() {
        // 可用 100MB，待打包 50MB → 需 150MB → 拒绝
        assert!(space_check(100, 50).is_err());
        // 可用 200MB，待打包 50MB → 需 150MB → 通过
        assert!(space_check(200, 50).is_ok());
    }

    #[test]
    fn size_check_enforces_cap() {
        assert!(size_check(400).is_ok());
        assert!(size_check(600).is_err());
    }

    #[test]
    fn space_check_error_mentions_numbers() {
        let e = space_check(100, 50).unwrap_err().to_string();
        // 报错必须给出具体数字（用户才能判断要清理多少）
        assert!(e.contains("100"), "{e}");
        assert!(e.contains("50"), "{e}");
        assert!(e.contains("150"), "{e}");
    }
}
