// 类型定义(与 core.rs serde 对齐)
// ---------- 类型(与 core.rs serde 对齐) ----------

export type SessionKind = "linux" | "windows" | "rdp";

export interface SessionInfo {
  name: string;
  kind?: SessionKind;
  host: string;
  port: number;
  user: string;
  key_file: string | null;
  password: string | null;
}

export type SessionStatus = "Connected" | "Disconnected" | "Connecting";

export interface AiConfig {
  model: string;
  api_key_env: string;
  api_key: string | null;
  api_base_url: string | null;
  system_prompt: string;
  temperature: number;
  max_tokens: number | null;
  stream: boolean;
  max_history: number;
  max_steps: number;
  max_output_chars: number;
  timeout_secs: number;
  agent_confirm: boolean;
  command_timeout_secs: number | null;
  system_prompt_agent: string | null;
  mode: string;
  extra_headers: Record<string, string>;
  extra_body: unknown;
}

export type Theme = "light" | "dark" | "system";

export interface UiConfig {
  window_width: number;
  window_height: number;
  dock_sessions: boolean;
  dock_chat: boolean;
  sessions_panel_pct: number;
  chat_panel_pct: number;
  theme: Theme;
}

export type DangerLevel = "Safe" | "Warning" | "Critical";

// ---------- 事件负载 ----------

export interface ConnectionPayload {
  status: "connected" | "failed" | "disconnected";
  name: string;
  error?: string;
}

export type AiPayload =
  | { kind: "stepBegin" }
  | { kind: "streaming"; text: string }
  | { kind: "stepOutputEnd" }
  | { kind: "commandStep"; command: string; success: boolean; message: string; output: string }
  | { kind: "done"; message: string }
  | { kind: "error"; message: string }
  | { kind: "pendingCommand"; command: string; level: DangerLevel; reason: string }
  | { kind: "busy"; busy: boolean };

export interface TerminalOutputPayload {
  name: string;
  data: number[];
}

export interface SysMonPayload {
  name: string;
  cpu: number;
  memTotalMb: number;
  memUsedMb: number;
  load1: number;
  load5: number;
  load15: number;
  rxBps: number;
  txBps: number;
  ok: boolean;
  error?: string;
}

export interface FileEntry {
  name: string;
  isDir: boolean;
  size: number;
  perms: string;
  mtime: string;
}

export interface FsResult {
  ok: boolean;
  message: string;
  truncated?: boolean;
}

export interface AiLogEntry {
  id: number;
  name: string;
  type: "cmd" | "info";
  command: string;
  success: boolean;
  message: string;
  output: string;
}

// AI 活动流卡片(P39):QA 回答 / Agent 命令步骤
export interface AiQaCard {
  id: number;
  kind: "qa";
  text: string;
  done: boolean;
}

export type AiStepStatus = "running" | "ok" | "fail" | "skipped" | "confirm";

export interface AiStepCard {
  id: number;
  kind: "step";
  command: string;
  status: AiStepStatus;
  message?: string;
  output?: string;
  level?: DangerLevel;
  reason?: string;
}

export type AiCard = AiQaCard | AiStepCard;

