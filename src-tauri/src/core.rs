// ============================================================================
// core.rs - Tauri 命令层：CoreState + commands
// 迁移自旧 app.rs 的连接 / AI 流程语义：
//   - 连接流程：后台 spawn connect → open_shell(cols,rows) → set_active，
//     完成发 "connection" 事件（Connected/Failed）
//   - AI 流程：后台任务按模式分发，QA 直接 chat 流式；Agent 逐步执行，
//     危险/确认命令发 PendingCommand 事件等 ai_control 命令回传
//   - 输出推送：后台任务等 Notify 事件 → drain_output → emit "terminal-output"
// ============================================================================

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::Mutex;

use crate::agent::{Agent, AgentMode};
use crate::config::{save_config, HelmConfig, SessionInfo};
use crate::safety::DangerLevel;
use crate::ai_job::{run_ai_job, TaskCtx};
use crate::ssh::{SshManager, SessionStatus};

// ---------- AI 控制通道 ----------


/// 主循环 → 后台 AI 任务控制
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AiControl {
    /// 确认执行
    Approve,
    /// 跳过
    Reject,
    /// 取消任务
    Cancel,
}

// ---------- 事件负载（emit 到前端） ----------

/// 连接事件
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum ConnectionPayload {
    Connected { name: String },
    Failed { name: String, error: String },
    Disconnected { name: String },
}

/// AI 事件
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum AiPayload {
    StepBegin,
    Streaming { text: String },
    StepOutputEnd,
    CommandStep { command: String, success: bool, message: String, output: String },
    Done { message: String },
    Error { message: String },
    PendingCommand { command: String, level: DangerLevel, reason: String },
    Busy { busy: bool },
}

/// 终端输出事件
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutputPayload {
    pub name: String,
    pub data: Vec<u8>,
}

// ---------- 全局状态 ----------

/// 跨命令共享的核心状态
pub struct CoreState {
    /// 配置文件路径（保存会话时写回）
    pub config_path: PathBuf,
    /// 配置内容（会话列表 + AI 配置），写操作需加锁
    pub config: Mutex<HelmConfig>,
    /// SSH 管理器（内部细粒度锁，网络操作锁外执行）
    pub ssh: Arc<SshManager>,
    /// AI Agent
    pub agent: Arc<Mutex<Agent>>,
    /// AI 任务忙碌标记
    pub ai_busy: Arc<AtomicBool>,
    /// AI 模式缓存(true=Agent):查询不碰 agent 锁,
    /// 避免 QA 聊天持锁期间 ai_mode 被阻塞最长 60s
    pub ai_mode_agent: Arc<AtomicBool>,
    /// AI 控制通道发送端（每次任务启动时重建）
    pub ai_ctl: Arc<Mutex<Option<UnboundedSender<AiControl>>>>,
    /// 文件浏览器各会话当前目录
    pub fs_cwd: Mutex<std::collections::HashMap<String, String>>,
    /// 主机密钥库（TOFU）
    pub known_hosts: Arc<Mutex<crate::known_hosts::KnownHostsStore>>,
}

impl CoreState {
    /// 从配置创建状态；AI 配置缺失或失败时退回默认 Agent
    pub fn new(config_path: PathBuf, config: HelmConfig) -> Self {
        let agent = match config.ai.as_ref() {
            Some(ai_cfg) => Agent::new(ai_cfg).unwrap_or_default(),
            None => Agent::default(),
        };
        let mode_agent = config.ai.as_ref().map(|a| a.mode.eq_ignore_ascii_case("agent")).unwrap_or(false);
        let known_hosts = Arc::new(Mutex::new(crate::known_hosts::KnownHostsStore::load(
            config_path.parent().map(PathBuf::from).unwrap_or_default().join("known_hosts.json"),
        )));
        Self {
            config_path,
            config: Mutex::new(config),
            ssh: Arc::new(SshManager::with_known_hosts(known_hosts.clone())),
            agent: Arc::new(Mutex::new(agent)),
            ai_busy: Arc::new(AtomicBool::new(false)),
            ai_mode_agent: Arc::new(AtomicBool::new(mode_agent)),
            ai_ctl: Arc::new(Mutex::new(None)),
            fs_cwd: Mutex::new(std::collections::HashMap::new()),
            known_hosts,
        }
    }

