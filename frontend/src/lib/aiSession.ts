// 会话级 AI 态单桶（P134-A：自 App.svelte 8 个并行 Record 收敛）
//
// 为什么收敛：aiBusy/aiState/aiMode/aiCards/aiTaskText/aiSummary/aiStreamOpen/
// aiThinking 曾各自一个 Record，新增一个状态要改四处（声明 / 事件路由 / 改名迁移 /
// 删除清理），该缺陷类在 P89/P92/P95/P96/P127 各出过一次真 bug。收敛为单一 map +
// 本模块后，「加一个状态」只动 AiSessionState 与本文件两处，迁移/清理策略集中且有
// Vitest 覆盖（先例见 lib/conv.ts、lib/cdtrack.ts）。
//
// 写入采用「原地改写」而非 `{ ...rec, [name]: { ... } }` 重建整条目：
// Svelte 5 深层响应式按属性粒度订阅——只有原地写才能保住「conv 拉取 $effect 只随
// 该会话 busy 变化重跑」的原始粒度（整体替换会让每条流式卡片追加都触发重跑，
// 长回答会诱发成百次多余的 aiConvRead）。
import type { AiCard, AiState } from "./types";

/** 一个会话的全部 AI 运行态（多会话并行下各自独立） */
export interface AiSessionState {
  /** 后端 Busy 事件：任务运行中（面板只读活动会话那一份） */
  busy: boolean;
  /** §8.7.1 唯一状态机：后端 State 事件落盘，dock 状态 chip 唯一判态来源 */
  state: AiState;
  /** P95：QA/Agent 模式按会话隔离（旧全局单值在多会话混跑时事件路由串扰） */
  mode: "qa" | "agent";
  /** P89：对话卡片（后端 AiSlot.conv 为权威副本，切标签时拉取重建） */
  cards: AiCard[];
  /** 任务描述锚点（命令条上方显示） */
  taskText: string;
  /** 任务摘要（ok=false 时警示色） */
  summary: { text: string; ok: boolean } | null;
  /** P2-5：活动流开合按会话分桶（全局单值会把 A 的开合态串给 B） */
  streamOpen: boolean;
  /** 推理型模型思考过程：实时累积展示，不混入最终答案卡片 */
  thinking: string;
}

/** 会话名 → AI 态（App.svelte 中的 $state 单桶） */
export type AiSessionMap = Record<string, AiSessionState>;

/** 缺省态工厂（缺条目时 getAi 返回，setAi/updateAi 以此补齐） */
export function defaultAiSession(): AiSessionState {
  return {
    busy: false,
    state: "idle",
    mode: "qa",
    cards: [],
    taskText: "",
    summary: null,
    streamOpen: false,
    thinking: "",
  };
}

/** 只读取用：条目缺失时返回缺省态（模板/条件判断用，勿对其写入） */
export function getAi(rec: AiSessionMap, name: string): AiSessionState {
  return rec[name] ?? defaultAiSession();
}

/** 标量/整体字段写入；条目缺失时按缺省态补齐后合并 patch */
export function setAi(rec: AiSessionMap, name: string, patch: Partial<AiSessionState>): void {
  if (!rec[name]) rec[name] = defaultAiSession();
  Object.assign(rec[name], patch);
}

/** 需要旧值的写入（cards 追加、thinking 累积等）：fn 收到当前态，返回补丁 */
export function updateAi(
  rec: AiSessionMap,
  name: string,
  fn: (s: AiSessionState) => Partial<AiSessionState>,
): void {
  if (!rec[name]) rec[name] = defaultAiSession();
  const cur = rec[name];
  Object.assign(cur, fn(cur));
}

/**
 * 改名迁移（P96 语义）：后端 update_session 已 rename AI 槽并复位 busy——
 * 前端只迁移「静态」状态 mode/streamOpen；卡片/任务/摘要/思考不迁移，
 * 由 $effect(activeTab) 用后端迁移后的 conv 重建，busy/state 默认复位。
 * 源条目不存在时不新建目标条目（原实现此处所有 delete 均为 no-op，等价不动）。
 */
export function renameAiSession(rec: AiSessionMap, from: string, to: string): void {
  if (from === to || !rec[from]) return;
  const old = rec[from];
  delete rec[from];
  if (!rec[to]) rec[to] = defaultAiSession();
  const dst = rec[to];
  dst.mode = old.mode;
  dst.streamOpen = old.streamOpen;
}

/** 删除会话时清理该会话的全部 AI 态 */
export function dropAiSession(rec: AiSessionMap, name: string): void {
  if (name in rec) delete rec[name];
}
