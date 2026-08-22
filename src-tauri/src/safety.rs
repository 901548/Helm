// ============================================================================
// safety.rs - 命令安全检查模块
// 在执行命令前对命令文本做 shell 感知的分词 + 语义判定：
//   Critical - 极其危险，可能造成数据丢失或系统崩溃，必须用户确认
//   Warning  - 有一定风险，建议用户确认
//   Safe     - 无风险，直接执行
// 相比旧版"子串正则"，本实现可覆盖变体写法（rm -r -f /、rm -rf -- /、
// rm -rfv /、sudo rm -rf / 等），且 `echo 'rm -rf /'` 这类"被引号整体包裹
// 的参数"不会误判。判定规则：先把命令按 ; && || | 换行切成段，再对每段
// 做引号感知分词得到 argv 令牌；任一段 Critical → 整体 Critical，否则任一
// 段 Warning → Warning。
// ============================================================================

use regex::Regex;
use std::sync::OnceLock;

/// 命令危险等级
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum DangerLevel {
    /// 无风险
    Safe,
    /// 有风险，建议确认
    Warning,
    /// 极其危险，必须确认
    Critical,
}

/// fork 炸弹形变（`:(){...}` / `: ( ) { ... }`），保留正则兜底
fn fork_bomb_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r":\s*\(\s*\)\s*\{").unwrap())
}

