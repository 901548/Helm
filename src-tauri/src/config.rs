// ============================================================================
// config.rs - 配置管理模块
// 负责 Helm 配置文件的加载与解析。配置查找顺序为：
//   1. 命令行参数指定的配置文件路径
//   2. 当前目录下的 config.yaml
//   3. 用户配置目录 ~/.config/helm/config.yaml
// ============================================================================

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// 会话平台类型（决定连接方式、shell 注入与功能分支）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    /// Linux/Unix 服务器（bash + POSIX 工具，默认）
    #[default]
    Linux,
    /// Windows 主机 OpenSSH Server（cmd/PowerShell 交互）
    Windows,
    /// Windows 远程桌面(RDP，经 mstsc 控屏)
    Rdp,
    /// 远端主机上的 Docker 容器（经 SSH 到主机，AI 命令用 docker exec 注入）
    Docker,
}

/// 一个 SSH 会话的配置信息
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionInfo {
    /// 会话名称（唯一标识）
    pub name: String,
    /// 会话平台类型：linux(默认) / windows / rdp
    #[serde(default)]
    pub kind: SessionKind,
    /// 服务器主机名或 IP 地址
    pub host: String,
    /// SSH 端口号，默认 22
    #[serde(default = "default_port")]
    pub port: u16,
    /// 登录用户名，默认 root
    #[serde(default = "default_user")]
    pub user: String,
    /// 私钥文件路径（可选，与 password 二选一）
    pub key_file: Option<String>,
    /// 登录密码（可选）
    pub password: Option<String>,
    /// Docker 容器名/ID（仅 kind=Docker 时使用：AI 命令经 docker exec 注入该容器）
    #[serde(default)]
    pub container: Option<String>,
}

/// 默认 SSH 端口
fn default_port() -> u16 {
    22
}

/// 默认登录用户名
fn default_user() -> String {
    "root".to_string()
}

/// AI Agent 的配置信息
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AiConfig {
    /// 使用的模型名称（如 deepseek-chat / gpt-4o-mini）
    pub model: String,
    /// 保存 API Key 的环境变量名（回退路径，api_key 优先）
    pub api_key_env: String,
    /// DPAPI 加密后的 API Key（base64 密文，优先于 api_key_env）
    #[serde(default)]
    pub api_key: Option<String>,
    /// 兼容 OpenAI 格式的 API 基础地址（可选，用于 DeepSeek 等）
    pub api_base_url: Option<String>,
    /// 系统提示词
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    /// 采样温度
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// 最大生成 token 数（可选）
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// 是否启用流式输出（不支持时自动回退非流式）
    #[serde(default = "default_stream")]
    pub stream: bool,
    /// 对话历史最大条数（含 system，超出后裁剪最旧消息）
    #[serde(default = "default_max_history")]
    pub max_history: usize,
    /// Agent 模式最大执行步数
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
    /// Agent 模式每步命令输出回传给模型的字符上限
    #[serde(default = "default_max_output_chars")]
    pub max_output_chars: usize,
    /// 单次 API 请求超时秒数
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Agent 模式执行命令前是否需人工确认（危险命令始终强制确认；默认 false=仅危险命令确认）
    #[serde(default = "default_agent_confirm")]
    pub agent_confirm: bool,
    /// 初始工作模式：qa（问答）或 agent（代理执行）
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Agent 模式单条命令执行超时（秒，None 时用默认 60）
    #[serde(default)]
    pub command_timeout_secs: Option<u64>,
    /// Agent 模式专用系统提示词（None 时回退 system_prompt）
    #[serde(default)]
    pub system_prompt_agent: Option<String>,
    /// 附加请求头
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
    /// 附加请求体字段（合并进 chat/completions body）
    #[serde(default)]
    pub extra_body: serde_json::Value,
}

impl Default for AiConfig {
    /// 空配置的默认值（AI 未配置时的兜底）
    fn default() -> Self {
        Self {
            model: String::new(),
            api_key_env: String::new(),
            api_key: None,
            api_base_url: None,
            system_prompt: default_system_prompt(),
            temperature: default_temperature(),
            max_tokens: None,
            stream: default_stream(),
            max_history: default_max_history(),
            max_steps: default_max_steps(),
            max_output_chars: default_max_output_chars(),
            timeout_secs: default_timeout_secs(),
            agent_confirm: default_agent_confirm(),
            mode: default_mode(),
            command_timeout_secs: None,
            system_prompt_agent: None,
            extra_headers: HashMap::new(),
            extra_body: serde_json::Value::Null,
        }
    }
}

/// 默认采样温度
fn default_temperature() -> f32 {
    0.3
}

/// 默认开启流式输出
fn default_stream() -> bool {
    true
}

/// 默认历史条数
fn default_max_history() -> usize {
    30
}

/// 默认最大执行步数
fn default_max_steps() -> u32 {
    50
}

/// 默认命令输出回传字符上限
fn default_max_output_chars() -> usize {
    6000
}

/// 默认 API 超时秒数
fn default_timeout_secs() -> u64 {
    60
}

/// 默认命令执行前需人工确认（false：普通命令自动执行，危险命令仍强制确认）
fn default_agent_confirm() -> bool {
    false
}

/// 默认初始工作模式（问答）
fn default_mode() -> String {
    "qa".to_string()
}

