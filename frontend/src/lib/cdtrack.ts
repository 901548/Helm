// 非 bash 会话的 cd 跟随回退状态机（P124 自 TerminalTabs.svelte 抽出）
//
// 背景（P23/P24）：权威 PWD 是 bash 的 OSC7 标记；没有标记的会话（非 bash、或注入失效）
// 只能靠"回显行 + 紧随的提示符"推断 cd 是否成功。这套推断有三个易错点，全部靠本模块的
// 测试锁住：
//   1. 成功提示符行**不带尾换行** —— 若只按 \n 切"完整行"，提示符会滞留尾缓冲永不确认，
//      表现为"cd 跟随失效"。故未成行的尾段也必须投入扫描（并原样留回缓冲）。
//   2. 失败必须作废 —— `-bash: cd: /x: No such file` 这类错误行要清掉候选，否则
//      后续任意提示符都会把它误判为成功。
//   3. 中间有其他输出即作废 —— 杜绝 `echo 'cd /tmp'` 之类的输出误触发。

/** 已登记、等待确认的 cd 候选 */
export interface CdPending {
  target: string;
  t: number;
  /** 登记时的原始行文本：用于识别"同一行被重新扫描"，避免确认窗口被无限续期 */
  raw: string;
}

/** 每会话的回退跟踪状态（由调用方按会话名保存） */
export interface CdFallbackState {
  /** 尚未成行的尾段（跨 data 块累积；成功提示符常以这种形态到达） */
  buf: string;
  /** 已登记、等待确认的 cd 候选 */
  pending: CdPending | null;
}

/** 初始状态 */
export function initialCdState(): CdFallbackState {
  return { buf: "", pending: null };
}

/** 尾缓冲上限：防长时间无换行的刷屏把内存撑大 */
export const MAX_ECHO_TAIL = 16384;

/** 回退确认的时间窗：提示符须在此窗口内出现才算"这次 cd 的"提示符 */
const CONFIRM_WINDOW_MS = 3000;

/** 登记行：`]# cd /etc` / `$ cd` / `]# cd '/dev'`（提示符锚定） */
const ECHO_CD_RE = /\]\s*[#$]\s*cd(?:[ \t]+([^\s;&|]+))?[ \t]*$/;
/** 失败行：`-bash: cd: /x: ...`（中英文错误都靠 `: cd(` 前缀命中） */
const CD_ERROR_RE = /: cd(:| )|\bNo such file\b|\bnot found\b/;
/** 确认行：裸提示符 `]# `，或 `user@host:path# ` 形态（后者须在时间窗内） */
const BARE_PROMPT_RE = /\]\s*[#$]\s*$/;
const FULL_PROMPT_RE = /^\S+@\S+[: ].*[#$]\s*$/;

export interface CdFallbackResult {
  /** 更新后的状态（调用方写回） */
  state: CdFallbackState;
  /** 本次需要触发的 cd 目标（通常 0 或 1 个） */
  fires: string[];
}

/**
 * 消费一段（已 stripAnsi 的）终端输出，推进 cd 回退跟踪。
 *
 * @param state 上一轮状态
 * @param text  本轮解码后的文本
 * @param now   当前时间戳（Date.now()；显式传入以便测试）
 */
export function trackCdFallback(
  state: CdFallbackState,
  text: string,
  now: number,
): CdFallbackResult {
  let tail = state.buf + text;
  if (tail.length > MAX_ECHO_TAIL) tail = tail.slice(-MAX_ECHO_TAIL);

  const lines = tail.split(/\r\n|\n|\r/);
  let keep = "";
  if (!/[\r\n]$/.test(tail)) {
    // 末段未成行：既要扫描（提示符常在此），又要留回缓冲
    keep = lines.pop() ?? "";
  }

  let pending = state.pending;
  const fires: string[] = [];

  for (const rawLine of [...lines, keep]) {
    const line = rawLine.trim();
    if (!line) continue;

    // 1) 登记候选：提示符后的复合回显行（Tab 补全/历史回放/粘贴都会以这种形态出现）
    //
    // 注意：未成行的尾段会留在 buf 里，下一块**重新扫描**它。若每次都刷新 `t`，
    // 3 秒确认窗口就永远不会过期（P124 抽模块时发现的原实现缺陷）。故同一行
    // 只在内容变化时才重新计时。
    const echoM = line.match(ECHO_CD_RE);
    if (echoM) {
      const tgt = echoM[1] ?? "~";
      // `cd -`（回到上一目录）目标未知，置空表示"不触发跟随"
      const target = tgt === "-" ? "" : tgt.replace(/^['"]|['"]$/g, "");
      const sameLine = pending !== null && pending.raw === line;
      pending = { target, t: sameLine ? pending.t : now, raw: line };
      continue;
    }

    if (!pending) continue;

    // 2) 失败行 → 作废
    if (CD_ERROR_RE.test(line)) {
      pending = null;
      continue;
    }

    // 3) 成功确认：紧随的新提示符
    const confirmOk =
      BARE_PROMPT_RE.test(line) || (FULL_PROMPT_RE.test(line) && now - pending.t < CONFIRM_WINDOW_MS);
    if (confirmOk) {
      if (pending.target) fires.push(pending.target);
      pending = null;
      continue;
    }

    // 4) 其他输出 → 非简单 cd，作废（防误触发）
    pending = null;
  }

  return { state: { buf: keep, pending }, fires };
}
