// ============================================================================
// intercept.rs - 命令前拦截（P127 二期：让不可逆命令变可逆的**主路径**）
//
// 目标（设计稿 §4）：**抢在命令执行前**判级，危险的先拍快照再放行 → 零丢失。
//
// ## 为什么必须在 send_input 拦（而不是前端）
//
// 前端是**逐键**发送的（`term.onData` 每按键一次 `send_input`），命令文本早已
// 逐字符进了服务器 PTY。但关键洞察是：
//
//   **命令在 readline 的行编辑缓冲里，尚未执行 —— 扣住 `\r` 就能抢在它前面。**
//
// 前端拦不住（P19 栽过：Tab 补全/↑历史/Ctrl+V 粘贴都漏），后端拿到的字节流最完整。
//
// ## 铁律：拿不准就不拦（宁可不拦，不可错拦）
//
// `↑` 历史回放时前端只发 `\x1b[A`，**后端根本不知道服务器会回放哪条命令**。
// 错拦的代价是"我按了回车却没反应"—— 这是**功能坏掉**的体验，比漏拦更糟
// （漏拦还有滚动基线兜底，见设计稿 §5）。
//
// ## 本模块只做"判定"，不做"弹窗"
//
// 状态机产出 `Decision`，由 core.rs 决定是直接放行还是挂起等用户确认。
// 这样纯逻辑可单测（不依赖 Tauri/网络）。
// ============================================================================

use crate::safety::{check_danger, DangerLevel};

/// 每会话的行缓冲状态
#[derive(Debug, Default, Clone)]
pub struct LineBuf {
    /// 当前累积的命令行（仅可见字符 + 退格修正后的结果）
    buf: String,
    /// 本行是否"不可信"（出现过 `\x1b` 等无法追踪的编辑操作）
    untrusted: bool,
}

/// 对**一次 `send_input` 调用所携带的整段字节**的处理结论。
///
/// 为什么是"整段"而不是"单个字节"：前端逐键发送时每段只有一个字符，
/// 但 **Ctrl+V 粘贴会一次发来整个多行文本块**（`paste()` 直接调 `sendInput`）。
/// 故决策必须覆盖整段 —— 危险时扣住整段，确认后再原样重放。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 原样发送（默认；含"不可信行"与 Safe/Warning 命令）
    Pass,
    /// 检测到危险命令：**扣住整段**，等用户确认后原样重放
    Confirm { cmd: String, level: DangerLevel, reason: String },
}

impl LineBuf {
    #[allow(dead_code)] // 测试与调试用
    pub fn new() -> Self {
        Self::default()
    }

    /// 清空（会话切换/断开时调用）
    pub fn reset(&mut self) {
        self.buf.clear();
        self.untrusted = false;
    }

    /// 当前已累积的命令行（供测试与调试）
    #[allow(dead_code)]
    pub fn current(&self) -> &str {
        &self.buf
    }

    /// 处理一段输入字节，返回对该**整段**的处理结论。
    ///
    /// 扫描整段（而非遇 `\r` 即返回），这样一段里的多行都能正确累积，
    /// 且"哪一行危险"能被准确识别。
    ///
    /// **调用方契约**：`Hold`/`Confirm` 时**不得发送该段任何字节**；
    /// 确认通过后把原段**原样重放**给 `send_input` 即可。
    pub fn feed(&mut self, data: &[u8], intercept_enabled: bool) -> Decision {
        let mut hit: Option<(String, DangerLevel, String)> = None;

        for &b in data {
            match b {
                // 回车/换行 → 一行结束，做判定
                b'\r' | b'\n' => {
                    let cmd = self.buf.trim().to_string();
                    let untrusted = self.untrusted;
                    // 无论判定结果如何，本行缓冲都结束
                    self.buf.clear();
                    self.untrusted = false;

                    // 拦截未开启 / 空行 / 不可信行 → 放行（铁律，见模块头）
                    if !intercept_enabled || cmd.is_empty() || untrusted {
                        continue;
                    }
                    let (level, reason) = check_danger(&cmd);
                    if level == DangerLevel::Critical && hit.is_none() {
                        // 只记第一个危险命令（整段都会被扣住）
                        hit = Some((cmd, level, reason));
                    }
                    // Safe / Warning 默认放行（设计稿 §4.7：只对 Critical 弹）
                }
                // 退格 → 删尾（不可信行不追踪编辑，故不生效）
                0x7f | 0x08 if !self.untrusted => {
                    self.buf.pop();
                }
                // ESC 序列 → 本行不可信（↑历史 / 补全后的光标移动 / Delete 等）
                // 设计稿 §4.4：v3 首版一律标记不可信，只保留退格/回车/可见字符。
                0x1b => {
                    self.untrusted = true;
                }
                // Tab（补全）：补全结果在服务器侧，输入侧看不到 → 本行不可信
                b'\t' => {
                    self.untrusted = true;
                }
                // Ctrl+C / Ctrl+U：取消当前输入行
                0x03 | 0x15 => {
                    self.buf.clear();
                    self.untrusted = false;
                }
                // 可打印字符（含 UTF-8 多字节的续字节 ≥ 0x80）
                // 按**字节**收集（多字节字符拆成多个 char 存）；
                // 判级只看 ASCII 命令名不受影响，展示前用 normalized() 还原。
                0x20..=0x7e | 0x80..=0xff if !self.untrusted => {
                    self.buf.push(b as char);
                }
                // 其余控制字符忽略
                _ => {}
            }
        }

        match hit {
            Some((cmd, level, reason)) => Decision::Confirm { cmd, level, reason },
            None => Decision::Pass,
        }
    }

