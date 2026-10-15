// ============================================================================
// baseline.rs - 滚动基线（P127 三期：拦截层的兜底）
//
// ## 为什么需要它
//
// 拦截层（二期）只挡 `check_danger == Critical`，也就是 `rm -rf /`、`shutdown`
// 这类**系统级**不可逆。但现实里最常见的事故是：
//
//     sed -i 's/80/8080/' /etc/nginx/nginx.conf     ← 判级 Safe，拦不住
//     手滑改错一个参数 → 服务起不来 → 没有退路
//
// 这类命令**不该拦**（太频繁，拦了用户会关掉功能 = 等于没有）。退路靠滚动基线：
//
//     首次写操作 → 立即拍基线（否则"改了 10 个文件后才拍"，前 9 个的原始状态永久丢失）
//     之后每 N 条写操作 或 T 分钟 → 更新
//     手滑改坏 → 退路 = 最近一次基线（最多丢 T 分钟）
//
// ## 为什么用 is_readonly_command 而不是"命令条数"（用户明确要求"弹性"）
//
//     > 用户原话："这个肯定要是弹性的，如果都是查询肯定没必要。"
//
// `safety.rs::is_readonly_command` 就是现成判据（77 个白名单命令 + 每命令参数守卫
// + 重定向/命令替换/后台全禁，白名单外保守视为写操作）。于是：
//
//     50 条 ls/grep/tail  → 一次都不拍
//     3 条 sed -i         → 会拍
//
// ## 触发通道：OSC7 提示符（复用 P24）
//
// 用 OSC7 而不是"解析输入字节流"：OSC7 的语义是**一条命令执行完毕**（bash 每次
// 重画提示符都发），可靠；而输入字节流只说明"用户敲了什么"（含退格/方向键/Tab），
// 与"实际执行了什么"不等价。
//
// 非 bash 会话（无 OSC7 标记）**不启用自动基线** —— 宁可不自动，不可乱自动。
//
// ## 本模块只做"该不该拍"的判定与记账，不做实际打包
//
// 打包复用 `snapshot::pack`。这样判定逻辑可脱离网络单测。
// ============================================================================

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// 触发策略（设计稿 §4.8：**不让用户选档位**，由探测结果推出）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaselinePolicy {
    /// 每 N 条写操作更新一次
    pub every_n_writes: u32,
    /// 或每 T 秒更新一次（即使写操作不足 N 条）
    pub every_secs: u64,
}

impl Default for BaselinePolicy {
    fn default() -> Self {
        // 保守默认：10 条写操作或 15 分钟。用户改配置通常一次改几条，
        // 15 分钟能保证"改完一段时间内"总有基线。
        Self {
            every_n_writes: 10,
            every_secs: 900,
        }
    }
}

/// 单个会话的基线记账状态。
///
/// 全字段零值即为正确初值（未建立、无计时、未打包），故 `Default` 可 derive。
#[derive(Debug, Default)]
pub struct BaselineState {
    /// 自上次拍基线以来的写操作计数
    writes_since: u32,
    /// 上次拍基线的时间（None = 从未拍过）
    last_at: Option<Instant>,
    /// 是否已建立过基线（首次写操作必须立即拍）
    established: bool,
    /// 是否正在打包（避免并发重复拍）
    packing: bool,
}



/// 判定结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselineVerdict {
    /// 无需动作（只读命令，或还没到阈值）
    Idle,
    /// **应当立即拍基线**（首次写操作，或已达阈值）
    Capture,
    /// 正在打包中，跳过
    Busy,
}

impl BaselineState {
    #[allow(dead_code)] // 测试与调试用
    pub fn new() -> Self {
        Self::default()
    }

    /// 打包完成回调（成功或失败都调用 —— 失败也要重置计时，
    /// 否则每次写操作都会重试打包，把磁盘打满）
    pub fn finish_capture(&mut self) {
        self.packing = false;
        self.established = true;
        self.writes_since = 0;
        self.last_at = Some(Instant::now());
    }

    /// 记录一条**已执行完毕**的命令，返回是否该拍基线。
    ///
    /// `readonly` 由调用方用 `safety::is_readonly_command` 判定后传入
    /// （本模块不依赖 safety，便于单测）。
    pub fn observe(&mut self, readonly: bool, policy: BaselinePolicy) -> BaselineVerdict {
        if readonly {
            // 只读命令不计入、也不触发 —— 这正是"弹性"的体现
            return BaselineVerdict::Idle;
        }
        if self.packing {
            return BaselineVerdict::Busy;
        }
        // **首次写操作立即拍**：否则"改了 10 个文件后才拍"，
        // 前 9 个文件的原始状态已永久丢失。
        if !self.established {
            self.packing = true;
            return BaselineVerdict::Capture;
        }
        self.writes_since += 1;
        // 阈值一：写操作条数
        if self.writes_since >= policy.every_n_writes {
            self.packing = true;
            return BaselineVerdict::Capture;
        }
        // 阈值二：时间（即使写操作不足 N 条，也要保证"最多丢 T 分钟"）
        if let Some(t) = self.last_at {
            if t.elapsed() >= Duration::from_secs(policy.every_secs) {
                self.packing = true;
                return BaselineVerdict::Capture;
            }
        }
        BaselineVerdict::Idle
    }

    /// 当前记账（供展示/调试）
    #[allow(dead_code)]
    pub fn writes_since(&self) -> u32 {
        self.writes_since
    }
    #[allow(dead_code)] // 管理器层有同名方法（实际被调用的是那个）
    pub fn established(&self) -> bool {
        self.established
    }
}

/// 全局基线状态（每会话一份）
#[derive(Default)]
pub struct BaselineManager {
    states: HashMap<String, BaselineState>,
}

