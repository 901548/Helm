// ============================================================================
// recorder.rs - 操作记录器（P70）：为训练终端操作小模型采集轨迹数据
//   - 用户输入：PTY 字节流按行组装（剥 ANSI 转义、退格修整），Enter 落一行
//   - AI 轨迹：(任务, 命令, 危险级, 退出码, cwd, 输出截断) —— 训练核心三元组
//   - 落盘：config 同目录 logs/terminal-YYYYMMDD.jsonl（UTC 按天滚动，append-only）
//   - 记录失败静默忽略：日志器绝不能拖垮主功能
// ============================================================================

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::json;

const MAX_INPUT_LINE: usize = 2000;
const MAX_BUF_CHARS: usize = 4000;

/// 取锁：中毒（持锁线程 panic）时取回内部数据继续用。
///
/// 记录器是"绝不能拖垮主功能"的旁路组件，而这两把锁各护一个 HashMap 的
/// 独立操作、无跨字段不变量——中毒后恢复内部数据比让后续记录全线 panic 安全。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Recorder {
    dir: PathBuf,
    enabled: AtomicBool,
    /// 每会话未成行的输入半行（Enter 前的字符，跨 invoke 分包累积）
    bufs: Mutex<HashMap<String, String>>,
    /// 每会话跨 invoke 遗留的不完整 UTF-8 字节（多字节字符被分包时暂存，下次 prepend）
    pending: Mutex<HashMap<String, Vec<u8>>>,
    /// 落盘串行化。实测（Windows）多线程同时 append 同一 JSONL 会**交错**成
    /// 非法行（不是丢行而是字节级混写），而记录是要喂训练的 JSONL，损坏即报废。
    write: Mutex<()>,
}

