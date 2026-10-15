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

// 忘记主机的 TOFU 公钥指纹(服务器重装后密钥变更时重置信任);返回是否有记录被删
export const forgetHostKey = (host: string, port: number) =>
  invoke<boolean>("forget_host_key", { host, port });

// 系统默认浏览器打开外部链接(终端链接点击;后端只放行 http/https)
export const openExternal = (url: string) => invoke<void>("open_external", { url });

// P73 ZMODEM：协议字节走 record=false（训练日志只记人类键入）
export const sendInputRaw = (name: string, data: Uint8Array) =>
  invoke<void>("send_input", { name, data, record: false });

// ZMODEM 下载落盘（后端写 ~/Downloads/helm-zmodem/<名>，返回实际路径）
export const zmodemSave = (name: string, b64: string) =>
  invoke<string>("zmodem_save", { name, b64 });

// 切换主窗口全屏（F11），返回切换后的状态
export const toggleFullscreen = () => invoke<boolean>("toggle_fullscreen");

// ---------- 终端 commands ----------

export const sendInput = (name: string, data: Uint8Array) =>
  invoke<void>("send_input", { name, data: Array.from(data) });

export const sendActiveInput = (data: Uint8Array) =>
  invoke<void>("send_active_input", { data: Array.from(data) });

export const resizeSessions = (cols: number, rows: number) =>
  invoke<void>("resize_sessions", { cols, rows });

// ---------- AI commands ----------
// AI 操作按会话隔离：所有调用携带目标会话名（多会话并行）

export const aiSubmit = (
  name: string,
  input: string,
  pwd?: string,
  container?: string | null,
  termContext?: string | null,
) =>
  invoke<void>("ai_submit", { name, input, pwd, container: container ?? null, termContext: termContext ?? null });

export const aiControl = (
  name: string,
  action: "approve" | "reject" | "cancel" | "edit",
  commands?: string[],
) => invoke<void>("ai_control", { name, action, commands: commands ?? null });

export const aiStop = (name: string) => invoke<void>("ai_stop", { name });

export const aiClearHistory = (name: string) => invoke<void>("ai_clear_history", { name });

export const aiSetMode = (name: string, mode: "qa" | "agent") =>
  invoke<void>("ai_set_mode", { name, mode });

export const aiMode = (name: string) => invoke<"qa" | "agent">("ai_mode", { name });

// 列出 Docker 宿主机容器名(docker ps)，供 Agent 任务容器下拉选择（§8.7.4）
export const dockerPs = (name: string) => invoke<string[]>("docker_ps", { name });

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


// 读取操作历史（记录器 JSONL，新→旧；可按会话/类型过滤）
export const historyRead = (limit?: number, session?: string, type?: string) =>
  invoke<Record<string, unknown>[]>('history_read', { limit: limit ?? null, session: session ?? null, type: type ?? null });

// 读取会话对话记录（P89：后端 AiSlot.conv，切标签时拉取）
export const aiConvRead = (name: string) =>
  invoke<Record<string, unknown>[]>('ai_conv_read', { name });

// 清空会话对话记录
export const aiConvClear = (name: string) => invoke<void>('ai_conv_clear', { name });

// 导出全部操作历史为单个 JSONL（返回落盘路径）
export const historyExport = () => invoke<string>('history_export');

// ---------- P127 快照（一期：探测 + 手动打包 + 列表 + 删除） ----------
// 命题：让不可逆命令变可逆。详见 docs/P127-服务器快照-设计方案.md

export interface SnapshotEntry {
  file: string;
  path: string;
  createdAt: number;
  size: number;
  verified: boolean;
  paths: string[];
}

export interface SnapshotProbe {
  diskAvailMb: number;
  pathSizes: [string, number][];
  recommended: string[];
  /** 给用户看的**结论**（不是选择题）—— 见设计稿 §4.8 */
  reason: string;
  lvmFreeMb: number;
  reflink: boolean;
  memTotalMb: number;
}

export interface SnapshotPack {
  file: string;
  path: string;
  size: number;
  verified: boolean;
  elapsedMs: number;
  paths: string[];
}

/** 探测本机快照能力（返回推荐配置，不让用户选档位） */
export const snapshotProbe = (name: string) =>
  invoke<SnapshotProbe>('snapshot_probe', { name });

/** 打包快照（内部先做体积与空间检查，不够则拒绝） */
export const snapshotPack = (
  name: string,
  paths: string[],
  diskAvailMb: number,
  estMb: number,
) => invoke<SnapshotPack>('snapshot_pack', { name, paths, diskAvailMb, estMb });

export const snapshotList = (name: string) =>
  invoke<SnapshotEntry[]>('snapshot_list', { name });

export const snapshotRemove = (name: string, file: string) =>
  invoke<void>('snapshot_remove', { name, file });

/** 执行保留策略（超出份数删最旧），返回删除数量 */
export const snapshotPrune = (name: string) =>
  invoke<number>('snapshot_prune', { name });
// ---------- P127 二期：命令前拦截 ----------

/** 查询是否有被拦截、等待确认的危险命令；返回命令行或 null */
export const interceptPending = () =>
  invoke<[string, string] | null>('intercept_pending');

/** 解决挂起的危险命令：approve=true 原样重放，false 丢弃 */
export const interceptResolve = (approve: boolean) =>
  invoke<void>('intercept_resolve', { approve });

/** 设置/读取命令前拦截开关（默认关，须显式开启） */
export const snapshotSetIntercept = (on: boolean) =>
  invoke<void>('snapshot_set_intercept', { on });
export const snapshotGetIntercept = () => invoke<boolean>('snapshot_get_intercept');