    /// 读取当前会话列表
    async fn session_list(&self) -> Vec<SessionInfo> {
        self.config.lock().await.sessions.clone()
    }

    /// 将会话列表写回配置文件
    async fn persist_sessions(&self) -> Result<()> {
        let config = self.config.lock().await.clone();
        save_config(&self.config_path, &config)
    }
}

// ---------- 会话 commands ----------

// ---------- 会话密码加密辅助（P26） ----------

/// 明文密码 → 落盘密文（`enc:<base64>`）
fn encrypt_session_password(plain: &str) -> Result<String, String> {
    let cipher = crate::crypto::encrypt_secret(plain).map_err(|e| e.to_string())?;
    Ok(format!("{}{}", crate::crypto::SECRET_ENCRYPTED_PREFIX, cipher))
}

/// 落盘密码 → 连接用明文（`enc:` 密文解密；旧版明文原样透传）
fn decrypt_session_password(stored: Option<&str>) -> Result<Option<String>, String> {
    let Some(stored) = stored else { return Ok(None) };
    let stored = stored.trim();
    if stored.is_empty() {
        return Ok(None);
    }
    if let Some(rest) = stored.strip_prefix(crate::crypto::SECRET_ENCRYPTED_PREFIX) {
        return crate::crypto::decrypt_secret(rest)
            .map(Some)
            .map_err(|e| format!("会话密码解密失败: {e}"));
    }
    Ok(Some(stored.to_string()))
}

/// 落盘密码处理：哨兵/空 → 保留旧值；明文 → 加密；None → 清除
fn persist_session_password(
    incoming: Option<&str>,
    existing: Option<&str>,
) -> Result<Option<String>, String> {
    match incoming.map(str::trim) {
        Some(p) if p.is_empty() || p == crate::crypto::SECRET_MASK => {
            Ok(existing.map(|s| s.to_string()))
        }
        Some(p) => Ok(Some(encrypt_session_password(p)?)),
        None => Ok(None),
    }
}

/// 列出全部会话（密码脱敏为哨兵，密文/明文均不下发）
#[tauri::command]
pub async fn list_sessions(state: State<'_, CoreState>) -> Result<Vec<SessionInfo>, String> {
    let mut sessions = state.session_list().await;
    for s in &mut sessions {
        if let Some(p) = s.password.as_mut() {
            if !p.is_empty() {
                *p = crate::crypto::SECRET_MASK.to_string();
            }
        }
    }
    Ok(sessions)
}

/// 查询单个会话连接状态
#[tauri::command]
pub async fn session_status(
    state: State<'_, CoreState>,
    name: String,
) -> Result<SessionStatus, String> {
    let ssh = state.ssh.clone();
    Ok(ssh.get_status(&name).await)
}

/// 新增会话（重名报错），密码加密后写回配置
#[tauri::command]
pub async fn add_session(state: State<'_, CoreState>, mut info: SessionInfo) -> Result<(), String> {
    info.password = persist_session_password(info.password.as_deref(), None)?;
    {
        let mut config = state.config.lock().await;
        if config.sessions.iter().any(|s| s.name == info.name) {
            return Err(format!("会话名称 {} 已存在", info.name));
        }
        config.sessions.push(info);
    }
    state.persist_sessions().await.map_err(|e| e.to_string())
}