/// 按 shell 元符号（; 换行 && || |）在引号感知下把命令切成若干独立命令段。
/// 引号内的分隔符不切分；产生的空段会被剔除。
pub fn split_commands(cmd: &str) -> Vec<String> {
    let mut segs = Vec::new();
    let mut cur = String::new();
    let mut chars = cmd.chars().peekable();
    let (mut in_sq, mut in_dq) = (false, false);
    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_dq => in_sq = !in_sq,
            '"' if !in_sq => in_dq = !in_dq,
            '\\' if !in_sq => {
                // 保留反斜杠+下一字符进入段文本,后续分词时去转义
                if let Some(&n) = chars.peek() {
                    cur.push(c);
                    cur.push(n);
                    chars.next();
                    continue;
                }
            }
            ';' | '\n' if !in_sq && !in_dq => {
                segs.push(cur.trim().to_string());
                cur.clear();
                continue;
            }
            '&' | '|' if !in_sq && !in_dq => {
                // 合并 `&&`/`||`;单 `&`/`|` 也作为分段(后台/管道各成员独立判级)
                if chars.peek() == Some(&c) {
                    chars.next();
                }
                segs.push(cur.trim().to_string());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    segs.push(cur.trim().to_string());
    segs.retain(|s| !s.is_empty());
    segs
}

/// 对单个命令段做 shell 分词,返回 argv 令牌列表。
/// 引号内保留空格(作为单个令牌);引号与转义在令牌内被去除。
pub fn tokenize_segment(seg: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut chars = seg.chars().peekable();
    let (mut in_sq, mut in_dq) = (false, false);
    let mut has = false;
    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_dq => {
                in_sq = !in_sq;
                has = true;
            }
            '"' if !in_sq => {
                in_dq = !in_dq;
                has = true;
            }
            '\\' if !in_sq => {
                if let Some(&n) = chars.peek() {
                    cur.push(n);
                    chars.next();
                    has = true;
                }
            }
            c if !in_sq && !in_dq && (c == ' ' || c == '\t') => {
                if has {
                    tokens.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            _ => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        tokens.push(cur);
    }
    tokens
}

/// 判断令牌是否为 rm 的危险参数（递归 + 强制）
struct RmFlags {
    recursive: bool,
    force: bool,
}

fn rm_flags(tokens: &[String]) -> Option<RmFlags> {
    let mut flags = RmFlags { recursive: false, force: false };
    let mut positional_only = false;
    for tok in tokens {
        if positional_only {
            continue;
        }
        if tok == "--" {
            positional_only = true;
            continue;
        }
        if let Some(long) = tok.strip_prefix("--") {
            // 长选项:必须先剥 "--",再比较名称
            if long == "recursive" {
                flags.recursive = true;
            } else if long == "force" {
                flags.force = true;
            }
        } else if let Some(short) = tok.strip_prefix('-') {
            // 短选项组合（如 -rf / -rfv / -fr），r 与 f 必须各自独立判定,
                // 不能写成 if/else-if 链（否则 -rf 只识别出 r 而漏掉 f）。
                if short.chars().any(|c| c == 'r' || c == 'R') {
                    flags.recursive = true;
                }
                if short.chars().any(|c| c == 'f') {
                    flags.force = true;
                }
        }
    }
    if flags.recursive && flags.force {
        Some(flags)
    } else {
        None
    }
}

/// 目标是否为"毁灭性根目标":`/` 本身或 `/*` `/**` 形式的直接清根。
/// 普通绝对路径(如 /tmp/a)不算——文件面板删除恒用绝对路径,
/// 若误判为 Critical 会拦截正常删除操作(P45 修复的 P29 回归)。
fn has_root_target(tokens: &[String]) -> bool {
    tokens.iter().any(|t| {
        t == "/" || (t.starts_with("/*") && t.chars().all(|c| c == '/' || c == '*'))
    })
}

/// 常见命令包装器：`sudo shutdown` / `env VAR=x cmd` / `nohup cmd &` 等。
/// 判定"命令名令牌"时跳过这些前缀，避免把包装器误当命令。
const COMMAND_WRAPPERS: [&str; 8] = ["sudo", "env", "command", "time", "nohup", "nice", "setsid", "stdbuf"];

/// 取段内真正的命令名（跳过前置包装器与前导选项令牌）。返回空串表示拿不到。
/// 仅用于 shutdown/reboot/pkill 等"以完整命令出现才告警"的规则，
/// 避免 `echo shutdown` 之类的文本被误判（旧版子串正则存在此误报）。
fn first_command(tokens: &[String]) -> &str {
    tokens
        .iter()
        .find(|t| !t.starts_with('-') && !COMMAND_WRAPPERS.contains(&t.as_str()))
        .map(|s| s.as_str())
        .unwrap_or("")
}

/// 单个命令段的危险等级
fn check_segment(seg: &str) -> (DangerLevel, &'static str) {
    if fork_bomb_re().is_match(seg) {
        return (DangerLevel::Critical, "Fork 炸弹");
    }
    let tokens = tokenize_segment(seg);
    if tokens.is_empty() {
        return (DangerLevel::Safe, "命令安全");
    }
    check_tokens(&tokens)
}

/// token 级危险判定(供段分析与 xargs 管道合并分析复用)
fn check_tokens(tokens: &[String]) -> (DangerLevel, &'static str) {
    // Critical：rm -rf 指向根路径
    if tokens.iter().any(|t| t == "rm") && rm_flags(&tokens).is_some() {
        if has_root_target(&tokens) {
            return (DangerLevel::Critical, "删除根目录 (rm -rf /)");
        }
        return (DangerLevel::Warning, "递归删除文件 (rm -rf)");
    }
    // Critical：chmod 777 根路径 / shutdown / reboot / mkfs / dd 写磁盘
    if tokens.iter().any(|t| t == "chmod") {
        let has777 = tokens.iter().any(|t| t.contains("777"));
        if has777 && has_root_target(&tokens) {
            return (DangerLevel::Critical, "根目录设置为 777 权限");
        }
        if has777 {
            return (DangerLevel::Warning, "设置 777 权限");
        }
    }
    // 关停/进程/分区类:以独立命令出现才告警(带包装器),`echo shutdown` 不误报
    match first_command(&tokens) {
        "shutdown" => return (DangerLevel::Critical, "关闭系统 (shutdown)"),
        "reboot" => return (DangerLevel::Critical, "重启系统 (reboot)"),
        "pkill" => return (DangerLevel::Warning, "终止进程 (pkill)"),
        "killall" => return (DangerLevel::Warning, "终止进程 (killall)"),
        "fdisk" => return (DangerLevel::Warning, "磁盘分区操作 (fdisk)"),
        "parted" => return (DangerLevel::Warning, "磁盘分区操作 (parted)"),
        _ => {}
    }
    if tokens.iter().any(|t| t.starts_with("mkfs")) {
        return (DangerLevel::Critical, "格式化磁盘 (mkfs)");
    }
    if tokens.iter().any(|t| t == "dd")
        && tokens.iter().any(|t| {
            t.starts_with("of=/dev/sd")
                || t.starts_with("of=/dev/nvme")
                || t.starts_with("of=/dev/mapper")
                || t.starts_with("of=/dev/vd")
                || t.starts_with("of=/dev/hd")
        })
    {
        return (DangerLevel::Critical, "直接写入磁盘 (dd 到 /dev/*)");
    }

    (DangerLevel::Safe, "命令安全")
}

/// xargs 需要提级合并分析的危险命令(目标可能来自管道 stdin)
const XARGS_DANGER_CMDS: [&str; 6] = ["rm", "chmod", "dd", "mkfs", "shutdown", "reboot"];

/// token 归一化后是否为毁灭性根目标(剥掉 `$()` / 引号残留字符)
fn token_is_root_wipe(t: &str) -> bool {
    // 只剥包装标点($、括号、引号、分号等),不剥字母数字——
    // 否则 $(pwd)/build 会被剥成 "/" 误判根
    let norm = t.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '/' && c != '*');
    norm == "/" || (norm.starts_with("/*") && norm.chars().all(|c| c == '/' || c == '*'))
}

/// 检查命令的危险等级，返回 (等级, 中文原因说明)
pub fn check_danger(cmd: &str) -> (DangerLevel, String) {
    let segs = split_commands(cmd);
    // 二次执行防护 1:命令替换——危险命令的 $(...) 内出现根目标即升级
    // (`rm -rf $(echo /)` 分词后看不到完整目标,替换体按保守原则判)
    for seg in &segs {
        if !seg.contains("$(") && !seg.contains('`') {
            continue;
        }
        let tokens = tokenize_segment(seg);
        let dangerous = (tokens.iter().any(|t| t == "rm") && rm_flags(&tokens).is_some())
            || tokens.iter().any(|t| t.starts_with("mkfs"));
        if dangerous && tokens.iter().any(|t| token_is_root_wipe(t)) {
            return (DangerLevel::Critical, "命令替换中出现根目标 (rm -rf $(... /))".to_string());
        }
    }
    // 二次执行防护 2:xargs 管道——目标经 stdin 传入,分段后不可见;
    // 若 xargs 段本身含危险命令,合并整条管道的 token 分析
    // (`echo / | xargs rm -rf` → 合并后见 "/" → Critical)
    for seg in &segs {
        let tokens = tokenize_segment(seg);
        let has_xargs = tokens.iter().any(|t| t == "xargs");
        let has_danger = XARGS_DANGER_CMDS
            .iter()
            .any(|c| tokens.iter().any(|t| t.starts_with(c)));
        if has_xargs && has_danger {
            let merged = tokenize_segment(&cmd.replace(['|', ';', '\n'], " "));
            let (level, reason) = check_tokens(&merged);
            if level == DangerLevel::Critical {
                return (level, reason.to_string());
            }
        }
    }
    for seg in &segs {
        let (level, reason) = check_segment(seg);
        if level == DangerLevel::Critical {
            return (DangerLevel::Critical, reason.to_string());
        }
    }
    for seg in &segs {
        let (level, reason) = check_segment(seg);
        if level == DangerLevel::Warning {
            return (DangerLevel::Warning, reason.to_string());
        }
    }
    (DangerLevel::Safe, "命令安全".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is(level: DangerLevel, cmd: &str) {
        assert_eq!(check_danger(cmd).0, level, "命令: {cmd}");
    }

    #[test]
    fn critical_rm_variants() {
        is(DangerLevel::Critical, "rm -rf /");
        is(DangerLevel::Critical, "rm -r -f /");
        is(DangerLevel::Critical, "rm -rf -- /");
        is(DangerLevel::Critical, "rm -rf /*");
        is(DangerLevel::Critical, "rm -rfv /");
        is(DangerLevel::Critical, "sudo rm -rf /");
        is(DangerLevel::Critical, "rm -rf /; echo ok");
        is(DangerLevel::Critical, "rm -rf / && reboot");
        // 长选项变体(曾因 strip_prefix 少剥一个 '-' 永假漏检)
        is(DangerLevel::Critical, "rm --recursive --force /");
        is(DangerLevel::Critical, "rm --recursive -f /*");
        is(DangerLevel::Critical, "rm -r --force /");
    }

    #[test]
    fn warning_rm_variants() {
        is(DangerLevel::Warning, "rm -rf ./backup.sh");
        is(DangerLevel::Warning, "rm -rf dir");
        // 普通绝对路径不是清根:回落 Warning(P45 语义修正,
        // 文件面板删除恒用绝对路径,误判 Critical 会拦截正常删除)
        is(DangerLevel::Warning, "rm -rf /tmp/a");
        is(DangerLevel::Warning, "rm -rf /tmp/helm_smoke_test");
        is(DangerLevel::Warning, "rm --recursive --force dir");
    }

    #[test]
    fn safe_rm_and_echo_commands() {
        is(DangerLevel::Safe, "rm file");
        is(DangerLevel::Safe, "rm -r dir");
        is(DangerLevel::Safe, "echo 'rm -rf /'");
        is(DangerLevel::Safe, "echo shutdown");
    }

    #[test]
    fn xargs_secondary_execution() {
        // 管道上游的根目标经 stdin 传给危险命令 → 合并分析升级
        is(DangerLevel::Critical, "echo / | xargs rm -rf");
        is(DangerLevel::Critical, "echo /* | xargs rm -rf");
        // 非根目标仍是 Warning(日常清理模式)
        is(DangerLevel::Warning, "echo /tmp/a | xargs rm -rf");
        is(DangerLevel::Warning, "find /var/log -name '*.gz' | xargs rm -rf");
        // xargs 段无危险命令不提级
        is(DangerLevel::Safe, "echo x | xargs cat");
    }

    #[test]
    fn command_substitution_root() {
        // 危险命令的替换体内出现根目标 → Critical
        is(DangerLevel::Critical, "rm -rf $(echo /)");
        is(DangerLevel::Critical, "rm -rf $(echo /*)");
        // 替换体非根目标 → 维持 Warning
        is(DangerLevel::Warning, "rm -rf $(echo /tmp/a)");
        is(DangerLevel::Warning, "rm -rf $(pwd)/build");
        // 非危险命令带替换不升级
        is(DangerLevel::Safe, "echo $(ls /)");
    }

    #[test]
    fn system_shutdown_commands() {
        is(DangerLevel::Critical, "shutdown -h now");
        is(DangerLevel::Critical, "reboot");
        is(DangerLevel::Safe, "echo reboot now");
    }

    #[test]
    fn dd_disk_commands() {
        is(DangerLevel::Critical, "dd if=/dev/zero of=/dev/sda bs=1M");
        is(DangerLevel::Critical, "dd if=/dev/zero of=/dev/mapper/root bs=1M");
        is(DangerLevel::Safe, "dd if=/dev/x of=/tmp/out");
    }

    #[test]
    fn chmod_commands() {
        is(DangerLevel::Critical, "chmod 777 /");
        is(DangerLevel::Critical, "chmod 777 /*");
        // 普通绝对路径非清根(P45 语义修正)
        is(DangerLevel::Warning, "chmod 777 /etc");
        is(DangerLevel::Warning, "chmod 777 file");
        is(DangerLevel::Warning, "chmod 0777 server");
        is(DangerLevel::Safe, "chmod 755 file");
    }

    #[test]
    fn fork_bomb_variants() {
        is(DangerLevel::Critical, ":(){ :|:& };:");
        is(DangerLevel::Critical, ": ( ) { :|:& };:");
    }

    #[test]
    fn process_tools() {
        is(DangerLevel::Warning, "pkill sshd");
        is(DangerLevel::Warning, "killall nginx");
        is(DangerLevel::Warning, "fdisk -l");
        is(DangerLevel::Warning, "parted /dev/sda print");
        is(DangerLevel::Safe, "echo pkill");
    }

    #[test]
    fn mkfs_command() {
        is(DangerLevel::Critical, "mkfs.ext4 /dev/sdb1");
    }

    #[test]
    fn split_respects_quotes() {
        let segs = split_commands("echo 'a;b' && ls");
        assert_eq!(segs, vec!["echo 'a;b'".to_string(), "ls".to_string()]);
    }
}