impl Recorder {
    pub fn new(dir: PathBuf, enabled: bool) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        Self {
            dir,
            enabled: AtomicBool::new(enabled),
            bufs: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            write: Mutex::new(()),
        }
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::SeqCst);
    }

    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }

    /// 用户键入字节流 → 行级记录（同步落盘）。
    ///
    /// 生产路径一律走 `record_input_async`（落盘在阻塞线程池）；本同步版仅供
    /// 同步单测直接驱动组装 + 落盘逻辑使用。
    #[cfg(test)]
    pub fn record_input(&self, session: &str, data: &[u8]) {
        self.write_records(self.assemble_input(session, data));
    }

    /// 同上，但把落盘移进阻塞线程池。
    ///
    /// 调用点是 async 命令与 AI 任务路径：`OpenOptions::open` + `writeln!` 是阻塞 I/O，
    /// 在 tokio worker 上做会拖慢整个异步线程池（P100-5）。
    pub async fn record_input_async(self: &Arc<Self>, session: &str, data: &[u8]) {
        let records = self.assemble_input(session, data);
        if records.is_empty() {
            return;
        }
        let me = Arc::clone(self);
        let _ = tauri::async_runtime::spawn_blocking(move || me.write_records(records)).await;
    }

    /// 字节流 → 待落盘记录（不碰磁盘）。
    ///
    /// 字节流可能任意分包，按会话累积半行；ESC 序列跳过（方向键/历史回放不会污染），
    /// 退格弹出末字符（记录修正后的最终行）。
    fn assemble_input(&self, session: &str, data: &[u8]) -> Vec<serde_json::Value> {
        if !self.enabled.load(Ordering::SeqCst) || data.is_empty() {
            return Vec::new();
        }
        // P96：拼接上次跨包遗留的不完整 UTF-8 字节——旧实现 `from_utf8` 对
        // 截断切片返回 Err 直接丢弃，多字节字符被整字丢光（违反"任意分包"契约）
        let mut stream: Vec<u8> = lock(&self.pending).remove(session).unwrap_or_default();
        stream.extend_from_slice(data);
        let data = stream;
        let mut leftover: Vec<u8> = Vec::new();
        let mut completed: Vec<String> = Vec::new();
        {
            let mut bufs = lock(&self.bufs);
            let buf = bufs.entry(session.to_string()).or_default();
            let mut i = 0;
            while i < data.len() {
                let b = data[i];
                if b == 0x1b {
                    // ESC 序列跳过。P96：区分 CSI（ESC [ ...）与单字节 ESC 序列——
                    // CSI 的引入符 `[`(0x5B) 之后，参数/中间字节为 0x20-0x3F，
                    // 终结字节为 0x40-0x7E（含字母与 `~`）；旧逻辑只认字母/BEL，
                    // `~` 结尾的 Insert/Delete/Home/End/PgUp/PgDn 会吞掉同包后续键入
                    i += 1;
                    if i < data.len() && data[i] == b'[' {
                        i += 1; // 跳过 CSI 引入符 [
                        while i < data.len() {
                            let c = data[i];
                            i += 1;
                            if c >= 0x40 {
                                break;
                            }
                        }
                    } else {
                        while i < data.len() {
                            let c = data[i];
                            i += 1;
                            if c == 0x07 || c.is_ascii_alphabetic() {
                                break;
                            }
                        }
                    }
                    continue;
                }
                if b == b'\r' || b == b'\n' {
                    completed.push(std::mem::take(buf));
                    i += 1;
                    continue;
                }
                if b == 0x08 || b == 0x7f {
                    buf.pop();
                    i += 1;
                    continue;
                }
                if b >= 0x20 {
                    let len = utf8_len(b);
                    let end = (i + len).min(data.len());
                    match std::str::from_utf8(&data[i..end]) {
                        Ok(ch) => {
                            buf.push_str(ch);
                            i = end;
                        }
                        Err(_) => {
                            if i + len > data.len() {
                                // 跨包不完整多字节序列：留待下次 prepend 拼接
                                leftover.extend_from_slice(&data[i..]);
                                i = data.len();
                            } else {
                                // 完整长度但非法字节序列：丢弃
                                i = end;
                            }
                        }
                    }
                    continue;
                }
                i += 1; // 其余控制字符丢弃
            }
            // 防半行无限膨胀：超长截尾
            let chars = buf.chars().count();
            if chars > MAX_BUF_CHARS {
                *buf = buf.chars().skip(chars - MAX_INPUT_LINE).collect();
            }
        }
        if !leftover.is_empty() {
            lock(&self.pending).insert(session.to_string(), leftover);
        }
        completed
            .into_iter()
            .filter_map(|line| {
                let t = line.trim();
                if t.is_empty() {
                    return None;
                }
                let out: String = t.chars().take(MAX_INPUT_LINE).collect();
                Some(json!({ "type": "input", "session": session, "data": out }))
            })
            .collect()
    }

    /// 落一条结构化记录（自动附 UTC 时间戳；禁用态零开销）。
    ///
    /// 同上：生产路径走 `log_async`，本同步版供单测使用。
    #[cfg(test)]
    pub fn log(&self, record: serde_json::Value) {
        if !self.enabled.load(Ordering::SeqCst) {
            return;
        }
        self.write_records(vec![record]);
    }

    /// 同上，落盘移进阻塞线程池（AI 轨迹在 async 任务里记录）
    pub async fn log_async(self: &Arc<Self>, record: serde_json::Value) {
        if !self.enabled.load(Ordering::SeqCst) {
            return;
        }
        let me = Arc::clone(self);
        let _ = tauri::async_runtime::spawn_blocking(move || me.write_records(vec![record])).await;
    }

    /// append-only 落盘：逐条补 UTC 时间戳后写 JSONL。
    ///
    /// 整体持写锁串行化：多会话并行时（P55 起 AI 任务与会话输入可能同时落盘）
    /// 各自 append 会交错。失败静默忽略——日志器绝不能拖垮主功能。
    fn write_records(&self, records: Vec<serde_json::Value>) {
        if records.is_empty() {
            return;
        }
        let _guard = lock(&self.write);
        for mut record in records {
            let (date, iso) = now_parts();
            if let Some(obj) = record.as_object_mut() {
                obj.insert("ts".into(), json!(iso));
            }
            let path = self.dir.join(format!("terminal-{date}.jsonl"));
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = writeln!(f, "{record}");
            }
        }
    }
}

/// 按首字节判 UTF-8 字符长度（非法首字节按 1 处理，由 from_utf8 兜底丢弃）
fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

/// 当前 UTC 时间 → (日期 "20260828", ISO "2026-08-28T12:34:05Z")
fn now_parts() -> (String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let (y, m, d) = civil_from_days(secs.div_euclid(86400));
    let rem = secs.rem_euclid(86400);
    (
        format!("{y:04}{m:02}{d:02}"),
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60),
    )
}