/// 更新会话（旧名定位；重名但非自身报错），密码哨兵/留空保留旧密文，明文重新加密，成功写回配置。
/// 若修改了名称且旧名称处于连接状态，先断开旧连接，避免后端残留旧名会话。
#[tauri::command]
pub async fn update_session(
    state: State<'_, CoreState>,
    old_name: String,
    mut info: SessionInfo,
) -> Result<(), String> {
    {
        let mut config = state.config.lock().await;
        if config
            .sessions
            .iter()
            .any(|s| s.name == info.name && s.name != old_name)
        {
            return Err(format!("会话名称 {} 已存在", info.name));
        }
        if let Some(pos) = config.sessions.iter().position(|s| s.name == old_name) {
            let old_pw = config.sessions[pos].password.clone();
            info.password = persist_session_password(info.password.as_deref(), old_pw.as_deref())?;
            config.sessions[pos] = info.clone();
        } else {
            // 旧名不存在:显式报错,不再静默成功(前端会误以为已保存)
            return Err(format!("会话 {} 不存在(可能已被删除或改名)", old_name));
        }
    }
    if old_name != info.name {
        let ssh = state.ssh.clone();
        if ssh.get_status(&old_name).await != SessionStatus::Disconnected {
            ssh.disconnect(&old_name).await;
        }
        // 迁移文件面板跟踪目录,避免残留旧键
        state.fs_cwd.lock().await.remove(&old_name);
    }
    state.persist_sessions().await.map_err(|e| e.to_string())
}

/// 删除会话并断开连接，成功写回配置
#[tauri::command]
pub async fn delete_session(state: State<'_, CoreState>, name: String) -> Result<(), String> {
    {
        state.ssh.remove(&name).await;
        state.fs_cwd.lock().await.remove(&name);
    }
    {
        let mut config = state.config.lock().await;
        config.sessions.retain(|s| s.name != name);
    }
    state.persist_sessions().await.map_err(|e| e.to_string())
}

/// 后台连接会话：connect → open_shell(cols,rows) → set_active，结果发 "connection" 事件
#[tauri::command]
pub async fn connect_session(
    app: AppHandle,
    state: State<'_, CoreState>,
    name: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let info = {
        let config = state.config.lock().await;
        config
            .sessions
            .iter()
            .find(|s| s.name == name)
            .cloned()
            .ok_or_else(|| format!("会话 {} 不存在", name))?
    };
    let info = {
        let mut info = info;
        if info.password.is_some() {
            info.password = decrypt_session_password(info.password.as_deref())?;
        }
        info
    };
    let ssh = state.ssh.clone();
    if ssh.get_status(&name).await != SessionStatus::Disconnected {
        return Ok(());
    }
    // 双击守卫:已有连接任务进行中则忽略本次触发
    if !ssh.mark_connecting(&name).await {
        return Ok(());
    }
    tokio::spawn(async move {
        let result = async {
            let ok = ssh.connect(&info).await?;
            if !ok {
                anyhow::bail!("认证未通过");
            }
            if let Err(e) = ssh.open_shell(&info.name, cols, rows, info.kind).await {
                ssh.disconnect(&info.name).await;
                return Err(e);
            }
            ssh.set_active(&info.name).await;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let ev = match result {
            Ok(()) => ConnectionPayload::Connected { name: info.name },
            Err(e) => ConnectionPayload::Failed {
                name: info.name,
                error: e.to_string(),
            },
        };
        let _ = app.emit("connection", ev);
    });
    Ok(())
}

/// 断开指定会话
#[tauri::command]
pub async fn disconnect_session(state: State<'_, CoreState>, name: String) -> Result<(), String> {
    state.ssh.disconnect(&name).await;
    Ok(())
}

/// 设置当前活跃会话（切换标签时调用）
#[tauri::command]
pub async fn set_active(state: State<'_, CoreState>, name: String) -> Result<(), String> {
    state.ssh.set_active(&name).await;
    Ok(())
}

/// 清空活跃会话
#[tauri::command]
pub async fn clear_active(state: State<'_, CoreState>) -> Result<(), String> {
    state.ssh.clear_active().await;
    Ok(())
}

/// 远程桌面控屏(P33)：为 kind=rdp 会话生成临时 .rdp 文件并拉起系统 mstsc。
/// 密码不写入 .rdp（mstsc 交互式提示输入，避免明文落盘）。
#[tauri::command]
pub fn rdp_connect(state: State<'_, CoreState>, name: String) -> Result<(), String> {
    let info = {
        let config = state.config.blocking_lock();
        let info = config
            .sessions
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("会话 {} 不存在", name))?
            .clone();
        info
    };
    if info.kind != crate::config::SessionKind::Rdp {
        return Err(format!("会话 {} 不是远程桌面(RDP)会话", name));
    }
    let port = if info.port == 0 { 3389 } else { info.port };
    let user = info.user.trim();
    // 生成 mstsc 可读的临时 .rdp 文件（系统提示输入凭据）
    let mut rdp = String::new();
    rdp.push_str(&format!("full address:s:{}\n", info.host));
    rdp.push_str(&format!("server port:i:{}\n", port));
    rdp.push_str(&format!("prompt for credentials:i:1\n"));
    rdp.push_str("enablecredsspsupport:i:1\n");
    rdp.push_str("authentication level:i:2\n");
    rdp.push_str("screen mode id:i:1\n");
    rdp.push_str("session bpp:i:32\n");
    rdp.push_str("bitmapcachepersistenable:i:1\n");
    if !user.is_empty() {
        rdp.push_str(&format!("username:s:{}\n", user));
    }
    let dir = std::env::temp_dir();
    let path = dir.join(format!("helm-rdp-{}.rdp", std::process::id()));
    std::fs::write(&path, rdp).map_err(|e| format!("写入 .rdp 文件失败: {e}"))?;
    let _ = std::process::Command::new("mstsc")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("启动 mstsc 失败: {e}"))?;
    // mstsc 读取文件通常在启动后数秒内完成,延迟清理避免残留主机信息
    let del_path = path.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(30));
        let _ = std::fs::remove_file(del_path);
    });
    Ok(())
}

