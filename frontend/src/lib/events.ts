// 事件订阅(listen)——返回 Promise<UnlistenFn>
import { listen } from "@tauri-apps/api/event";
import type { ConnectionPayload, AiPayload, TerminalOutputPayload, SysMonPayload } from "./types";

// ---------- 事件订阅 ----------

export const onConnection = (cb: (p: ConnectionPayload) => void) =>
  listen<ConnectionPayload>("connection", (e) => cb(e.payload));

export const onAi = (cb: (p: AiPayload) => void) =>
  listen<AiPayload>("ai", (e) => cb(e.payload));

export const onTerminalOutput = (cb: (p: TerminalOutputPayload) => void) =>
  listen<TerminalOutputPayload>("terminal-output", (e) => cb(e.payload));

export const onSysMon = (cb: (p: SysMonPayload) => void) =>
  listen<SysMonPayload>("sysmon", (e) => cb(e.payload));