/// 默认的系统提示词
fn default_system_prompt() -> String {
    "你是一个专业的运维AI助手，负责在远程Linux服务器上执行任务。\
     你只能输出需要执行的shell命令，命令之间用换行分隔。\
     如果任务完成，只输出 DONE。\
     不要输出解释、不要使用代码围栏。"
        .to_string()
}

/// 界面主题模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// 白天模式（白底黑字）
    Light,
    /// 黑夜模式（黑底主体）
    Dark,
    /// 跟随系统
    System,
}

/// 界面/窗口相关配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UiConfig {
    /// 窗口默认宽度（逻辑像素）
    #[serde(default = "default_window_width")]
    pub window_width: u32,
    /// 窗口默认高度（逻辑像素）
    #[serde(default = "default_window_height")]
    pub window_height: u32,
    /// 窗口水平位置（物理像素，P26：None 表示未记录，启动时居中）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_x: Option<i32>,
    /// 窗口垂直位置（物理像素，P26：None 表示未记录，启动时居中）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_y: Option<i32>,
    /// 左侧会话面板是否显示
    #[serde(default = "default_dock_sessions")]
    pub dock_sessions: bool,
    /// 右侧 AI 面板是否显示
    #[serde(default = "default_dock_chat")]
    pub dock_chat: bool,
    /// 会话面板宽度占比（百分比）
    #[serde(default = "default_sessions_panel_pct")]
    pub sessions_panel_pct: u16,
    /// 聊天面板高度占比（百分比）
    #[serde(default = "default_chat_panel_pct")]
    pub chat_panel_pct: u16,
    /// 主题模式
    #[serde(default = "default_theme")]
    pub theme: Theme,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            window_width: default_window_width(),
            window_height: default_window_height(),
            window_x: None,
            window_y: None,
            dock_sessions: default_dock_sessions(),
            dock_chat: default_dock_chat(),
            sessions_panel_pct: default_sessions_panel_pct(),
            chat_panel_pct: default_chat_panel_pct(),
            theme: default_theme(),
        }
    }
}

/// 默认窗口宽度
fn default_window_width() -> u32 {
    1200
}

/// 默认窗口高度
fn default_window_height() -> u32 {
    800
}

/// 默认显示会话面板
fn default_dock_sessions() -> bool {
    true
}

/// 默认显示 AI 面板
fn default_dock_chat() -> bool {
    true
}

/// 默认会话面板宽度占比
fn default_sessions_panel_pct() -> u16 {
    22
}

/// 默认聊天面板高度占比
fn default_chat_panel_pct() -> u16 {
    28
}

/// 默认主题（黑夜）
fn default_theme() -> Theme {
    Theme::Dark
}

/// Helm 顶层配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HelmConfig {
    /// 所有 SSH 会话配置
    #[serde(default)]
    pub sessions: Vec<SessionInfo>,
    /// AI Agent 配置（可选）
    #[serde(default)]
    pub ai: Option<AiConfig>,
    /// 界面/窗口配置（可选）
    #[serde(default)]
    pub ui: Option<UiConfig>,
}

impl Default for HelmConfig {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            ai: None,
            ui: None,
        }
    }
}

/// 按优先级查找并加载配置文件
///
/// 查找顺序：命令行参数路径 > ./config.yaml > ~/.config/helm/config.yaml。
/// 若全部不存在，则返回包含查找路径的友好错误信息。
#[allow(dead_code)]
pub fn load_config(cli_path: Option<&str>) -> Result<HelmConfig> {
    let (_, config) = load_config_with_path(cli_path)?;
    Ok(config)
}

/// 按优先级查找并加载配置文件，同时返回实际使用的路径（供保存时回写）
pub fn load_config_with_path(cli_path: Option<&str>) -> Result<(PathBuf, HelmConfig)> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = cli_path {
        candidates.push(expand_tilde(path));
    }
    candidates.push(PathBuf::from("./config.yaml"));
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".config").join("helm").join("config.yaml"));
    }

    for path in &candidates {
        if path.is_file() {
            let content = std::fs::read_to_string(path)?;
            let config: HelmConfig = serde_yaml::from_str(&content)?;
            return Ok((path.clone(), config));
        }
    }

    let searched = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n  - ");
    bail!(
        "未找到配置文件，已按以下顺序查找（均不存在）：\n  - {}\n\n\
         请创建 config.yaml 后重试，或使用 `helm <配置文件路径>` 指定自定义配置。",
        searched
    );
}

/// 将配置写回指定的 YAML 文件
pub fn save_config(path: &Path, config: &HelmConfig) -> Result<()> {
    let content = serde_yaml::to_string(config)?;
    // 原子写入:先写同目录唯一临时文件再 rename 替换,避免写一半崩溃损坏 config.yaml
    // (会丢掉全部会话含 DPAPI 密文密码)。临时文件用 进程ID+序号 保证唯一,并发保存
    // (persist_sessions 锁外 save、update_* 锁内 save)不再双写同一 tmp 相互踩踏;
    // 崩溃残留至多一个唯一 tmp,rename 对已存在目标等同替换(Windows 亦然)。
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nonce = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("yaml.tmp-{}-{}", std::process::id(), nonce));
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 展开路径中的 `~` 为当前用户主目录
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}