// ---------- 终端 commands ----------

/// 向指定会话发送输入字节
#[tauri::command]
pub async fn send_input(
    state: State<'_, CoreState>,
    name: String,
    data: Vec<u8>,
) -> Result<(), String> {
    state.ssh.send_input(&name, &data).await;
    Ok(())
}

/// 向活跃会话发送输入字节
#[tauri::command]
pub async fn send_active_input(state: State<'_, CoreState>, data: Vec<u8>) -> Result<(), String> {
    state.ssh.send_active_input(&data).await;
    Ok(())
}

/// 调整所有会话 PTY 尺寸
#[tauri::command]
pub async fn resize_sessions(
    state: State<'_, CoreState>,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state.ssh.send_resize_all(cols, rows).await;
    Ok(())
}

// ---------- AI commands ----------

/// 提交 AI 输入：按当前模式分发后台任务，结果经 "ai" 事件回传
///
/// `pwd`：前端持有的权威 PWD（OSC7），用作 Agent 任务的初始目录。
#[tauri::command]
pub async fn ai_submit(
    app: AppHandle,
    state: State<'_, CoreState>,
    input: String,
    pwd: Option<String>,
) -> Result<(), String> {
    // 原子抢占 busy 标记:并发提交只有一个能进入(修复 TOCTOU;
    // 早退路径必须复位,否则任务永远"忙")
    if state
        .ai_busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("AI 任务正在执行中，请先停止或等待".to_string());
    }
    let rollback = |state: &State<'_, CoreState>, app: &AppHandle| {
        state.ai_busy.store(false, Ordering::SeqCst);
        let _ = app.emit("ai", AiPayload::Busy { busy: false });
    };
    let mode = {
        let agent = state.agent.lock().await;
        agent.mode()
    };
    let input = input.trim().to_string();
    if input.is_empty() {
        rollback(&state, &app);
        return Ok(());
    }

    // Agent 模式锁定当前活动会话为任务执行目标（防中途切标签命令发去别的机器）
    let ctx = if mode == AgentMode::Agent {
        let ssh = state.ssh.clone();
        let name = ssh.active_name().await;
        let sessions = state.config.lock().await.sessions.clone();
        let Some(name) = name else {
            rollback(&state, &app);
            return Err("未连接会话：请先连接一个会话，再发起 Agent 任务".to_string());
        };
        let info = sessions.iter().find(|s| s.name == name);
        let (host, user) = match info {
            Some(info) => (info.host.clone(), info.user.clone()),
            None => (name.clone(), String::new()),
        };
        Some(TaskCtx {
            name,
            host,
            user,
            pwd: pwd.unwrap_or_default().trim().to_string(),
        })
    } else {
        None
    };

    let _ = app.emit("ai", AiPayload::Busy { busy: true });

    // 为本次任务建立控制通道
    let (ctl_tx, mut ctl_rx) = mpsc::unbounded_channel();
    {
        let mut ctl = state.ai_ctl.lock().await;
        *ctl = Some(ctl_tx);
    }

    let ssh = state.ssh.clone();
    let agent = state.agent.clone();
    let busy = state.ai_busy.clone();
    let ctl_slot = state.ai_ctl.clone();
    tokio::spawn(async move {
        run_ai_job(&agent, &ssh, &app, &mut ctl_rx, &input, mode, ctx).await;
        busy.store(false, Ordering::SeqCst);
        let _ = app.emit("ai", AiPayload::Busy { busy: false });
        // 清空控制通道槽位
        let mut ctl = ctl_slot.lock().await;
        *ctl = None;
    });
    Ok(())
}