/// Howard Hinnant civil_from_days（与 fs.rs 同款零依赖实现）
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// 每测试独立目录（cargo test 并行线程共享路径会互删文件）
    fn tmp_recorder() -> (Recorder, PathBuf) {
        let dir = std::env::temp_dir()
            .join(format!("helm-rec-test-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
        (Recorder::new(dir.clone(), true), dir)
    }

    #[test]
    fn input_lines_assemble_across_packets() {
        let (r, dir) = tmp_recorder();
        r.record_input("s", b"ls ");
        r.record_input("s", b"-la\r");
        r.record_input("s", b"pwd\r");
        let f = std::fs::read_to_string(dir.join(format!("terminal-{}.jsonl", now_parts().0))).unwrap();
        let lines: Vec<&str> = f.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"ls -la\""));
        assert!(lines[1].contains("\"pwd\""));
        assert!(lines[0].contains("\"type\":\"input\"") && lines[0].contains("\"ts\""));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn input_esc_sequences_and_backspace() {
        let (r, dir) = tmp_recorder();
        // ↑(ESC[A) 历史键不进缓冲；退格修正；最终行是修正后的
        r.record_input("s", b"\x1b[Aab\x7fc\r");
        let f = std::fs::read_to_string(dir.join(format!("terminal-{}.jsonl", now_parts().0))).unwrap();
        assert!(f.contains("\"ac\""), "got: {f}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn esc_csi_tilde_terminator_does_not_swallow_following_input() {
        // P96：Insert(ESC[2~) 以 `~` 结尾，旧跳过循环只认字母/BEL，会吞掉后随的 "hi"
        let (r, dir) = tmp_recorder();
        r.record_input("s", b"\x1b[2~hi\r");
        let f = std::fs::read_to_string(dir.join(format!("terminal-{}.jsonl", now_parts().0))).unwrap();
        assert!(f.contains("\"hi\""), "`~` 结尾的 CSI 序列不应吞后续输入，got: {f}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn multibyte_char_split_across_packets_reassembled() {
        // P96：「中」= E4 B8 AD，拆成两包分别送入，应重组为一个字符而非整字丢失
        let (r, dir) = tmp_recorder();
        let ch = "中".as_bytes();
        r.record_input("s", &ch[..2]);
        r.record_input("s", &[ch[2], b'\r']);
        let f = std::fs::read_to_string(dir.join(format!("terminal-{}.jsonl", now_parts().0))).unwrap();
        assert!(f.contains("中"), "跨包多字节字符应重组，got: {f}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn input_empty_lines_skipped_and_disabled_noop() {
        let (r, dir) = tmp_recorder();
        r.record_input("s", b"\r\r");
        assert!(std::fs::read_dir(&dir).unwrap().count() == 0);
        r.set_enabled(false);
        r.record_input("s", b"ls\r");
        r.log(json!({"type":"x"}));
        assert!(std::fs::read_dir(&dir).unwrap().count() == 0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn civil_roundtrip_known_anchors() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(31), (1970, 2, 1)); // 1 月 31 天
        assert_eq!(civil_from_days(365), (1971, 1, 1)); // 1970 非闰年
        assert_eq!(civil_from_days(366), (1971, 1, 2));
    }

    /// P100-5：持锁线程 panic 使锁中毒后，记录器仍须工作——
    /// 旧实现 `.lock().unwrap()` 会让此后每次记录连锁 panic（日志器拖垮主功能）
    #[test]
    fn poisoned_lock_does_not_break_recording() {
        let (r, dir) = tmp_recorder();
        let r = Arc::new(r);
        let r2 = Arc::clone(&r);
        let _ = std::thread::spawn(move || {
            let _guard = r2.bufs.lock().unwrap();
            panic!("boom");
        })
        .join();
        assert!(r.bufs.is_poisoned(), "前置条件：锁应已中毒");

        r.record_input("s", b"ls\r");
        let f = std::fs::read_to_string(dir.join(format!("terminal-{}.jsonl", now_parts().0))).unwrap();
        assert!(f.contains("\"ls\""), "中毒后仍应记录，got: {f}");
        std::fs::remove_dir_all(dir).ok();
    }

    /// P100-5：多线程并发记录不丢行（各线程会话独立，锁与 append 写均须安全）
    #[test]
    fn concurrent_writes_do_not_lose_lines() {
        let (r, dir) = tmp_recorder();
        let r = Arc::new(r);
        let threads = 8;
        let per_thread = 20;
        let mut handles = Vec::new();
        for t in 0..threads {
            let r = Arc::clone(&r);
            handles.push(std::thread::spawn(move || {
                let session = format!("s{t}");
                for n in 0..per_thread {
                    r.record_input(&session, format!("cmd{n}\r").as_bytes());
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let f = std::fs::read_to_string(dir.join(format!("terminal-{}.jsonl", now_parts().0))).unwrap();
        let lines: Vec<&str> = f.lines().collect();
        assert_eq!(lines.len(), threads * per_thread, "并发写丢行：{}", lines.len());
        for l in &lines {
            // 行内不交错（每条都是完整 JSON）
            assert!(serde_json::from_str::<serde_json::Value>(l).is_ok(), "行被交错破坏: {l}");
        }
        std::fs::remove_dir_all(dir).ok();
    }
}
