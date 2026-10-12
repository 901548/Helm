// 后端权威对话记录 → 前端活动流卡片（P124 自 App.svelte 抽出）
//
// 为什么单独成模块：这段映射是「切标签回看」的正确性核心（后端 ConvEntry 的字段名、
// 状态取值、Goal 分组语义都在这里对齐），却埋在 App.svelte 的 $effect 里无法单测。
// 抽成纯函数后可用 Vitest 直接覆盖，字段漂移会立刻被测试抓到。
import type { AiCard } from "./types";

/** 后端 ConvEntry 的 JSON 形态（字段名见 core.rs：变体名 lowercase，字段仍 snake_case） */
export type ConvEntry = Record<string, unknown>;

/** 取字符串字段（后端字段缺失时给空串，避免 "undefined" 进 UI） */
const str = (v: unknown): string => (v == null ? "" : String(v));

/** 把一条 ConvEntry 映射为卡片；`id` 由调用方分配（保证前端 key 稳定唯一） */
export function convToCard(e: ConvEntry, id: number): AiCard {
  const kind = str(e.kind);
  if (kind === "qa") {
    return { id, kind: "qa", text: str(e.a), done: true };
  }
  if (kind === "plan") {
    return {
      id,
      kind: "plan",
      commands: (e.commands as never) ?? [],
      status: "plan",
      needConfirm: Boolean(e.need_confirm),
    };
  }
  // P104-4 L2-beta：子目标组头（后端 ConvEntry::Goal，status active/ok/failed）
  if (kind === "goal") {
    const st = str(e.status) || "active";
    return {
      id,
      kind: "goal",
      index: Number(e.index ?? 0),
      total: Number(e.total ?? 0),
      title: str(e.title),
      status: st === "ok" || st === "failed" ? st : "active",
    };
  }
  // 兜底按 step 处理（后端目前也只有 qa/plan/step/goal/task 五种）
  return {
    id,
    kind: "step",
    command: str(e.command),
    status: e.success ? "ok" : "fail",
    message: str(e.message),
    output: str(e.output) || undefined,
  };
}

/** 从对话记录里取出任务文本（`task` 条目不是卡片，只用于回填输入框上方的任务锚点） */
export function convTaskText(conv: ConvEntry[]): string {
  const task = conv.find((e) => str(e.kind) === "task");
  return task ? str(task.task) : "";
}

/**
 * 整份对话记录 → 卡片列表（跳过 task 条目）。
 * `nextId` 是取号器：调用方通常传 `() => ++cardId` 以保证全局唯一。
 */
export function convToCards(conv: ConvEntry[], nextId: () => number): AiCard[] {
  return conv.filter((e) => str(e.kind) !== "task").map((e) => convToCard(e, nextId()));
}