/// AI 控制：批准/跳过待确认命令，或取消任务
#[tauri::command]
pub async fn ai_control(state: State<'_, CoreState>, action: String) -> Result<(), String> {
    let ctl = match action.as_str() {
        "approve" => AiControl::Approve,
        "reject" => AiControl::Reject,
        "cancel" => AiControl::Cancel,
        _ => return Err(format!("未知控制动作: {}", action)),
    };
    let ctl_slot = state.ai_ctl.lock().await;
    match ctl_slot.as_ref() {
        Some(tx) => tx.send(ctl).map_err(|_| "AI 任务已结束".to_string()),
        None => Err("AI 任务未在运行".to_string()),
    }
}

/// 中止当前 AI 任务
#[tauri::command]
pub async fn ai_stop(state: State<'_, CoreState>) -> Result<(), String> {
    {
        let ctl_slot = state.ai_ctl.lock().await;
        if let Some(tx) = ctl_slot.as_ref() {
            let _ = tx.send(AiControl::Cancel);
        }
    }
    {
        let mut agent = state.agent.lock().await;
        agent.reset_task();
        if agent.mode() == AgentMode::Agent {
            agent.clear_history();
        }
    }
    Ok(())
}

/// 清空 AI 对话历史（忙碌时拒绝）
#[tauri::command]
pub async fn ai_clear_history(state: State<'_, CoreState>) -> Result<(), String> {
    if state.ai_busy.load(Ordering::SeqCst) {
        return Err("请先停止当前 AI 任务".to_string());
    }
    let mut agent = state.agent.lock().await;
    agent.clear_history();
    Ok(())
}

/// 切换 AI 工作模式
#[tauri::command]
pub async fn ai_set_mode(state: State<'_, CoreState>, mode: String) -> Result<(), String> {
    if state.ai_busy.load(Ordering::SeqCst) {
        return Err("请先停止当前 AI 任务".to_string());
    }
    let mode = match mode.as_str() {
        "qa" => AgentMode::QA,
        "agent" => AgentMode::Agent,
        _ => return Err(format!("未知模式: {}", mode)),
    };
    {
        let mut agent = state.agent.lock().await;
        agent.set_mode(mode);
    }
    // 同步缓存,供 ai_mode 免锁查询
    state
        .ai_mode_agent
        .store(mode == AgentMode::Agent, Ordering::SeqCst);
    Ok(())
}

/// 查询当前 AI 模式(读缓存不碰 agent 锁,QA 聊天持锁期间也能即时返回)
#[tauri::command]
pub async fn ai_mode(state: State<'_, CoreState>) -> Result<String, String> {
    Ok(if state.ai_mode_agent.load(Ordering::SeqCst) {
        "agent".to_string()
    } else {
        "qa".to_string()
    })
}

// ---------- 配置 commands ----------

