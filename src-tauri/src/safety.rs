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

/// 取令牌的 basename（最后一个 `/` 之后的部分）：`/bin/rm` → `rm`。
/// 危险命令用绝对路径（`/bin/rm -rf /`、`/sbin/shutdown`）时，精确词元比较会漏检，
/// 归一化到裸命令名后统一判级。
fn basename(t: &str) -> &str {
    t.rsplit('/').next().unwrap_or(t)
}

/// 取段内真正的命令名（跳过前置包装器与前导选项令牌，并归一化 basename）。
/// 返回空串表示拿不到。仅用于 shutdown/reboot/pkill 等"以完整命令出现才告警"的规则，
/// 避免 `echo shutdown` 之类的文本被误判（旧版子串正则存在此误报）。
fn first_command(tokens: &[String]) -> &str {
    tokens
        .iter()
        .find(|t| !t.starts_with('-') && !COMMAND_WRAPPERS.contains(&t.as_str()))
        .map(|s| basename(s.as_str()))
        .unwrap_or("")
}

/// 命令名（first_command，含 basename 归一化）或任一令牌**精确**等于 name。
/// 「任一精确匹配」是给 xargs 合并分析用的（危险命令出现在 xargs 之后、非命令位）；
/// 只对命令名做 basename，避免把 `tar -rf archive /etc/rm`、`ls /usr/sbin/mkfs.ext4`
/// 这类「路径参数恰好叫 rm/mkfs」误判成危险命令。
fn cmd_is(tokens: &[String], name: &str) -> bool {
    first_command(tokens) == name || tokens.iter().any(|t| t == name)
}

/// 命令名或任一令牌前缀匹配 prefix（mkfs 系列：mkfs.ext4/mkfs.xfs...）
fn cmd_starts(tokens: &[String], prefix: &str) -> bool {
    first_command(tokens).starts_with(prefix) || tokens.iter().any(|t| t.starts_with(prefix))
}

/// 判断命令名（basename）是否为"执行器"：`sh -c`/`eval`/`python -c`/`perl -e` 等
/// 会把后续参数字符串**真正执行**（不同于 `echo 'rm -rf /'` 只打印文本）。
/// 这类包装器若不递归判级，危险命令会借壳绕过安全护栏。
fn is_executor(name: &str) -> bool {
    is_shell_executor(name) || is_script_executor(name)
}

/// shell 执行器：其 `-c`/eval 参数是 shell 代码，可递归进既有 shell 判级
fn is_shell_executor(name: &str) -> bool {
    matches!(
        basename(name),
        "sh" | "bash" | "dash" | "zsh" | "ksh" | "fish" | "csh" | "tcsh" | "eval"
    )
}

/// 脚本语言执行器：其 `-c`/`-e` 参数是 python/perl/... 代码，非 shell，
/// 无法完整静态解析，改用破坏性关键字保守扫描
fn is_script_executor(name: &str) -> bool {
    matches!(
        basename(name),
        "python" | "python2" | "python3" | "perl" | "ruby" | "node" | "php"
    )
}

/// 定位执行器命令名，返回 (token 下标, basename)。无执行器返回 None。
/// 语义与 first_command 一致：只认「第一个非包装器/非前导选项」的命令名——
/// 绝不扫描参数位置，否则 `echo sh -c 'rm -rf /'` 会把参数里的 `sh` 当执行器误判。
fn executor_pos(tokens: &[String]) -> Option<(usize, &str)> {
    let idx = tokens
        .iter()
        .position(|t| !t.starts_with('-') && !COMMAND_WRAPPERS.contains(&t.as_str()))?;
    let name = basename(&tokens[idx]);
    if is_executor(name) {
        Some((idx, name))
    } else {
        None
    }
}

