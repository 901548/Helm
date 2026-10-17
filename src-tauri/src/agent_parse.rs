// ============================================================================
// agent_parse.rs - Agent 文本协议解析（P134-C 自 agent.rs 拆出）
// 模型回复清理/归一化、命令提取（含 FC 工具名剥壳）、子目标协议
// （GOAL/REFLEXION/goal_marker）、QA 只读 RUN 协议与流式显示过滤器。
// ============================================================================

// ============================================================================
// P110 QA 只读命令协议：模型可输出 `RUN: <只读命令>` 行请求执行查看类命令，
// 由 ai_job 的 QA 循环经 safety::is_readonly_command 门禁后自动执行并回喂输出。
// ============================================================================

/// 是否为 RUN 请求行（行首 `RUN:` 或全角冒号 `RUN：`；两侧冒号都认，防模型用全角）
pub fn is_run_line(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("RUN:") || t.starts_with("RUN：")
}

/// 从完整回复提取全部 RUN 命令（保持出现顺序）
pub fn extract_run_commands(reply: &str) -> Vec<String> {
    reply
        .lines()
        .filter(|l| is_run_line(l))
        .map(|l| {
            let t = l.trim();
            t.strip_prefix("RUN:")
                .or_else(|| t.strip_prefix("RUN："))
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .filter(|c| !c.is_empty())
        .collect()
}

/// 剥掉回复里的 RUN 行——问答记录与完成摘要只保留给用户看的回答文本
/// （执行情况由步骤卡呈现，不混进答案正文）
pub fn strip_run_lines(reply: &str) -> String {
    let filtered: Vec<&str> = reply.lines().filter(|l| !is_run_line(l)).collect();
    filtered.join("\n")
}

/// P113 流式答案过滤器：RUN 行不进流式答案（执行情况由步骤卡呈现）。
///
/// 按行缓冲——`RUN:` 可能被 chunk 从中间切开，凑成完整行才判定；
/// `finish` 在每轮回合结束时冲刷无换行的尾段（回答末行通常不带换行）。
pub struct QaLineFilter {
    buf: String,
}

impl Default for QaLineFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl QaLineFilter {
    pub fn new() -> Self {
        Self { buf: String::new() }
    }

    /// 喂入一段增量，返回其中可显示的文本（RUN 行被吞掉）
    pub fn push(&mut self, delta: &str) -> String {
        self.buf.push_str(delta);
        let mut out = String::new();
        while let Some(pos) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=pos).collect();
            if !is_run_line(&line) {
                out.push_str(&line);
            }
        }
        out
    }

    /// 回合结束冲刷：返回剩余尾段中可显示的部分，并清空缓冲（下一轮从零开始）
    pub fn finish(&mut self) -> String {
        let rest = std::mem::take(&mut self.buf);
        if is_run_line(&rest) {
            String::new()
        } else {
            rest
        }
    }
}

/// 清理模型回复：去除首尾任意层代码围栏并修剪空白
pub(crate) fn clean_response(resp: &str) -> String {
    let lines: Vec<&str> = resp.trim().lines().collect();
    let mut start = 0;
    while start < lines.len() && lines[start].trim_start().starts_with("```") {
        start += 1;
    }
    let mut end = lines.len();
    while end > start && lines[end - 1].trim_end().ends_with("```") {
        end -= 1;
    }
    lines[start..end].join("\n").trim().to_string()
}

/// 去除命令行常见前缀
fn strip_cmd_prefix(line: &str) -> &str {
    for p in ["命令：", "命令:", "执行：", "执行:"] {
        if let Some(rest) = line.strip_prefix(p) {
            return rest.trim();
        }
    }
    line
}

// ---------- P100-7：FC 工具名剥壳 ----------
// 背景：模型不支持 function calling 时（glm4 等）会粘性降级到文本协议，但它仍可能把
// FC 工具名当普通文本吐出来（实测 `run_command uname -r` / `run_command uptime -p`），
// 旧解析层原样当 shell 命令执行 → `exit 127`（bash: run_command: 未找到命令），
// 一个任务白烧 3 个回合，且这些假命令会写进 P70 训练日志污染 SFT 数据。