/// 读取 AI 配置（None 表示未配置）
/// API Key 密文不直接下发：有 key 时以哨兵 "·" 标识（前端仅用于显示"已保存"）
#[tauri::command]
pub async fn get_ai_config(
    state: State<'_, CoreState>,
) -> Result<Option<crate::config::AiConfig>, String> {
    let mut cfg = state.config.lock().await.ai.clone();
    if let Some(ai) = cfg.as_mut() {
        ai.api_key = match ai.api_key.as_ref().filter(|k| !k.is_empty()) {
            Some(_) => Some("·".to_string()),
            None => None,
        };
    }
    Ok(cfg)
}

/// 保存 AI 配置并重建 Agent（校验通过后持久化）
/// api_key 处理：空/哨兵 → 保留原密文；非空明文 → DPAPI 加密后落盘
#[tauri::command]
pub async fn update_ai_config(
    state: State<'_, CoreState>,
    mut config: crate::config::AiConfig,
) -> Result<(), String> {
    // busy 守卫:任务运行中替换 Agent 会与进行中的请求互踩(且会阻塞等 agent 锁)
    if state.ai_busy.load(Ordering::SeqCst) {
        return Err("AI 任务执行中，请先停止再修改配置".to_string());
    }
    {
        let cfg = state.config.lock().await;
        let old_cipher = cfg.ai.as_ref().and_then(|a| a.api_key.clone());
        match config.api_key.as_ref().map(|k| k.trim()).unwrap_or("") {
            "" | "·" => config.api_key = old_cipher,
            plain => config.api_key = Some(crate::crypto::encrypt_api_key(plain).map_err(|e| e.to_string())?),
        }
    }
    {
        let mut cfg = state.config.lock().await;
        cfg.ai = Some(config);
    }
    let agent = {
        let cfg = state.config.lock().await;
        let ai_cfg = cfg.ai.as_ref().ok_or_else(|| "AI 配置为空".to_string())?;
        Agent::new(ai_cfg).map_err(|e| e.to_string())?
    };
    *state.agent.lock().await = agent;
    let cfg = state.config.lock().await.clone();
    save_config(&state.config_path, &cfg).map_err(|e| e.to_string())
}

/// 以已保存 AI 配置为底，合并设置弹窗表单值（非空才覆盖；api_key 空/哨兵=沿用已存密文）
fn merged_ai_config(
    cfg: &crate::config::HelmConfig,
    model: Option<String>,
    api_base_url: Option<String>,
    api_key: Option<String>,
) -> crate::config::AiConfig {
    let mut out = cfg.ai.clone().unwrap_or_default();
    if let Some(m) = model {
        let t = m.trim().to_string();
        if !t.is_empty() {
            out.model = t;
        }
    }
    if let Some(u) = api_base_url {
        let t = u.trim().to_string();
        if !t.is_empty() {
            out.api_base_url = Some(t);
        }
    }
    if let Some(k) = api_key {
        let t = k.trim().to_string();
        if !t.is_empty() && t != crate::crypto::SECRET_MASK {
            out.api_key = Some(t);
        }
    }
    out
}

/// 测试 AI 连接（只读探测，不落盘、不重建 Agent、不受 busy 限制）
///
/// 表单值非空时覆盖已保存配置；api_key 传明文，空/哨兵表示沿用已保存值。
#[tauri::command]
pub async fn test_ai_connection(
    state: State<'_, CoreState>,
    model: Option<String>,
    api_base_url: Option<String>,
    api_key: Option<String>,
) -> Result<String, String> {
    let cfg = state.config.lock().await;
    Agent::test_connection(&merged_ai_config(&cfg, model, api_base_url, api_key)).await
}

/// 拉取提供商可用模型列表（GET /models，只读探测，规则同 test_ai_connection）
#[tauri::command]
pub async fn ai_list_models(
    state: State<'_, CoreState>,
    model: Option<String>,
    api_base_url: Option<String>,
    api_key: Option<String>,
) -> Result<Vec<String>, String> {
    let cfg = state.config.lock().await;
    Agent::list_models(&merged_ai_config(&cfg, model, api_base_url, api_key)).await
}