/// 提取执行器真正要执行的代码串；不是执行器包装时返回 None。
/// - `sh -c 'CODE'` / `python -c 'CODE'`：返回 CODE（独立或粘连 `-cCODE` 形式均可）
/// - `eval CODE...`：eval 无 `-c` 标记，返回其后全部参数按空格连接
fn executor_code(tokens: &[String]) -> Option<String> {
    let (idx, name) = executor_pos(tokens)?;
    if name == "eval" {
        let rest: Vec<&str> = tokens[idx + 1..].iter().map(|s| s.as_str()).collect();
        if rest.is_empty() {
            return None;
        }
        return Some(rest.join(" "));
    }
    for i in idx + 1..tokens.len() {
        let t = tokens[i].as_str();
        if matches!(t, "-c" | "-e" | "-E" | "-r") {
            return tokens.get(i + 1).filter(|s| !s.is_empty()).cloned();
        }
        if (t.starts_with("-c") || t.starts_with("-e") || t.starts_with("-E") || t.starts_with("-r")) && t.len() > 2 {
            return Some(t[2..].to_string());
        }
    }
    None
}

/// 脚本语言代码串的破坏性关键字保守扫描（大小写不敏感）。
/// 只扫「明确毁灭性」标记，避免把 `print("shutdown")` 这类文本误判；
/// 定位的是 `os.system("rm -rf /")` / `subprocess` 等 shell-out 破坏面。
fn contains_destructive_marker(code: &str) -> bool {
    let lower = code.to_lowercase();
    for m in [
        "rm -rf", "rm -fr", "rm -r -f", "rm --recursive --force",
        "shutdown", "reboot", "mkfs", "of=/dev/",
    ] {
        if lower.contains(m) {
            return true;
        }
    }
    false
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
    // Critical：rm -rf 指向根路径（命令名按 basename 归一化，覆盖 /bin/rm 等绝对路径）
    if cmd_is(tokens, "rm") && rm_flags(tokens).is_some() {
        if has_root_target(tokens) {
            return (DangerLevel::Critical, "删除根目录 (rm -rf /)");
        }
        return (DangerLevel::Warning, "递归删除文件 (rm -rf)");
    }
    // Critical：chmod 777 根路径 / shutdown / reboot / mkfs / dd 写磁盘
    if cmd_is(tokens, "chmod") {
        let has777 = tokens.iter().any(|t| t.contains("777"));
        if has777 && has_root_target(tokens) {
            return (DangerLevel::Critical, "根目录设置为 777 权限");
        }
        if has777 {
            return (DangerLevel::Warning, "设置 777 权限");
        }
    }
    // 关停/进程/分区类:以独立命令出现才告警(带包装器),`echo shutdown` 不误报
    match first_command(tokens) {
        "shutdown" => return (DangerLevel::Critical, "关闭系统 (shutdown)"),
        "reboot" => return (DangerLevel::Critical, "重启系统 (reboot)"),
        "pkill" => return (DangerLevel::Warning, "终止进程 (pkill)"),
        "killall" => return (DangerLevel::Warning, "终止进程 (killall)"),
        "fdisk" => return (DangerLevel::Warning, "磁盘分区操作 (fdisk)"),
        "parted" => return (DangerLevel::Warning, "磁盘分区操作 (parted)"),
        _ => {}
    }
    if cmd_starts(tokens, "mkfs") {
        return (DangerLevel::Critical, "格式化磁盘 (mkfs)");
    }
    if cmd_is(tokens, "dd")
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
    check_danger_inner(cmd, 0)
}

