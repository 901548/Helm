// Tauri command 封装(invoke)
import { invoke } from "@tauri-apps/api/core";
import type { SessionInfo, SessionStatus, AiConfig, UiConfig, FileEntry, FsResult } from "./types";

// ---------- 会话 commands ----------

export const listSessions = () =>
  invoke<SessionInfo[]>("list_sessions");

export const sessionStatus = (name: string) =>
  invoke<SessionStatus>("session_status", { name });

export const addSession = (info: SessionInfo) =>
  invoke<void>("add_session", { info });

export const updateSession = (oldName: string, info: SessionInfo) =>
  invoke<void>("update_session", { oldName, info });

export const deleteSession = (name: string) =>
  invoke<void>("delete_session", { name });

export const connectSession = (name: string, cols: number, rows: number) =>
  invoke<void>("connect_session", { name, cols, rows });

export const disconnectSession = (name: string) =>
  invoke<void>("disconnect_session", { name });

export const setActive = (name: string) => invoke<void>("set_active", { name });

export const clearActive = () => invoke<void>("clear_active");

export const rdpConnect = (name: string) => invoke<void>("rdp_connect", { name });

// ---------- 终端 commands ----------

export const sendInput = (name: string, data: Uint8Array) =>
  invoke<void>("send_input", { name, data: Array.from(data) });

export const sendActiveInput = (data: Uint8Array) =>
  invoke<void>("send_active_input", { data: Array.from(data) });

export const resizeSessions = (cols: number, rows: number) =>
  invoke<void>("resize_sessions", { cols, rows });

// ---------- AI commands ----------

export const aiSubmit = (input: string, pwd?: string) =>
  invoke<void>("ai_submit", { input, pwd });

export const aiControl = (action: "approve" | "reject" | "cancel") =>
  invoke<void>("ai_control", { action });

export const aiStop = () => invoke<void>("ai_stop");

export const aiClearHistory = () => invoke<void>("ai_clear_history");

export const aiSetMode = (mode: "qa" | "agent") => invoke<void>("ai_set_mode", { mode });

export const aiMode = () => invoke<"qa" | "agent">("ai_mode");

// ---------- 配置 commands ----------

export const getAiConfig = () => invoke<AiConfig | null>("get_ai_config");

export const updateAiConfig = (config: AiConfig) =>
  invoke<void>("update_ai_config", { config });

// 测试 AI 连接(表单值非空时覆盖已保存配置;api_key 传明文,空=沿用已保存值)
export const testAiConnection = (
  model?: string,
  apiBaseUrl?: string,
  apiKey?: string,
) => invoke<string>("test_ai_connection", { model, apiBaseUrl, apiKey });

// 拉取提供商可用模型列表(参数规则同 testAiConnection)
export const aiListModels = (
  model?: string,
  apiBaseUrl?: string,
  apiKey?: string,
) => invoke<string[]>("ai_list_models", { model, apiBaseUrl, apiKey });

export const getUiConfig = () => invoke<UiConfig>("get_ui_config");

export const updateUiConfig = (config: UiConfig) =>
  invoke<void>("update_ui_config", { config });

// ---------- 文件浏览器 commands ----------

export const fsListDir = (name: string, path: string) =>
  invoke<FileEntry[]>("fs_list_dir", { name, path });

export const fsCurrentDir = (name: string) =>
  invoke<string>("fs_current_dir", { name });

export const fsMkdir = (name: string, path: string) =>
  invoke<FsResult>("fs_mkdir", { name, path });

export const fsRename = (name: string, old_path: string, new_path: string) =>
  invoke<FsResult>("fs_rename", { name, oldPath: old_path, newPath: new_path });

export const fsRemove = (name: string, path: string) =>
  invoke<FsResult>("fs_remove", { name, path });

export const fsUpload = (name: string, path: string, b64: string, append: boolean) =>
  invoke<FsResult>("fs_upload", { name, path, b64, append });

export const fsDownload = (name: string, path: string) =>
  invoke<FsResult>("fs_download", { name, path });

export const fsReadFile = (name: string, path: string, limit: number) =>
  invoke<FsResult>("fs_read_file", { name, path, limit });