/// 读取 UI 配置（None 表示未配置，用默认值）
#[tauri::command]
pub async fn get_ui_config(
    state: State<'_, CoreState>,
) -> Result<crate::config::UiConfig, String> {
    let cfg = state.config.lock().await;
    Ok(cfg.ui.clone().unwrap_or_default())
}

/// 保存 UI 配置并应用窗口尺寸/位置
///
/// 前端设置弹窗不携带窗口位置字段（window_x/y 为 None），此处保留已有位置，
/// 位置由窗口移动事件记录、退出时统一落盘（见 main.rs）。
#[tauri::command]
pub async fn update_ui_config(
    app: AppHandle,
    state: State<'_, CoreState>,
    mut config: crate::config::UiConfig,
) -> Result<(), String> {
    {
        let mut cfg = state.config.lock().await;
        if let Some(existing) = cfg.ui.as_ref() {
            if config.window_x.is_none() {
                config.window_x = existing.window_x;
            }
            if config.window_y.is_none() {
                config.window_y = existing.window_y;
            }
        }
        cfg.ui = Some(config.clone());
    }
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.set_size(tauri::LogicalSize::new(
            config.window_width as f64,
            config.window_height as f64,
        ));
        if let (Some(x), Some(y)) = (config.window_x, config.window_y) {
            // 跳过离屏哨兵坐标（最小化遗留的 -32000 等）
            if x.abs() < 20000 && y.abs() < 20000 {
                let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
            }
        }
    }
    let cfg = state.config.lock().await.clone();
    save_config(&state.config_path, &cfg).map_err(|e| e.to_string())
}

// ---------- 后台任务 ----------