/// 递归判级：执行器包装（sh -c/eval/python -c 等）里的代码串要借壳递归，
/// 深度上限 8 防 `eval eval ...` 自嵌套（每层至少剥掉一层执行器，输入必然缩短）。
fn check_danger_inner(cmd: &str, depth: u8) -> (DangerLevel, String) {
    if depth > 8 {
        return (DangerLevel::Safe, "命令安全".to_string());
    }
    let segs = split_commands(cmd);
    // 二次执行防护 1:命令替换——危险命令的 $(...) 内出现根目标即升级
    // (`rm -rf $(echo /)` 分词后看不到完整目标,替换体按保守原则判)
    for seg in &segs {
        if !seg.contains("$(") && !seg.contains('`') {
            continue;
        }
        let tokens = tokenize_segment(seg);
        let dangerous = (cmd_is(&tokens, "rm") && rm_flags(&tokens).is_some())
            || cmd_starts(&tokens, "mkfs");
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
            .any(|c| tokens.iter().any(|t| basename(t).starts_with(c)));
        if has_xargs && has_danger {
            let merged = tokenize_segment(&cmd.replace(['|', ';', '\n'], " "));
            let (level, reason) = check_tokens(&merged);
            if level == DangerLevel::Critical {
                return (level, reason.to_string());
            }
        }
    }
    // 二次执行防护 3:执行器包装——`sh -c 'rm -rf /'`/`eval 'rm -rf /'`/
    // `python -c 'import os;os.system("rm -rf /")'` 的参数字符串会被真正执行。
    // shell 执行器(sh/bash/eval)递归判级代码串;脚本语言执行器(python/perl/...)做破坏性关键字扫描(P90)。
    for seg in &segs {
        let tokens = tokenize_segment(seg);
        let Some(code) = executor_code(&tokens) else { continue };
        let is_script = executor_pos(&tokens)
            .map(|(_, n)| is_script_executor(n))
            .unwrap_or(false);
        let crit = if is_script {
            contains_destructive_marker(&code)
        } else {
            check_danger_inner(&code, depth + 1).0 == DangerLevel::Critical
        };
        if crit {
            return (DangerLevel::Critical, "执行器包装的命令含危险操作".to_string());
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
    // 执行器包装内的 Warning(如 `sh -c 'rm -rf /tmp'`)也要提级,不能只透出 Safe
    for seg in &segs {
        let tokens = tokenize_segment(seg);
        let Some(code) = executor_code(&tokens) else { continue };
        let is_script = executor_pos(&tokens)
            .map(|(_, n)| is_script_executor(n))
            .unwrap_or(false);
        // 脚本语言执行器只做 Critical 关键字扫描,不做 Warning 提级(避免 `print("rm -rf")` 误报)
        if !is_script && check_danger_inner(&code, depth + 1).0 == DangerLevel::Warning {
            return (DangerLevel::Warning, "执行器包装的命令含风险操作".to_string());
        }
    }
    (DangerLevel::Safe, "命令安全".to_string())
}

// ============================================================================
// P110 QA 只读命令门禁：问答模式允许模型执行「查看/检索」类命令辅助回答，
// 一切修改类命令一律拒绝。与 check_danger 的"危险分级"互补——
// check_danger 只识别破坏性命令，touch/mkdir/yum install 等修改类命令是 Safe，
// 不能仅靠它保证"只读"，故用白名单 + 逃逸通道封堵双保险。
//
// P111 重构：白名单与逃逸守卫合并为**单一规格表**（命令, 守卫）——
// 旧版两张表分维护，首版就漏了 8 个命令本体（单测逐个抓出来的漂移）。
// ============================================================================

/// QA 只读命令的逃逸守卫：该命令在白名单内，但其某些参数/子命令会改系统，逐一封堵。
#[derive(Clone, Copy, PartialEq, Eq)]
enum RoGuard {
    /// 纯查看，无参数逃逸口
    Plain,
    /// find：-delete/-exec/-ok 系会删会执行
    Find,
    /// sed：-i/--in-place 就地写
    Sed,
    /// awk：system()/getline/协程/文件重定向
    Awk,
    /// git：只放纯读子命令
    Git,
    /// docker：只放纯读子命令
    Docker,
    /// systemctl：修改子命令黑名单（status 前可插 --no-pager，查 tokens[1] 会漏）
    Systemctl,
    /// ip：修改子命令黑名单
    Ip,
    /// nmcli：连接的启停/增删/网络开关黑名单
    Nmcli,
    /// env：无参数才是读，env CMD 会执行 CMD
    EnvNoArgs,
    /// date：-s/--set 改系统时钟
    Date,
    /// hostname：带非旗标参数=改主机名
    Hostname,
    /// hostnamectl：set-* 子命令改机器标识
    HostnameCtl,
    /// sort：-o/--output 写文件
    Sort,
    /// dmesg：-c 清缓冲、-w 跟随、--console-* 改控制台日志
    Dmesg,
    /// journalctl：-f 跟随、--rotate/--vacuum-* 删日志、--flush
    Journalctl,
    /// tail：-f/--follow 永不退出（占满 60s 步骤超时）
    Tail,
    /// ping：必须带 -c 次数（否则永不退出）
    Ping,
    /// top：必须 -b -n（批量模式，否则交互挂起）
    Top,
}

/// 单一规格表：白名单命令 + 各自守卫。**新增命令只改这里一行**（P110 漂移教训）。
const READONLY_SPECS: &[(&str, RoGuard)] = &[
    ("df", RoGuard::Plain),
    ("free", RoGuard::Plain),
    ("ps", RoGuard::Plain),
    ("ls", RoGuard::Plain),
    ("cat", RoGuard::Plain),
    ("head", RoGuard::Plain),
    ("tail", RoGuard::Tail),
    ("grep", RoGuard::Plain),
    ("egrep", RoGuard::Plain),
    ("fgrep", RoGuard::Plain),
    ("du", RoGuard::Plain),
    ("wc", RoGuard::Plain),
    ("sort", RoGuard::Sort),
    ("uniq", RoGuard::Plain),
    ("cut", RoGuard::Plain),
    ("tr", RoGuard::Plain),
    ("column", RoGuard::Plain),
    ("comm", RoGuard::Plain),
    ("nl", RoGuard::Plain),
    ("tac", RoGuard::Plain),
    ("rev", RoGuard::Plain),
    ("stat", RoGuard::Plain),
    ("file", RoGuard::Plain),
    ("which", RoGuard::Plain),
    ("whereis", RoGuard::Plain),
    ("lscpu", RoGuard::Plain),
    ("lsblk", RoGuard::Plain),
    ("findmnt", RoGuard::Plain),
    ("blkid", RoGuard::Plain),
    ("uname", RoGuard::Plain),
    ("uptime", RoGuard::Plain),
    ("hostname", RoGuard::Hostname),
    ("hostnamectl", RoGuard::HostnameCtl),
    ("date", RoGuard::Date),
    ("cal", RoGuard::Plain),
    ("who", RoGuard::Plain),
    ("w", RoGuard::Plain),
    ("whoami", RoGuard::Plain),
    ("id", RoGuard::Plain),
    ("groups", RoGuard::Plain),
    ("env", RoGuard::EnvNoArgs),
    ("printenv", RoGuard::Plain),
    ("lsof", RoGuard::Plain),
    ("vmstat", RoGuard::Plain),
    ("iostat", RoGuard::Plain),
    ("mpstat", RoGuard::Plain),
    ("pidof", RoGuard::Plain),
    ("pgrep", RoGuard::Plain),
    ("top", RoGuard::Top),
    ("last", RoGuard::Plain),
    ("lastlog", RoGuard::Plain),
    ("ping", RoGuard::Ping),
    ("ss", RoGuard::Plain),
    ("netstat", RoGuard::Plain),
    ("lspci", RoGuard::Plain),
    ("lsusb", RoGuard::Plain),
    ("lsmod", RoGuard::Plain),
    ("nproc", RoGuard::Plain),
    ("nmcli", RoGuard::Nmcli),
    ("journalctl", RoGuard::Journalctl),
    ("dmesg", RoGuard::Dmesg),
    ("md5sum", RoGuard::Plain),
    ("sha1sum", RoGuard::Plain),
    ("sha256sum", RoGuard::Plain),
    ("cksum", RoGuard::Plain),
    ("base64", RoGuard::Plain),
    ("printf", RoGuard::Plain),
    ("echo", RoGuard::Plain),
    ("bc", RoGuard::Plain),
    ("seq", RoGuard::Plain),
    ("systemctl", RoGuard::Systemctl),
    ("ip", RoGuard::Ip),
    ("find", RoGuard::Find),
    ("sed", RoGuard::Sed),
    ("awk", RoGuard::Awk),
    ("git", RoGuard::Git),
    ("docker", RoGuard::Docker),
];

/// 守卫实现：true = 该次调用仍是只读
fn guard_ok(guard: RoGuard, tokens: &[String], seg: &str) -> bool {
    let has = |needle: &str| tokens.iter().any(|t| t.as_str() == needle);
    let has_prefix = |needle: &str| tokens.iter().any(|t| t.as_str().starts_with(needle));
    match guard {
        RoGuard::Plain => true,
        RoGuard::Find => !tokens.iter().any(|t| {
            matches!(
                t.as_str(),
                "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir" | "-fprintf" | "-fprint" | "-fls"
            )
        }),
        RoGuard::Sed => !has_prefix("-i") && !has("--in-place"),
        RoGuard::Awk => {
            // 比较运算符 `> 100` 合法；重定向形态是 `> "file"` / `>>`，协程是 `| "cmd"`
            !(seg.contains("system(")
                || seg.contains("getline")
                || seg.contains("|&")
                || seg.contains("> \"")
                || seg.contains(">>")
                || seg.contains("| \""))
        }
        RoGuard::Git => tokens
            .get(1)
            .map(|s| s.as_str())
            .is_some_and(|sub| matches!(sub, "status" | "log" | "diff" | "show")),
        RoGuard::Docker => tokens.get(1).map(|s| s.as_str()).is_some_and(|sub| {
            matches!(
                sub,
                "ps" | "images" | "logs" | "inspect" | "version" | "info" | "top" | "stats" | "port"
            )
        }),
        RoGuard::Systemctl => !tokens.iter().any(|t| {
            matches!(
                t.as_str(),
                "start" | "stop" | "restart" | "reload" | "try-reload" | "try-restart" | "enable"
                    | "disable" | "mask" | "unmask" | "kill" | "isolate" | "set-default" | "link"
                    | "revert" | "freeze" | "thaw"
            )
        }),
        RoGuard::Ip => !tokens.iter().any(|t| {
            matches!(
                t.as_str(),
                "add" | "del" | "delete" | "flush" | "set" | "change" | "replace" | "append"
            )
        }),
        RoGuard::Nmcli => !tokens.iter().any(|t| {
            matches!(
                t.as_str(),
                "up" | "down" | "connect" | "disconnect" | "modify" | "add" | "delete" | "remove"
                    | "reload" | "reapply" | "monitor" | "on" | "off"
            )
        }),
        RoGuard::EnvNoArgs => tokens.len() == 1,
        RoGuard::Date => !has("-s") && !has("--set"),
        RoGuard::Hostname => tokens.len() == 1 || tokens[1..].iter().all(|t| t.starts_with('-')),
        RoGuard::HostnameCtl => !tokens.iter().any(|t| t.starts_with("set-")),
        RoGuard::Sort => !has("-o") && !has_prefix("--output"),
        RoGuard::Dmesg => !tokens.iter().any(|t| {
            matches!(t.as_str(), "-c" | "--clear" | "-w" | "--follow")
                || t.starts_with("--console")
        }),
        RoGuard::Journalctl => !tokens.iter().any(|t| {
            matches!(t.as_str(), "-f" | "--follow" | "--rotate" | "--flush" | "--relinquish-var")
                || t.starts_with("--vacuum")
        }),
        RoGuard::Tail => !has("-f") && !has("--follow"),
        RoGuard::Ping => has("-c"),
        RoGuard::Top => has("-b") && has("-n"),
    }
}

/// QA 模式只读判定：true = 允许自动执行。
/// 规则：整条命令重定向/命令替换/后台与逃逸通道全禁 + 每个管道段首命令在规格表内
/// + 表内的可编程命令再过各自的逃逸守卫。
pub fn is_readonly_command(cmd: &str) -> bool {
    let c = cmd.trim();
    if c.is_empty() {
        return false;
    }
    // 先剥掉无害的 "2>&1"/"1>&2"（fd 复制不是写文件），再看是否还有重定向
    let stripped = c.replace("2>&1", "").replace("1>&2", "");
    if stripped.contains('>') || stripped.contains('<') {
        return false; // 写文件/输入重定向
    }
    if stripped.contains("$(") || stripped.contains('`') {
        return false; // 命令替换藏命令
    }
    for seg in split_commands(&stripped) {
        let seg = seg.trim();
        if seg.is_empty() {
            return false;
        }
        // 段内残留 '&'：split_commands 只切 &&/|/;/换行，剩下的单个 & 是后台执行或 &>
        if seg.contains('&') {
            return false;
        }
        let tokens = tokenize_segment(seg);
        let Some(first) = tokens.first() else { return false };
        let base = basename(first);
        let Some((_, guard)) = READONLY_SPECS.iter().find(|(name, _)| *name == base) else {
            return false;
        };
        if !guard_ok(*guard, &tokens, seg) {
            return false;
        }
    }
    true
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

    // ---- P90：执行器包装 + 绝对路径 回归（安全护栏绕过修复） ----

    #[test]
    fn absolute_path_dangerous_commands() {
        // 危险命令用绝对路径，旧精确词元比较会漏检
        is(DangerLevel::Critical, "/bin/rm -rf /");
        is(DangerLevel::Critical, "/usr/bin/rm -rf /");
        is(DangerLevel::Critical, "/sbin/shutdown -h now");
        is(DangerLevel::Critical, "/sbin/reboot");
        is(DangerLevel::Critical, "/usr/sbin/mkfs.ext4 /dev/sdb1");
        is(DangerLevel::Warning, "/bin/rm -rf /tmp/a");
        is(DangerLevel::Warning, "/bin/rm -rf ./backup");
        // 不带危险旗标的绝对路径 rm 仍是普通命令（rm_flags 需 -r/-f 同时命中）
        is(DangerLevel::Safe, "ls /bin/rm");
    }

    #[test]
    fn executor_wrapper_dangerous_commands() {
        // sh/bash -c 真正执行参数字符串（区别于 echo 只打印）
        is(DangerLevel::Critical, "sh -c 'rm -rf /'");
        is(DangerLevel::Critical, "bash -c 'rm -rf /'");
        is(DangerLevel::Critical, "sh -c \"rm -rf /\"");
        is(DangerLevel::Critical, "sh -c 'shutdown -h now'");
        is(DangerLevel::Critical, "sudo sh -c 'rm -rf /'");
        // eval 直接执行其后参数
        is(DangerLevel::Critical, "eval 'rm -rf /'");
        is(DangerLevel::Critical, "eval rm -rf /");
        // 脚本语言执行器
        is(DangerLevel::Critical, "python3 -c 'import os; os.system(\"rm -rf /\")'");
        is(DangerLevel::Critical, "perl -e 'system(\"rm -rf /\")'");
        is(DangerLevel::Critical, "python -c 'import os; os.system(\"rm -rf /\")'");
        // 执行器内的非根删除 → Warning（不是 Safe，也不能当 Critical）
        is(DangerLevel::Warning, "sh -c 'rm -rf /tmp'");
        is(DangerLevel::Warning, "bash -c 'rm -rf /tmp/a'");
        // 嵌套执行器借壳
        is(DangerLevel::Critical, "sh -c \"bash -c 'rm -rf /'\"");
        // 非执行器仍不误报（echo 只打印）
        is(DangerLevel::Safe, "echo 'rm -rf /'");
        is(DangerLevel::Safe, "echo sh -c rm");
        // 参数位置出现执行器名不算（echo sh -c 'rm -rf /' 只是打印）
        is(DangerLevel::Safe, "echo sh -c 'rm -rf /'");
    }

    #[test]
    fn executor_wrapper_non_dangerous_ok() {
        // 执行器但代码串无害 → Safe
        is(DangerLevel::Safe, "sh -c 'echo hello'");
        is(DangerLevel::Safe, "python3 -c 'print(1+1)'");
        is(DangerLevel::Safe, "bash -c 'ls -la'");
    }

    /// basename 归一化只作用于命令名位置，路径参数恰好叫 rm/mkfs/dd 不得误判
    #[test]
    fn path_argument_named_like_danger_not_misjudged() {
        is(DangerLevel::Safe, "cat /etc/rm");
        is(DangerLevel::Safe, "tar -rf archive /etc/rm");
        is(DangerLevel::Safe, "ls /usr/sbin/mkfs.ext4");
        is(DangerLevel::Safe, "cp -rf src /tmp/rm");
        is(DangerLevel::Safe, "ls /bin/dd");
        is(DangerLevel::Safe, "echo /sbin/shutdown");
    }

    /// P110 QA 只读门禁：白名单内 + 无逃逸通道 → 允许
    #[test]
    fn readonly_allows_inspection_commands() {
        for ok in [
            "df -h",
            "free -m",
            "ps aux | grep nginx",
            "systemctl status sshd",
            "systemctl --no-pager status sshd",
            "journalctl -n 50 --no-pager",
            "cat /etc/hosts",
            "tail -n 100 /var/log/messages 2>&1",
            "find /var/log -name '*.log' -mtime -7",
            "docker ps --format '{{.Names}}'",
            "docker logs --tail 50 helm-container",
            "git status",
            "git log --oneline -10",
            "ip addr",
            "ip route show",
            "awk '{print $1}' /etc/passwd",
            "sed -n '1,10p' /etc/hosts",
            "env",
            "uname -r && uptime",
            "df -h; free -m; ps aux",
            // P111 新守卫的合法形态
            "sort /etc/passwd",
            "sort -t: -k3 /etc/passwd",
            "date +%F",
            "date",
            "hostname -s",
            "hostname",
            "hostnamectl status",
            "tail -n 20 /var/log/messages",
            "dmesg | tail -n 20",
            "journalctl -u sshd -n 20",
            "ping -c 3 example.com",
            "top -b -n 1",
            "ss -tlnp",
            "netstat -rn",
            "lsmod",
            "nproc",
            "nmcli device status",
            "nmcli connection show",
        ] {
            assert!(is_readonly_command(ok), "应放行: {ok}");
        }
    }

    /// P110：修改类命令与逃逸通道全拒
    #[test]
    fn readonly_rejects_mutation_and_escapes() {
        for bad in [
            // 白名单外
            "rm -rf /tmp/x",
            "touch /tmp/x",
            "mkdir /tmp/x",
            "yum install -y nginx",
            "apt-get update",
            "pip install requests",
            "tar -xzf a.tar.gz",
            "pkill nginx",
            "kill 1234",
            "chmod +x f",
            "chown root f",
            "curl http://x",
            "wget http://x",
            "python3 -c 'print(1)'",
            // 修改子命令守卫
            "systemctl restart sshd",
            "systemctl stop nginx",
            "systemctl enable sshd",
            "git push origin main",
            "git commit -m x",
            "docker rm x",
            "docker run busybox",
            "ip link set eth0 down",
            "ip route add default via 1.2.3.4",
            // 旗标逃逸
            "find / -name x -delete",
            "find /var -name x -exec rm {} \\;",
            "sed -i 's/a/b/' /etc/hosts",
            "awk 'BEGIN{system(\"rm -rf /tmp\")}'",
            // env 带参 = 执行
            "env rm -rf /tmp",
            // 重定向写文件
            "echo hi > /tmp/x",
            "echo hi >> /tmp/x",
            "df -h > /tmp/out",
            // 命令替换藏命令
            "echo $(rm -rf /tmp)",
            "cat `ls`",
            // 后台执行
            "df -h & rm -rf /tmp",
            // 管道逃逸
            "cat /etc/passwd | tee /tmp/x",
            "find / | xargs rm -rf",
            // 空命令
            "   ",
            // P111 新守卫：白名单内命令的写形态/挂起形态
            "sort -o /etc/passwd /tmp/in",
            "sort --output=/etc/passwd /tmp/in",
            "date -s '2020-01-01'",
            "hostname newname",
            "hostname -F /tmp/hostname-file",
            "hostnamectl set-hostname web1",
            "tail -f /var/log/messages",
            "dmesg -c",
            "journalctl --vacuum-size=1M",
            "journalctl -f",
            "ping example.com",
            "top",
            "awk '{print $1 > \"/tmp/x\"}' /etc/in",
            "awk '{print | \"sh\"}' /etc/in",
            "nmcli networking off",
            "nmcli connection down ens33",
        ] {
            assert!(!is_readonly_command(bad), "应拒绝: {bad}");
        }
    }

    /// P111 规格表结构不变量：命令名不得重复（防两张表时代的漂移复发）
    #[test]
    fn readonly_specs_have_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for (name, _) in READONLY_SPECS {
            assert!(seen.insert(*name), "规格表重复命令: {name}");
        }
    }
}