/// 剥壳后保留命令的 FC 工具名（`run_command <cmd>` → `<cmd>`）
const FC_TOOL_RUN: &str = "run_command";
/// 纯协议行（不是 shell 命令）——整行丢弃：`set_goal …` / `goal_ok` / `reflect …` / `finish`
const FC_TOOLS_DROP: [&str; 6] = ["set_goal", "goal_ok", "goal_done", "goal_fail", "reflect", "finish"];

/// 工具名后的合法边界字符（含 markdown 包裹 `**run_command** ls`）
fn is_tool_boundary(c: char) -> bool {
    c.is_whitespace() || matches!(c, ':' | '(' | '{' | '[' | '*' | '`')
}

/// 行首是否为指定工具名；是则返回其后的剩余部分。仅在**行首**匹配，
/// 故 `echo run_command` / `grep run_command f` 这类合法命令不受影响。
fn strip_tool_name<'a>(lower: &str, orig: &'a str, name: &str) -> Option<&'a str> {
    if !lower.starts_with(name) {
        return None;
    }
    let rest = &orig[name.len()..];
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if is_tool_boundary(c) => Some(rest),
        _ => None, // run_command_x / run_commands 之类不算
    }
}

/// 从 `{"command": "ls -la"}` 形态里取命令值（含单双引号、简单转义）
fn extract_json_command(s: &str) -> Option<String> {
    let key = "\"command\"";
    let low = s.to_ascii_lowercase();
    let idx = low.find(key)?;
    let after = s[idx + key.len()..].trim_start().strip_prefix(':')?.trim_start();
    let quote = after.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut out = String::new();
    let mut escaped = false;
    for c in after[1..].chars() {
        if escaped {
            out.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == quote {
            let t = out.trim();
            return if t.is_empty() { None } else { Some(t.to_string()) };
        }
        out.push(c);
    }
    None
}

/// 规范化工具调用载荷：`({"command": "x"})` / `: x` / `(x)` / `** x` → `x`
fn normalize_tool_payload(rest: &str) -> String {
    let s = rest.trim();
    // 先试 JSON 形态——必须在剥引号**之前**，否则 key 自身的引号会被吃掉（单测抓到）
    if let Some(cmd) = extract_json_command(s) {
        return cmd;
    }
    s.trim_start_matches(|c: char| {
        c.is_whitespace() || matches!(c, ':' | '(' | '[' | '{' | '*' | '`' | '"' | '\'')
    })
    .trim_end_matches(|c: char| {
        c.is_whitespace() || matches!(c, ')' | ']' | '}' | '*' | '`' | '"' | '\'' | ';')
    })
    .trim()
    .to_string()
}

/// P100-7：处理行首 FC 工具名。
/// - `run_command <cmd>` / `: <cmd>` / `(<cmd>)` / `({"command":"<cmd>"})` → `Some(<剥壳后的命令>)`
/// - `set_goal …` / `goal_ok` / `reflect …` / `finish` → `None`（整行丢弃）
/// - 其它行 → `Some(原样)`（注意不做首尾 `*` 清理，避免破坏 `ls *` 这类命令）
fn strip_fc_tool_wrapper(line: &str) -> Option<String> {
    let t = line.trim();
    // 允许 markdown 包裹：`**run_command …**` / `` `run_command …` ``
    let (lead, body) = if let Some(r) = t.strip_prefix("**") {
        ("**", r)
    } else if let Some(r) = t.strip_prefix('`') {
        ("`", r)
    } else {
        ("", t)
    };
    let lower = body.to_ascii_lowercase();
    for name in FC_TOOLS_DROP {
        if strip_tool_name(&lower, body, name).is_some() {
            return None;
        }
    }
    if let Some(rest) = strip_tool_name(&lower, body, FC_TOOL_RUN) {
        let cmd = normalize_tool_payload(rest);
        let cmd = match lead {
            "**" => cmd.trim_end_matches("**").trim().to_string(),
            "`" => cmd.trim_end_matches('`').trim().to_string(),
            _ => cmd,
        };
        return if cmd.is_empty() { None } else { Some(cmd) };
    }
    Some(t.to_string())
}

/// 判定一行是否像一条可直接执行的 shell 命令。
///
/// 弱本地模型（deepseek-r1 等）常把系统提示词回显出来（如
/// 「如果任务完成，请输出 "DONE"」「只输出 DONE」）或输出结论性散文，
/// 这类行不应被当作 shell 命令执行，否则会陷入"命令找不到"的死循环。
pub fn is_command_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    let upper = t.to_ascii_uppercase();
    // 提示词回显句式：指令被模型原样吐出
    if t.contains("如果任务完成") || t.contains("如果任务已完成") {
        return false;
    }
    if upper.contains("请输出") && upper.contains("DONE") {
        return false;
    }
    if (t.contains("只输出") || t.contains("不要输出") || t.contains("不要解释"))
        && upper.contains("DONE")
    {
        return false;
    }
    // 真命令至少含一个 ASCII 命令词元（剔除纯标点/纯中文散文行）
    let has_word = t.bytes().any(|b| b.is_ascii_graphic() && !b.is_ascii_digit());
    if !has_word {
        return false;
    }
    // P57 L2 协议标记：GOAL/REFLEXION/EXPECT 开头的是子目标/反思/校验指令,不是 shell 命令
    let pfx = |p: &str| upper.starts_with(p);
    if !(!pfx("GOAL ") && !pfx("GOAL:") && !pfx("GOAL_") && upper != "GOAL"
        && !pfx("REFLEXION ") && !pfx("REFLEXION:")
        && !pfx("EXPECT ") && !pfx("EXPECT:"))
    {
        return false;
    }
    // P68-§8.6 弱模型散文过滤：shell 命令必以 ASCII 程序名开头（exec 通道无交互别名），
    // 首字符为 CJK 的一律是解释文字（"对策：使用 systemctl ..."类反思散文、
    // "检查磁盘: smartctl ..."式条目——后者剥掉行首序号后同样 CJK 开头）。
    // 含中文**参数**的合法命令（echo 你好、grep 模式）首词元是 ASCII，不受影响。
    if starts_cjk(t) || starts_cjk(strip_enumerator(t)) {
        return false;
    }
    true
}