    /// 把按字节收集的缓冲修正为合法 UTF-8（多字节字符逐字节 push 会乱码）。
    ///
    /// 判级只看 ASCII 命令名，不受影响；此函数用于**确认卡展示与日志**。
    #[allow(dead_code)]
    pub fn normalized(&self) -> String {
        let bytes: Vec<u8> = self.buf.chars().map(|c| c as u8).collect();
        String::from_utf8_lossy(&bytes).to_string()
    }
}

// ---------- 单测 ----------

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_str(lb: &mut LineBuf, s: &str, on: bool) -> Decision {
        lb.feed(s.as_bytes(), on)
    }

    #[test]
    fn plain_safe_command_passes() {
        let mut lb = LineBuf::new();
        assert_eq!(feed_str(&mut lb, "ls -la", true), Decision::Pass);
        let d = feed_str(&mut lb, "\r", true);
        assert_eq!(d, Decision::Pass);
    }

    #[test]
    fn critical_command_triggers_confirm() {
        let mut lb = LineBuf::new();
        feed_str(&mut lb, "rm -rf /", true);
        match feed_str(&mut lb, "\r", true) {
            Decision::Confirm { cmd, level, .. } => {
                assert_eq!(cmd, "rm -rf /");
                assert_eq!(level, DangerLevel::Critical);
            }
            other => panic!("应为 Confirm,实际 {other:?}"),
        }
    }

    #[test]
    fn rm_rf_etc_is_warning_not_intercepted() {
        // 设计稿 §4.6：rm -rf /etc 是 Warning(不是 Critical)→ 默认不拦
        // 这一条固化"只拦 Critical"的语义,防止误升级为拦 Warning
        let mut lb = LineBuf::new();
        feed_str(&mut lb, "rm -rf /etc/nginx", true);
        assert_eq!(feed_str(&mut lb, "\r", true), Decision::Pass);
    }

    #[test]
    fn backspace_corrects_command() {
        let mut lb = LineBuf::new();
        // 输入 "rm -rf /X" 再退格 → 得到 "rm -rf /"
        feed_str(&mut lb, "rm -rf /X", true);
        feed_str(&mut lb, "\x7f", true);
        match feed_str(&mut lb, "\r", true) {
            Decision::Confirm { cmd, .. } => assert_eq!(cmd, "rm -rf /", "退格必须生效"),
            other => panic!("应为 Confirm,实际 {other:?}"),
        }
    }

    #[test]
    fn esc_makes_line_untrusted_and_passes() {
        // ↑ 历史回放:只发 \x1b[A,后端不知道会回放什么 → 铁律:不拦
        let mut lb = LineBuf::new();
        feed_str(&mut lb, "\x1b[A", true);
        // 即使后续字节凑出危险命令,也不得拦截(因为可能是回放的)
        feed_str(&mut lb, "rm -rf /", true);
        assert_eq!(feed_str(&mut lb, "\r", true), Decision::Pass, "不可信行必须放行");
    }

    #[test]
    fn tab_makes_line_untrusted() {
        // Tab 补全:补全结果在服务器侧,输入侧看不到 → 不可信
        let mut lb = LineBuf::new();
        feed_str(&mut lb, "rm -rf /et", true);
        feed_str(&mut lb, "\t", true);
        assert_eq!(feed_str(&mut lb, "\r", true), Decision::Pass, "Tab 补全行必须放行");
    }

    #[test]
    fn disabled_never_intercepts() {
        let mut lb = LineBuf::new();
        feed_str(&mut lb, "rm -rf /", false);
        assert_eq!(feed_str(&mut lb, "\r", false), Decision::Pass, "未开启时一律放行");
    }

    #[test]
    fn empty_line_passes() {
        let mut lb = LineBuf::new();
        assert_eq!(feed_str(&mut lb, "\r", true), Decision::Pass);
    }

    #[test]
    fn buffer_resets_after_each_line() {
        let mut lb = LineBuf::new();
        feed_str(&mut lb, "ls", true);
        feed_str(&mut lb, "\r", true);
        assert_eq!(lb.current(), "", "回车后缓冲必须清空");
        // 第二行不应受第一行影响
        feed_str(&mut lb, "pwd", true);
        assert_eq!(feed_str(&mut lb, "\r", true), Decision::Pass);
    }

    #[test]
    fn untrusted_flag_clears_after_line() {
        let mut lb = LineBuf::new();
        feed_str(&mut lb, "\x1b[A", true);
        feed_str(&mut lb, "\r", true);
        // 下一行应恢复可信
        feed_str(&mut lb, "rm -rf /", true);
        match feed_str(&mut lb, "\r", true) {
            Decision::Confirm { .. } => {}
            other => panic!("新行应恢复可信并拦截,实际 {other:?}"),
        }
    }

    #[test]
    fn ctrl_c_clears_line() {
        let mut lb = LineBuf::new();
        feed_str(&mut lb, "rm -rf /", true);
        feed_str(&mut lb, "\x03", true); // Ctrl+C
        assert_eq!(feed_str(&mut lb, "\r", true), Decision::Pass, "Ctrl+C 后应清空");
    }

    #[test]
    fn multi_command_line_uses_first_dangerous_segment() {
        // 复合命令:任一段 Critical 即整体 Critical(check_danger 既有语义)
        let mut lb = LineBuf::new();
        feed_str(&mut lb, "ls && rm -rf /", true);
        match feed_str(&mut lb, "\r", true) {
            Decision::Confirm { .. } => {}
            other => panic!("复合命令中的危险段应被拦,实际 {other:?}"),
        }
    }

    #[test]
    fn quoted_danger_is_safe() {
        // echo 'rm -rf /' → Safe(P29 既有语义),不得误拦
        let mut lb = LineBuf::new();
        feed_str(&mut lb, "echo 'rm -rf /'", true);
        assert_eq!(feed_str(&mut lb, "\r", true), Decision::Pass, "引号内不得误拦");
    }

    #[test]
    fn split_across_chunks_still_detected() {
        // 逐键发送:命令被拆成多次 feed,仍必须正确累积
        let mut lb = LineBuf::new();
        for ch in ["r", "m", " ", "-", "r", "f", " ", "/"] {
            feed_str(&mut lb, ch, true);
        }
        match feed_str(&mut lb, "\r", true) {
            Decision::Confirm { cmd, .. } => assert_eq!(cmd, "rm -rf /"),
            other => panic!("跨块累积失败,实际 {other:?}"),
        }
    }

    #[test]
    fn tab_before_danger_still_untrusted() {
        // 顺序无关:先 Tab 后危险命令,仍不可信
        let mut lb = LineBuf::new();
        feed_str(&mut lb, "\t", true);
        feed_str(&mut lb, "rm -rf /", true);
        assert_eq!(feed_str(&mut lb, "\r", true), Decision::Pass);
    }

    #[test]
    fn paste_multi_line_detects_any_dangerous_line() {
        // 粘贴多行:整段一次到达,任意一行危险即整段扣住
        let mut lb = LineBuf::new();
        let d = feed_str(&mut lb, "ls\rrm -rf /\r", true);
        match d {
            Decision::Confirm { cmd, .. } => assert_eq!(cmd, "rm -rf /"),
            other => panic!("多行粘贴应识别出危险行,实际 {other:?}"),
        }
    }

    #[test]
    fn paste_safe_multi_line_passes() {
        let mut lb = LineBuf::new();
        assert_eq!(feed_str(&mut lb, "ls\rpwd\rwhoami\r", true), Decision::Pass);
    }

    #[test]
    fn paste_danger_on_first_line_detected() {
        let mut lb = LineBuf::new();
        match feed_str(&mut lb, "rm -rf /\rls\r", true) {
            Decision::Confirm { cmd, .. } => assert_eq!(cmd, "rm -rf /"),
            other => panic!("首行危险应被拦,实际 {other:?}"),
        }
    }

    #[test]
    fn utf8_command_normalized() {
        let mut lb = LineBuf::new();
        feed_str(&mut lb, "echo 你好", true);
        assert_eq!(lb.normalized(), "echo 你好", "多字节字符须能还原");
    }

    #[test]
    fn shutdown_detected() {
        let mut lb = LineBuf::new();
        feed_str(&mut lb, "shutdown -h now", true);
        match feed_str(&mut lb, "\r", true) {
            Decision::Confirm { level, .. } => assert_eq!(level, DangerLevel::Critical),
            other => panic!("shutdown 应被拦,实际 {other:?}"),
        }
    }

    #[test]
    fn echo_shutdown_not_intercepted() {
        // P29 既有语义:echo shutdown 是 Safe
        let mut lb = LineBuf::new();
        feed_str(&mut lb, "echo shutdown", true);
        assert_eq!(feed_str(&mut lb, "\r", true), Decision::Pass);
    }
}
