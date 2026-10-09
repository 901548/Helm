// AI 回显投递（P99-2）
// 背景：AI 过程回显经 App 的 `aiEcho` 队列推给 TerminalTabs，由 `$effect` 写进各会话终端。
// 旧实现用**单一全局** `lastEchoSeq`：终端未建的条目虽然 `continue` 不推进游标，但同帧内
// 后续**更高 seq** 的条目会把游标推过去 → 那条低 seq 回显（典型是"连接失败：原因"）
// 下一次因 `seq <= lastEchoSeq` 被永久跳过，与"保留待写"的注释意图相反（多会话并行时最易触发）。
// 现改为**按会话独立游标**：某会话终端未建时，只阻塞该会话自己的后续条目，不影响其它会话。

export interface EchoEntry {
  seq: number;
  name: string;
  text: string;
}

/**
 * 计算本次应写入的回显条目，并返回更新后的每会话游标。
 * @param entries 待处理队列（App 按 seq 递增追加）
 * @param cursors 每会话已写到的 seq（会话名 → seq）
 * @param hasTerm  该会话终端是否已建立
 */
export function replayEcho(
  entries: EchoEntry[],
  cursors: Map<string, number>,
  hasTerm: (name: string) => boolean,
): { writes: EchoEntry[]; next: Map<string, number> } {
  const next = new Map(cursors);
  const writes: EchoEntry[] = [];
  for (const e of entries) {
    if (e.seq <= (next.get(e.name) ?? 0)) continue;
    // 终端未建：保留待写——该会话游标不动，终端建好后由 termVersion 触发重放补写
    if (!hasTerm(e.name)) continue;
    writes.push(e);
    next.set(e.name, e.seq);
  }
  return { writes, next };
}