/// 首字符是否为 CJK 统一表意文字（含扩展 A 与兼容区）
fn starts_cjk(s: &str) -> bool {
    s.chars().next().is_some_and(|c| {
        let u = c as u32;
        (0x3400..=0x4DBF).contains(&u)
            || (0x4E00..=0x9FFF).contains(&u)
            || (0xF900..=0xFAFF).contains(&u)
    })
}

/// 剥掉行首序号（"1." "1、" "1)" + 可选空格）：弱模型爱输出 "1. 检查磁盘" 式条目。
/// 非"数字+分隔符"开头原样返回（"7z x"、"2>&1" 不受影响）。
fn strip_enumerator(t: &str) -> &str {
    let mut end = 0;
    for (i, c) in t.char_indices() {
        if c.is_ascii_digit() {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 {
        return t;
    }
    let rest = &t[end..];
    for sep in [".", "、", ")"] {
        if let Some(r) = rest.strip_prefix(sep) {
            return r.trim_start();
        }
    }
    t
}

/// 判断清洗后的回复中是否含一行独立的 DONE（不区分大小写），
/// 出现即视为任务完成——即使同一回复里还夹带了杂散噪音行。
pub fn contains_done_line(s: &str) -> bool {
    s.lines().any(|l| l.trim().eq_ignore_ascii_case("DONE"))
}

/// 将模型回复解析为待执行的命令列表
/// （去围栏、按行拆分、过滤注释/空行及提示词回显等非命令行）
pub fn parse_commands(text: &str) -> Vec<String> {
    clean_response(text)
        .lines()
        .filter_map(|raw| {
            // 顺序：中文前缀 → 行首序号 → P100-7 FC 工具名（None = 协议行，整行丢弃）
            let l = strip_fc_tool_wrapper(strip_enumerator(strip_cmd_prefix(raw.trim())))?;
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') || !is_command_line(l) {
                return None;
            }
            Some(l.to_string())
        })
        .collect()
}

/// 截断文本到指定字符数，超长时追加省略标记
pub fn truncate_text(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max).collect();
        s.push_str("…[已截断]");
        s
    }
}