impl BaselineManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一条命令，返回该会话的判定
    pub fn observe(&mut self, session: &str, readonly: bool, policy: BaselinePolicy) -> BaselineVerdict {
        let st = self.states.entry(session.to_string()).or_default();
        st.observe(readonly, policy)
    }

    /// 打包结束回调
    pub fn finish_capture(&mut self, session: &str) {
        if let Some(st) = self.states.get_mut(session) {
            st.finish_capture();
        }
    }

    /// 会话断开时清理（重连后重新建立基线）
    #[allow(dead_code)]
    pub fn forget(&mut self, session: &str) {
        self.states.remove(session);
    }

    /// 查询某会话是否已建立基线
    pub fn established(&self, session: &str) -> bool {
        self.states.get(session).map(|s| s.established).unwrap_or(false)
    }
}

// ---------- 单测 ----------

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> BaselinePolicy {
        BaselinePolicy {
            every_n_writes: 3,
            every_secs: 3600, // 时间阈值设很大，避免干扰条数测试
        }
    }

    #[test]
    fn readonly_never_triggers() {
        // 用户明确要求："如果都是查询肯定没必要"
        let mut st = BaselineState::new();
        for _ in 0..100 {
            assert_eq!(st.observe(true, policy()), BaselineVerdict::Idle);
        }
        assert!(!st.established(), "100 条只读命令不应建立基线");
    }

    #[test]
    fn first_write_captures_immediately() {
        // 关键：否则"改了 10 个文件后才拍",前 9 个的原始状态永久丢失
        let mut st = BaselineState::new();
        assert_eq!(st.observe(false, policy()), BaselineVerdict::Capture);
    }

    #[test]
    fn subsequent_writes_wait_for_threshold() {
        let mut st = BaselineState::new();
        st.observe(false, policy()); // 首次 → Capture
        st.finish_capture();
        assert_eq!(st.observe(false, policy()), BaselineVerdict::Idle); // 1
        assert_eq!(st.observe(false, policy()), BaselineVerdict::Idle); // 2
        assert_eq!(st.observe(false, policy()), BaselineVerdict::Capture); // 3 → 达阈值
    }

    #[test]
    fn readonly_does_not_count_toward_threshold() {
        let mut st = BaselineState::new();
        st.observe(false, policy());
        st.finish_capture();
        // 夹 100 条只读命令,不应推进计数
        for _ in 0..100 {
            st.observe(true, policy());
        }
        assert_eq!(st.writes_since(), 0, "只读命令不得计入写操作数");
        assert_eq!(st.observe(false, policy()), BaselineVerdict::Idle);
        assert_eq!(st.writes_since(), 1);
    }

    #[test]
    fn time_threshold_triggers_even_with_few_writes() {
        let mut st = BaselineState::new();
        let p = BaselinePolicy {
            every_n_writes: 1000,
            every_secs: 0, // 立即到期
        };
        st.observe(false, p);
        st.finish_capture();
        // 仅 1 条写操作,但时间已到 → 应拍
        assert_eq!(st.observe(false, p), BaselineVerdict::Capture);
    }

    #[test]
    fn busy_while_packing() {
        // 打包期间不得重复触发（否则会把磁盘打满）
        let mut st = BaselineState::new();
        assert_eq!(st.observe(false, policy()), BaselineVerdict::Capture);
        assert_eq!(st.observe(false, policy()), BaselineVerdict::Busy);
        assert_eq!(st.observe(false, policy()), BaselineVerdict::Busy);
    }

    #[test]
    fn finish_resets_counter_and_clears_busy() {
        let mut st = BaselineState::new();
        st.observe(false, policy());
        st.observe(false, policy()); // 1
        st.finish_capture();
        assert_eq!(st.writes_since(), 0, "打包完成后计数须清零");
        assert!(st.established());
        // 不再是 Busy
        assert_ne!(st.observe(false, policy()), BaselineVerdict::Busy);
    }

    #[test]
    fn manager_isolates_sessions() {
        let mut m = BaselineManager::new();
        assert_eq!(m.observe("a", false, policy()), BaselineVerdict::Capture);
        m.finish_capture("a");
        // b 是独立会话,首次写操作也应立即拍
        assert_eq!(m.observe("b", false, policy()), BaselineVerdict::Capture);
        assert!(m.established("a"));
        assert!(!m.established("b"), "b 尚未 finish");
    }

    #[test]
    fn forget_resets_session() {
        // 断开重连后应重新建立基线(旧基线可能已过时)
        let mut m = BaselineManager::new();
        m.observe("a", false, policy());
        m.finish_capture("a");
        assert!(m.established("a"));
        m.forget("a");
        assert!(!m.established("a"));
        assert_eq!(m.observe("a", false, policy()), BaselineVerdict::Capture);
    }

    #[test]
    fn default_policy_is_reasonable() {
        // 默认策略必须"不频繁"也不"太稀疏"
        let p = BaselinePolicy::default();
        assert!(p.every_n_writes >= 5, "太频繁会拖慢终端");
        assert!(p.every_n_writes <= 50, "太稀疏会丢太多改动");
        assert!(p.every_secs >= 300, "太频繁会拖慢终端");
        assert!(p.every_secs <= 3600, "太稀疏会丢太多改动");
    }

    #[test]
    fn capture_then_immediate_write_is_idle_not_capture() {
        // 回归:finish 后立刻再来一条写命令,不应再次 Capture
        // (否则每次写操作都拍,磁盘会被打满)
        let mut st = BaselineState::new();
        st.observe(false, policy());
        st.finish_capture();
        assert_eq!(st.observe(false, policy()), BaselineVerdict::Idle);
    }
}
