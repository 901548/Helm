// 终端流协议纯函数(自 TerminalTabs 抽出,P44):OSC7 权威 PWD 解析 + ANSI 剥离
// 解析逻辑承载 P24 的跨 TCP 分包拼接语义,是前端最易错的部分,配 Vitest 测试。

/// 剥离 ANSI/CSI/OSC 转义序列
export function stripAnsi(s: string): string {
  return s
    .replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, "")
    .replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, "")
    .replace(/\x1b[()[\]][0-9A-Za-z]/g, "")
    .replace(/\x1b[@-Z\\\-_]/g, "");
}

/// OSC7 标记解析结果:pwd = 最后一个完整标记的 PWD;rest = 未终止片段(跨包前半)
export interface OscParse {
  pwd: string | null;
  rest: string;
}

/// 从「上次未终止片段 + 本次原始数据」中解析 OSC7 `ESC]7;helm:PWD BEL` 标记。
/// - 扫描全部完成的标记取最后一个(PWD 以最后一次提示符重画为准)
/// - 保留最后一个 `ESC]` 起的未终止片段供下块拼接(TCP 分包)
export function parseOscPwd(prev: string, raw: string): OscParse {
  const buf = prev + raw;
  const idx = buf.lastIndexOf("\x1b]");
  let pwd: string | null = null;
  const re = /\x1b\]\d+;helm:([^\x07\x1b]*)(\x07|\x1b\\)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(buf))) {
    if (m[1].length > 0) pwd = m[1];
  }
  let rest = "";
  if (idx !== -1) {
    const frag = buf.slice(idx);
    if (!/(\x07|\x1b\\)/.test(frag)) rest = frag;
  } else if (buf.length > 512) {
    rest = buf.slice(-512);
  }
  if (rest.length > 4096) rest = "";
  return { pwd, rest };
}

/// 归一化 OSC7 回传的 PWD:剥尾斜杠(根除外)
export function normalizePwd(pwd: string): string {
  let norm = pwd;
  while (norm.length > 1 && norm.endsWith("/")) norm = norm.slice(0, -1);
  return norm;
}