/// 提取模型回复中的子目标行 `GOAL <序号> <短标题>`（P57 L2）。
/// 仅取纯标题（去掉序号），容错：无序号也可。非 `GOAL` 开头行一律忽略。
/// 弱模型常把多个 GOAL 挤在同一行（"GOAL 1. x GOAL 2. y"），按 GOAL 关键字切段逐一解析。
pub fn extract_goals(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        for seg in goal_segments(line.trim()) {
            let t = seg.trim();
            let upper = t.to_ascii_uppercase();
            if !(upper.starts_with("GOAL ") || upper.starts_with("GOAL:")) {
                continue;
            }
            let body = &t[4..].trim_start_matches([':', ' ']).trim();
            if body.is_empty() {
                continue;
            }
            // 去掉常见序号前缀："1."、"1)"、"1、"、"0x" 等
            let title = strip_goal_number(body);
            if !title.is_empty() {
                out.push(title.to_string());
            }
        }
    }
    out
}

/// 把一行按 `GOAL` 关键字出现位置切成若干 "GOAL..." 段（大小写不敏感；无切分时返回整行单段）
fn goal_segments(line: &str) -> Vec<&str> {
    let upper = line.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    let mut marks: Vec<usize> = Vec::new();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if &bytes[i..i + 4] == b"GOAL" {
            marks.push(i);
            i += 4;
        } else {
            i += 1;
        }
    }
    if marks.len() <= 1 {
        return vec![line];
    }
    let mut out = Vec::with_capacity(marks.len());
    for (n, s) in marks.iter().enumerate() {
        let end = marks.get(n + 1).copied().unwrap_or(line.len());
        out.push(&line[*s..end]);
    }
    out
}

/// 去掉子目标标题的开头序号（"1." "1)" "01-" "2." 等），保留其余。
fn strip_goal_number(s: &str) -> &str {
    let t = s.trim_start();
    let v: Vec<(usize, char)> = t.char_indices().collect();
    let len = v.len();
    if len == 0 {
        return t;
    }
    let mut i = 0usize;
    // 0x/0X 十六进制序号或十进制序号
    let mut is_hex = false;
    if len >= 2 && v[0].1 == '0' && (v[1].1 == 'x' || v[1].1 == 'X') {
        is_hex = true;
        i = 2;
    }
    while i < len && v[i].1.is_numeric() {
        i += 1;
    }
    if is_hex {
        while i < len && v[i].1.is_ascii_hexdigit() {
            i += 1;
        }
    }
    // 吃掉紧跟的序号分隔符："." ")" "、" " " "-"
    while i < len && matches!(v[i].1, '.' | ')' | '、' | '-' | '：' | ':' | ' ') {
        i += 1;
    }
    if i >= len {
        return "";
    }
    &t[v[i].0..]
}

/// 提取模型回复中的失败反思行 `REFLEXION <描述>`（P57 L2 Reflexion）。
pub fn extract_reflexion(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let t = l.trim();
        let upper = t.to_ascii_uppercase();
        // "对策：/反思：" 是弱模型的中文 REFLEXION 等价物（§8.6 实测 glm4 输出格式），同等采纳
        for (tag, skip) in [
            ("REFLEXION ", 10usize),
            ("REFLEXION:", 10),
            ("对策：", 0),
            ("对策:", 0),
            ("反思：", 0),
            ("反思:", 0),
        ] {
            let hit = if skip > 0 {
                upper.starts_with(tag)
            } else {
                t.starts_with(tag)
            };
            if hit {
                let body = if skip > 0 {
                    t[skip..].trim_start_matches([':', ' ']).trim()
                } else {
                    t[tag.len()..].trim()
                };
                if !body.is_empty() {
                    return Some(body.to_string());
                }
            }
        }
        None
    })
}

/// 子目标显式终结标记（P57 L2）：`GOAL_OK` / `GOAL_DONE` / `GOAL_FAIL`。
/// 命中返回终结状态，否则返回 None。模型可用它声明某子目标完成/失败（如"校验"类空命令目标）。
pub fn goal_marker(text: &str) -> Option<bool> {
    text.lines().find_map(|l| {
        let u = l.trim().to_ascii_uppercase();
        match u.as_str() {
            "GOAL_OK" | "GOAL_DONE" => Some(true),
            "GOAL_FAIL" => Some(false),
            _ => None,
        }
    })
}