/// 启动终端输出推送任务：等 Notify 事件（shell 有输出/通道结束）→ drain 所有已连接
/// 会话 → emit "terminal-output"。同时检测连接意外断开（远端关闭/网络中断），
/// 发 "connection" Disconnected 事件，让前端状态圆点及时同步。
/// 事件驱动取代 50ms 忙轮询：空闲会话零唤醒。
pub fn spawn_output_poller(app: AppHandle, ssh: Arc<SshManager>) {
    tauri::async_runtime::spawn(async move {
        let mut last_connected: std::collections::HashMap<String, bool> =
            std::collections::HashMap::new();
        loop {
            // 等待任一会话产生输出或通道结束（notify_one 只在无人等待时存 1 个许可，
            // 唤醒后全量 drain，多次通知自然合并，不会丢数据）。
            // 附加 500ms 心跳：通道刚结束时 is_closed() 可能尚为 false，心跳兜底补检，
            // 使远端断开/用户 exit 的状态同步不受单次通知时序影响。
            let notify_fut = ssh.wait_output();
            tokio::pin!(notify_fut);
            tokio::select! {
                _ = &mut notify_fut => {}
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
            let names: Vec<String> = ssh.session_names().await;
            let active: std::collections::HashSet<String> = names.iter().cloned().collect();
            for name in &names {
                let drained = ssh.drain_output(&name).await;
                let is_connected = ssh.get_status(&name).await == SessionStatus::Connected;
                // 状态翻转：之前已连接，现在断开 → 通知前端
                if last_connected.get(name) == Some(&true) && !is_connected {
                    let _ = app.emit(
                        "connection",
                        ConnectionPayload::Disconnected { name: name.clone() },
                    );
                }
                last_connected.insert(name.clone(), is_connected);
                if let Some(data) = drained {
                    if !data.is_empty() {
                        let _ = app.emit(
                            "terminal-output",
                            TerminalOutputPayload { name: name.clone(), data },
                        );
                    }
                }
            }
            // 清理已移除会话的旧状态
            last_connected.retain(|k, _| active.contains(k));
        }
    });
}

/// 列出目录内容
#[tauri::command]
pub async fn fs_list_dir(
    state: State<'_, CoreState>,
    name: String,
    path: String,
) -> Result<Vec<crate::fs::FileEntry>, String> {
    crate::fs::list_dir(&state.ssh, &state.fs_cwd, &name, &path).await
}

/// 获取会话当前目录
#[tauri::command]
pub async fn fs_current_dir(
    state: State<'_, CoreState>,
    name: String,
) -> Result<String, String> {
    crate::fs::current_dir(&state.ssh, &state.fs_cwd, &name).await
}

/// 新建目录
#[tauri::command]
pub async fn fs_mkdir(
    state: State<'_, CoreState>,
    name: String,
    path: String,
) -> Result<crate::fs::FsResult, String> {
    crate::fs::mkdir(&state.ssh, &name, &path).await
}

/// 重命名/移动
#[tauri::command]
pub async fn fs_rename(
    state: State<'_, CoreState>,
    name: String,
    old_path: String,
    new_path: String,
) -> Result<crate::fs::FsResult, String> {
    crate::fs::rename(&state.ssh, &name, &old_path, &new_path).await
}

/// 删除文件或目录
#[tauri::command]
pub async fn fs_remove(
    state: State<'_, CoreState>,
    name: String,
    path: String,
) -> Result<crate::fs::FsResult, String> {
    crate::fs::remove(&state.ssh, &name, &path).await
}

/// 上传 base64 分块
#[tauri::command]
pub async fn fs_upload(
    state: State<'_, CoreState>,
    name: String,
    path: String,
    b64: String,
    append: bool,
) -> Result<crate::fs::FsResult, String> {
    crate::fs::upload(&state.ssh, &name, &path, &b64, append).await
}

/// 下载文件（返回 base64）
#[tauri::command]
pub async fn fs_download(
    state: State<'_, CoreState>,
    name: String,
    path: String,
) -> Result<crate::fs::FsResult, String> {
    crate::fs::download(&state.ssh, &name, &path).await
}

/// 读取文件内容用于预览
#[tauri::command]
pub async fn fs_read_file(
    state: State<'_, CoreState>,
    name: String,
    path: String,
    limit: u64,
) -> Result<crate::fs::FsResult, String> {
    crate::fs::read_file(&state.ssh, &name, &path, limit).await
}

// ---------- 主机密钥（TOFU） commands ----------

/// 遗忘指定主机已记录的公钥指纹（服务器密钥变更后手动重置）
#[tauri::command]
pub async fn forget_host_key(
    state: State<'_, CoreState>,
    host: String,
    port: u16,
) -> Result<bool, String> {
    let mut store = state.known_hosts.lock().await;
    Ok(store.forget(&host, port))
}

/// 查询指定主机已记录的公钥指纹（未记录返回 None）
#[tauri::command]
pub async fn host_key_fingerprint(
    state: State<'_, CoreState>,
    host: String,
    port: u16,
) -> Result<Option<String>, String> {
    let store = state.known_hosts.lock().await;
    Ok(store.fingerprint(&host, port).map(|s| s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_password_encrypts_on_persist() {
        let stored = persist_session_password(Some("000000"), None).unwrap();
        let stored = stored.expect("应生成密文");
        assert!(stored.starts_with(crate::crypto::SECRET_ENCRYPTED_PREFIX));
        assert!(!stored.contains("000000"));
        assert_eq!(
            decrypt_session_password(Some(&stored)).unwrap(),
            Some("000000".to_string())
        );
    }

    #[test]
    fn session_password_mask_or_empty_keeps_old() {
        assert_eq!(
            persist_session_password(Some(crate::crypto::SECRET_MASK), Some("enc:old")).unwrap(),
            Some("enc:old".to_string())
        );
        assert_eq!(
            persist_session_password(Some(""), Some("enc:old")).unwrap(),
            Some("enc:old".to_string())
        );
    }

    #[test]
    fn session_password_legacy_plaintext_passes_through() {
        assert_eq!(
            decrypt_session_password(Some("000000")).unwrap(),
            Some("000000".to_string())
        );
        assert_eq!(decrypt_session_password(None).unwrap(), None);
        assert_eq!(decrypt_session_password(Some("")).unwrap(), None);
    }

    #[test]
    fn session_password_none_clears() {
        assert_eq!(persist_session_password(None, Some("enc:old")).unwrap(), None);
    }
}
