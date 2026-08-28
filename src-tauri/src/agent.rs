// ============================================================================
// agent.rs - AI Agent 引擎模块
// 通过兼容 OpenAI Chat Completions 格式的 HTTP API 提供两种工作模式：
//   QA    - 问答模式，直接回答用户问题
//   Agent - 代理模式，逐步决策并执行 shell 命令完成任务
// 支持流式输出（SSE），失败时自动回退非流式；非流式请求失败自动重试 1 次。
// ============================================================================

use std::env;
use std::time::Duration;

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use serde_json::json;

use crate::config::AiConfig;
use crate::crypto;

/// 请求体内由本模块按配置组装的核心字段，`extra_body` 合并时跳过，防误覆盖
const RESERVED_BODY_KEYS: [&str; 5] = ["model", "messages", "stream", "temperature", "max_tokens"];

/// 单条聊天消息
#[derive(Debug, Clone)]
pub struct ChatMessage {
    /// 消息角色：system / user / assistant
    pub role: String,
    /// 消息内容
    pub content: String,
}

impl ChatMessage {
    /// 构造一条用户消息
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }

    /// 构造一条助手消息
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

/// Agent 工作模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    /// 问答模式
    QA,
    /// 代理执行模式
    Agent,
}

/// 流式事件：正文增量 或 思考过程增量（推理型模型如 deepseek-r1 的 reasoning_content）
#[derive(Debug, Clone)]
pub enum AiStreamEvent {
    /// 最终回答内容（计入结果，参与命令解析）
    Content(String),
    /// 思考过程（仅供 UI 展示，不计入结果，避免污染命令解析）
    Reasoning(String),
}

/// AI Agent 引擎
pub struct Agent {
    /// 完整配置（模型、API、流式、历史裁剪等）
    config: AiConfig,
    /// API Key
    api_key: String,
    /// 复用 HTTP 客户端(连接池/TLS 会话复用,避免每步重建握手)
    client: reqwest::Client,
    /// 对话历史（首条始终为 system prompt）
    history: Vec<ChatMessage>,
    /// 当前工作模式
    mode: AgentMode,
    /// 当前任务名（Agent 模式下记录）
    current_task: Option<String>,
    /// QA 模式 system prompt（原 system_prompt）
    system_prompt_default: String,
    /// Agent 模式 system prompt（回退 system_prompt_default）
    system_prompt_agent: String,
}

/// 判断 base_url 是否指向本机（Ollama/LM Studio 等本地推理服务无需 API Key）
fn is_local_base(base: Option<&str>) -> bool {
    let Some(url) = base.map(str::trim).filter(|u| !u.is_empty()) else {
        return false;
    };
    if url.contains("[::1]") {
        return true;
    }
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split(&['/', ':'][..])
        .next()
        .unwrap_or("");
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0" | "::1")
}

/// 解析 GET /models 响应（OpenAI 格式 {"data":[{"id":"..."}]}），返回排序后的模型 id
fn parse_models_response(text: &str) -> Result<Vec<String>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("响应解析失败: {e}"))?;
    let Some(data) = value.get("data").and_then(|d| d.as_array()) else {
        return Err("响应缺少 data 数组（该服务可能不支持 /models 端点）".into());
    };
    let mut models: Vec<String> = data
        .iter()
        .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
        .map(str::to_string)
        .filter(|id| !id.is_empty())
        .collect();
    models.sort();
    Ok(models)
}

impl Agent {
    /// 从配置创建 Agent，API Key 按 api_key > api_key_env > 本地服务免 Key 顺序解析
    pub fn new(config: &AiConfig) -> Result<Self> {
        if config.model.is_empty() {
            return Err(anyhow!("未配置 AI 模型（ai.model）"));
        }
        let api_key = Self::resolve_api_key(config)?;
        let mut history = Vec::new();
        let system_prompt_default = config.system_prompt.clone();
        let system_prompt_agent = config
            .system_prompt_agent
            .clone()
            .unwrap_or_else(default_system_prompt_agent);
        let mode = if config.mode.eq_ignore_ascii_case("agent") {
            AgentMode::Agent
        } else {
            AgentMode::QA
        };
        history.push(ChatMessage {
            role: "system".into(),
            content: system_prompt_for(mode, &system_prompt_default, &system_prompt_agent),
        });
        let client = Self::build_client(config)?;
        Ok(Self {
            config: config.clone(),
            api_key,
            client,
            history,
            mode,
            current_task: None,
            system_prompt_default,
            system_prompt_agent,
        })
    }

    /// 问答模式：发送用户输入，流式增量推送增量文本，返回完整回复
    pub async fn chat(
        &mut self,
        user_input: &str,
        sink: Option<&mut (dyn FnMut(AiStreamEvent) + Send)>,
    ) -> Result<String> {
        self.ensure_ready()?;
        self.push_message(ChatMessage::user(user_input));
        let reply = self.call_api(sink).await?;
        self.push_message(ChatMessage::assistant(&reply));
        Ok(reply)
    }

    /// Agent 模式：逐步推进任务
    ///
    /// - 首次调用：记录 current_task，向模型描述任务并返回第一步命令
    /// - 后续调用：把上一步输出作为上下文，返回下一步命令或 "DONE"
    /// - `ctx`：当前执行环境描述（如 `会话 1 (root@192.168.79.150)，当前目录 /opt`）
    /// - `progress`：已完成/失败步骤的工作记忆账本，注入本次输入（防长任务被 max_history 裁剪后遗忘目标）
    pub async fn agent_step(
        &mut self,
        task: &str,
        prev_output: &str,
        ctx: &str,
        progress: &str,
        sink: Option<&mut (dyn FnMut(AiStreamEvent) + Send)>,
    ) -> Result<String> {
        self.ensure_ready()?;
        // 任务目标 + 步骤账本作为"工作记忆"：即使聊天历史被 max_history 裁剪，
        // 仍保留已完成/失败的决策事实，引导模型不重做已完成步骤、只重规划失败子集。
        let progress_block = if progress.trim().is_empty() {
            "（尚未执行任何步骤）".to_string()
        } else {
            format!("已完成/失败步骤账本：\n{}", progress.trim())
        };
        let user_msg = match &self.current_task {
            None => {
                self.current_task = Some(task.to_string());
                format!(
                    "当前环境：{}\n新任务：{}\n{}\n\
                     对复杂长任务，请先把它拆成若干子目标，每个子目标一行 `GOAL <序号> <短标题>`（如 `GOAL 1. 预检环境`），\
                     然后接着输出当前第一个子目标要执行的 shell 命令，只输出命令本身，不要解释。简单任务可跳过 GOAL 行直接给命令。\n\
                     输出纪律：GOAL 行单独一行，每行至多一条命令；除 GOAL/REFLEXION/DONE/命令行外不要输出任何解释文字。\n\
                     禁止使用实时跟随类参数（tail -f、journalctl -f、docker logs -f 等），命令必须能自然退出；查看最近日志用 -n/--since 限量读取。",
                    ctx.trim(),
                    task,
                    progress_block
                )
            }
            Some(_) => {
                format!(
                    "当前目录：{}\n{}\n这是上一步命令的执行输出：\n{}\n\
                     请判断下一步要执行的 shell 命令，只输出命令本身；如果任务已完成则只输出 DONE。\
                     当前子目标完成后输出 `GOAL_OK` 再给下一个子目标的命令；\
                     某子目标命令失败时，先输出一行 `REFLEXION 失败原因与对策`，再重规划该子目标、继续输出修正后的命令，不要重复重试已完成或已失败的步骤。\n\
                     输出纪律：每行至多一条命令，不要输出解释文字；禁止实时跟随类参数（-f/--follow），命令必须能自然退出。",
                    ctx.trim(),
                    progress_block,
                    prev_output
                )
            }
        };
        self.push_message(ChatMessage::user(user_msg));
        let reply = self.call_api(sink).await?;
        self.push_message(ChatMessage::assistant(&reply));

        let cleaned = clean_response(&reply);
        if contains_done_line(&cleaned) {
            self.current_task = None;
            Ok("DONE".to_string())
        } else {
            Ok(cleaned)
        }
    }

    /// Agent 模式最大执行步数
    pub fn max_steps(&self) -> u32 {
        self.config.max_steps.max(1)
    }

    /// Agent 模式命令输出回传上限
    pub fn max_output_chars(&self) -> usize {
        self.config.max_output_chars
    }

    /// Agent 模式是否每步都需人工确认
    #[allow(dead_code)]
    pub fn agent_confirm(&self) -> bool {
        self.config.agent_confirm
    }

    /// Agent 模式单条命令执行超时（秒）
    pub fn command_timeout_secs(&self) -> u64 {
        self.config.command_timeout_secs.filter(|&t| t > 0).unwrap_or(60)
    }

    /// 清空历史，仅保留第一条 system prompt（按当前模式重建）
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.history.push(ChatMessage {
            role: "system".into(),
            content: system_prompt_for(self.mode, &self.system_prompt_default, &self.system_prompt_agent),
        });
    }

    /// 重置当前任务（中断时调用）
    pub fn reset_task(&mut self) {
        self.current_task = None;
    }

    /// 切换工作模式
    pub fn set_mode(&mut self, mode: AgentMode) {
        self.mode = mode;
        // 模式切换后重建 system prompt，使 QA/Agent 用各自的提示词
        if let Some(system) = self.history.first_mut() {
            system.content = system_prompt_for(mode, &self.system_prompt_default, &self.system_prompt_agent);
        }
    }

    /// 当前工作模式
    pub fn mode(&self) -> AgentMode {
        self.mode
    }

    /// 返回对话历史
    #[allow(dead_code)]
    pub fn history(&self) -> &[ChatMessage] {
        &self.history
    }

    /// 追加消息并按 max_history 裁剪（始终保留首条 system）
    fn push_message(&mut self, msg: ChatMessage) {
        self.history.push(msg);
        let max = self.config.max_history.max(2);
        if self.history.len() > max {
            let remove = self.history.len() - max;
            self.history.drain(1..1 + remove);
        }
    }

    /// 检查模型与 API Key 是否已配置
    fn ensure_ready(&self) -> Result<()> {
        if self.config.model.is_empty() || self.api_key.is_empty() {
            return Err(anyhow!(
                "AI 未配置：请检查 config.yaml 的 ai 配置，并确保环境变量中设置了 API Key"
            ));
        }
        Ok(())
    }

    /// 调用 Chat Completions API
    ///
    /// 传入 sink 且配置开启流式时使用 SSE 增量输出；流式请求在未收到任何内容
    /// 前失败时自动回退到非流式（非流式自带 1 次重试）。
    async fn call_api(&self, sink: Option<&mut (dyn FnMut(AiStreamEvent) + Send)>) -> Result<String> {
        if self.config.stream {
            if let Some(sink) = sink {
                let mut started = false;
                match self.call_api_stream(sink, &mut started).await {
                    Ok(full) => return Ok(full),
                    Err(first) => {
                        if started {
                            return Err(first);
                        }
                        return self.call_api_plain().await;
                    }
                }
            }
        }
        self.call_api_plain().await
    }

    /// 非流式请求：一次取回完整回复，失败重试 1 次（间隔 1 秒）
    async fn call_api_plain(&self) -> Result<String> {
        let client = self.client.clone();
        let url = self.request_url();
        let body = self.request_body(false);

        let request_once = || async {
            let mut req = client.post(&url).json(&body);
            if !self.api_key.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", self.api_key));
            }
            let resp = req.send().await?;
            let status = resp.status();
            let text = resp.text().await?;
            if !status.is_success() {
                return Err(anyhow!("API 请求失败（HTTP {}）: {}", status, text));
            }
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| anyhow!("API 响应解析失败: {}", e))?;
            value["choices"][0]["message"]["content"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow!("API 响应缺少 choices[0].message.content: {}", text))
        };

        match request_once().await {
            Ok(reply) => Ok(reply),
            Err(first) => {
                tokio::time::sleep(Duration::from_secs(1)).await;
                request_once().await.map_err(|second| {
                    anyhow!("AI API 调用失败（已重试1次）: {}；{}", first, second)
                })
            }
        }
    }

    /// 流式请求：读取 SSE 增量，逐个回调 sink，返回完整文本
    ///
    /// `started` 标记是否已发出至少一段内容（用于判断能否安全回退非流式）。
    /// 流内 `{"error":{...}}` 数据行（200 后中途错误）会被识别为流式错误并返回 Err，
    /// 由调用方按 started 语义决定回退非流式或直接报错。
    async fn call_api_stream(
        &self,
        sink: &mut (dyn FnMut(AiStreamEvent) + Send),
        started: &mut bool,
    ) -> Result<String> {
        let client = self.client.clone();
        let url = self.request_url();
        let body = self.request_body(true);

        let mut req = client.post(&url).json(&body);
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await?;
            return Err(anyhow!("API 请求失败（HTTP {}）: {}", status, text));
        }

        let mut stream = resp.bytes_stream();
        let mut full = String::new();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow!("读取响应流失败: {}", e))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            loop {
                match buffer.find('\n') {
                    Some(pos) => {
                        let line = buffer[..pos].to_string();
                        buffer.drain(..=pos);
                        match parse_sse_line(&line) {
                            SseEvent::Content(delta) => {
                                *started = true;
                                sink(AiStreamEvent::Content(delta.clone()));
                                full.push_str(&delta);
                            }
                            SseEvent::Reasoning(delta) => {
                                // 思考过程只推给 UI，不写入 full（防污染命令解析/答案）
                                sink(AiStreamEvent::Reasoning(delta));
                            }
                            SseEvent::Done => return Ok(full),
                            SseEvent::Error(msg) => {
                                return Err(anyhow!("API 流式错误: {}", msg));
                            }
                            SseEvent::Ignore => {}
                        }
                    }
                    None => break,
                }
            }
        }
        Ok(full)
    }

    /// 组装请求 URL
    fn request_url(&self) -> String {
        let base = self
            .config
            .api_base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1");
        format!("{}/chat/completions", base.trim_end_matches('/'))
    }

    /// 组装请求体（合并配置中的 extra_body 字段）
    fn request_body(&self, stream: bool) -> serde_json::Value {
        let mut body = json!({
            "model": self.config.model,
            "messages": self.history.iter().map(|m| json!({
                "role": m.role,
                "content": m.content,
            })).collect::<Vec<_>>(),
            "temperature": self.config.temperature,
            "stream": stream,
        });
        if let Some(mt) = self.config.max_tokens {
            body["max_tokens"] = json!(mt);
        }
        if let serde_json::Value::Object(map) = &self.config.extra_body {
            for (k, v) in map {
                // 核心字段（本函数已按配置组装）不允许被 extra_body 覆盖，
                // 防止用户误配 `stream:false`/`model:"x"` 等静默破坏行为。
                if !RESERVED_BODY_KEYS.contains(&k.as_str()) {
                    body[k] = v.clone();
                }
            }
        }
        body
    }

    /// 解析 API Key：api_key 字段（密文→解密，明文→原样）> 环境变量 > 本地服务免 Key（空串）
    fn resolve_api_key(config: &AiConfig) -> Result<String> {
        if let Some(k) = config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
        {
            // 字段值可能是 DPAPI 密文（配置落盘的），也可能是明文（测试连接直传/手改配置）
            return Ok(crypto::decrypt_api_key(k).unwrap_or_else(|_| k.to_string()));
        }
        if !config.api_key_env.is_empty() {
            if let Ok(v) = env::var(&config.api_key_env) {
                if !v.trim().is_empty() {
                    return Ok(v);
                }
            }
        }
        if is_local_base(config.api_base_url.as_deref()) {
            return Ok(String::new());
        }
        let env_name = if config.api_key_env.is_empty() {
            "API_KEY"
        } else {
            &config.api_key_env
        };
        Err(anyhow!("未配置 API Key（请填写 API Key 或设置环境变量 {env_name}）"))
    }

    /// 测试 AI 连接（设置弹窗「测试连接」按钮）
    ///
    /// 只读探测：用最小请求（一条消息 + max_tokens=1）验证 base_url / api_key / model
    /// 是否可用，成功返回确认信息。不修改状态、不落盘。
    /// 超时沿用 timeout_secs（下限 20 秒）：本地推理服务冷启动可能超过 15 秒。
    pub async fn test_connection(config: &AiConfig) -> Result<String, String> {
        if config.model.trim().is_empty() {
            return Err("请先填写模型名称".into());
        }
        let key = Self::resolve_api_key(config).map_err(|e| e.to_string())?;
        let mut probe_cfg = config.clone();
        probe_cfg.timeout_secs = config.timeout_secs.max(20);
        let client = Self::build_client(&probe_cfg).map_err(|e| e.to_string())?;
        let base = config
            .api_base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1");
        let url = format!("{}/chat/completions", base.trim_end_matches('/'));
        let body = json!({
            "model": config.model,
            "messages": [{ "role": "user", "content": "hi" }],
            "max_tokens": 1,
            "stream": false,
        });
        let mut req = client.post(&url).json(&body);
        if !key.is_empty() {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let resp = req.send().await.map_err(|e| format!("请求失败: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let brief: String = text.chars().take(300).collect();
            return Err(format!("HTTP {status}: {brief}"));
        }
        // 多数 OpenAI 兼容服务会回显实际使用的 model 名
        let model_echo = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["model"].as_str().map(str::to_string));
        match model_echo {
            Some(m) => Ok(format!("连接成功（{m}）")),
            None => Ok("连接成功".into()),
        }
    }

    /// 拉取提供商可用模型列表（设置弹窗「获取模型列表」）
    ///
    /// GET {base}/models（OpenAI 兼容标准端点），返回排序后的模型 id 列表。
    /// 只读探测，超时/鉴权与 test_connection 同规则。
    pub async fn list_models(config: &AiConfig) -> Result<Vec<String>, String> {
        let key = Self::resolve_api_key(config).map_err(|e| e.to_string())?;
        let mut probe_cfg = config.clone();
        probe_cfg.timeout_secs = config.timeout_secs.max(20);
        let client = Self::build_client(&probe_cfg).map_err(|e| e.to_string())?;
        let base = config
            .api_base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1");
        let url = format!("{}/models", base.trim_end_matches('/'));
        let mut req = client.get(&url);
        if !key.is_empty() {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let resp = req.send().await.map_err(|e| format!("请求失败: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let brief: String = text.chars().take(300).collect();
            return Err(format!("HTTP {status}: {brief}"));
        }
        let models = parse_models_response(&text)?;
        if models.is_empty() {
            return Err("服务未返回任何模型".into());
        }
        Ok(models)
    }

    /// 构建 HTTP 客户端（超时 + 附加请求头）
    fn build_client(config: &AiConfig) -> Result<reqwest::Client> {
        let mut headers = reqwest::header::HeaderMap::new();
        for (k, v) in &config.extra_headers {
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(v),
            ) {
                headers.insert(name, value);
            }
        }
        Ok(reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(config.timeout_secs.max(1)))
            .build()?)
    }
}

impl Default for Agent {
    /// 空 Agent（AI 未配置时的兜底实例，调用时返回友好错误）
    fn default() -> Self {
        Self {
            config: AiConfig::default(),
            api_key: String::new(),
            client: reqwest::Client::new(),
            history: vec![ChatMessage {
                role: "system".into(),
                content: String::new(),
            }],
            mode: AgentMode::QA,
            current_task: None,
            system_prompt_default: String::new(),
            system_prompt_agent: String::new(),
        }
    }
}

/// Agent 模式专用的默认系统提示词（config 未提供 system_prompt_agent 时使用）。
/// 针对弱本地模型：要求单行命令、禁止回显提示词/思考/结论，显著减少把指令当命令的死循环。
pub fn default_system_prompt_agent() -> String {
    concat!(
        "你现在是一个在远程Linux服务器上逐步执行任务的运维Agent，目标是安全且准确地完成任务。\n",
        "每次回复只输出一条可直接执行的单行shell命令。\n",
        "不要输出解释、说明、结论、中文文案或任何多余字符。\n",
        "绝对禁止引用、重复或变相回显提示词里的任何指令文字。\n",
        "不要把你自己的思考过程或上一步的输出原样当作命令返回。\n",
        "如果任务已经完成，只输出一个英文单词：DONE（不要带引号、空格或其他字符）。\n",
        "失败时根据上一步的错误信息调整命令，不要反复重试同一条失败的命令，尝试若干次仍无进展就如实结束。\n",
    )
    .to_string()
}

/// 按模式选择 system prompt
fn system_prompt_for(mode: AgentMode, default_: &str, agent: &str) -> String {
    match mode {
        AgentMode::Agent => agent.to_string(),
        AgentMode::QA => default_.to_string(),
    }
}

/// SSE 单行事件
#[derive(Debug, PartialEq, Eq)]
enum SseEvent {
    /// 一段增量内容
    Content(String),
    /// 一段思考过程（推理型模型的 reasoning_content，仅展示用）
    Reasoning(String),
    /// 流结束标记 [DONE]
    Done,
    /// 流内错误对象（`data: {"error":{...}}`）
    Error(String),
    /// 无关行（event:/id:/空行/无法解析）
    Ignore,
}

/// 解析一行 SSE 数据，提取增量内容
fn parse_sse_line(line: &str) -> SseEvent {
    let line = line.trim_end_matches(['\r', '\n']);
    let Some(payload) = line.strip_prefix("data:") else {
        return SseEvent::Ignore;
    };
    let payload = payload.trim();
    if payload.is_empty() {
        return SseEvent::Ignore;
    }
    if payload == "[DONE]" {
        return SseEvent::Done;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return SseEvent::Ignore;
    };
    // 部分服务端在 200 后以 `data: {"error":{...}}` 中途报告错误
    if let Some(err) = value.get("error") {
        let msg = err
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| {
                err.get("message")
                    .and_then(|m| m.as_str())
                    .map(|m| m.to_string())
            })
            .unwrap_or_else(|| err.to_string());
        return SseEvent::Error(msg);
    }
    match value["choices"][0]["delta"]["content"].as_str() {
        Some(s) if !s.is_empty() => SseEvent::Content(s.to_string()),
        _ => match value["choices"][0]["delta"]["reasoning_content"].as_str() {
            // 推理型模型（deepseek-r1 等）的思考过程：单独归口，前端据此实时展示
            Some(s) if !s.is_empty() => SseEvent::Reasoning(s.to_string()),
            _ => SseEvent::Ignore,
        },
    }
}

/// 清理模型回复：去除首尾任意层代码围栏并修剪空白
fn clean_response(resp: &str) -> String {
    let lines: Vec<&str> = resp.trim().lines().collect();
    let mut start = 0;
    while start < lines.len() && lines[start].trim_start().starts_with("```") {
        start += 1;
    }
    let mut end = lines.len();
    while end > start && lines[end - 1].trim_end().ends_with("```") {
        end -= 1;
    }
    lines[start..end].join("\n").trim().to_string()
}

/// 去除命令行常见前缀
fn strip_cmd_prefix(line: &str) -> &str {
    for p in ["命令：", "命令:", "执行：", "执行:"] {
        if let Some(rest) = line.strip_prefix(p) {
            return rest.trim();
        }
    }
    line
}

/// 判定一行是否像一条可直接执行的 shell 命令。
///
/// 弱本地模型（deepseek-r1 等）常把系统提示词回显出来（如
/// 「如果任务完成，请输出 "DONE"」「只输出 DONE」）或输出结论性散文，
/// 这类行不应被当作 shell 命令执行，否则会陷入"命令找不到"的死循环。
pub fn is_command_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    let upper = t.to_ascii_uppercase();
    // 提示词回显句式：指令被模型原样吐出
    if t.contains("如果任务完成") || t.contains("如果任务已完成") {
        return false;
    }
    if upper.contains("请输出") && upper.contains("DONE") {
        return false;
    }
    if (t.contains("只输出") || t.contains("不要输出") || t.contains("不要解释"))
        && upper.contains("DONE")
    {
        return false;
    }
    // 真命令至少含一个 ASCII 命令词元（剔除纯标点/纯中文散文行）
    let has_word = t.bytes().any(|b| b.is_ascii_graphic() && !b.is_ascii_digit());
    if !has_word {
        return false;
    }
    // P57 L2 协议标记：GOAL/REFLEXION/EXPECT 开头的是子目标/反思/校验指令,不是 shell 命令
    let pfx = |p: &str| upper.starts_with(p);
    if !(pfx("GOAL ") == false && pfx("GOAL:") == false && pfx("GOAL_") == false && upper != "GOAL"
        && pfx("REFLEXION ") == false && pfx("REFLEXION:") == false
        && pfx("EXPECT ") == false && pfx("EXPECT:") == false)
    {
        return false;
    }
    // P68-§8.6 弱模型散文过滤：shell 命令必以 ASCII 程序名开头（exec 通道无交互别名），
    // 首字符为 CJK 的一律是解释文字（"对策：使用 systemctl ..."类反思散文、
    // "检查磁盘: smartctl ..."式条目——后者剥掉行首序号后同样 CJK 开头）。
    // 含中文**参数**的合法命令（echo 你好、grep 模式）首词元是 ASCII，不受影响。
    if starts_cjk(t) || starts_cjk(strip_enumerator(t)) {
        return false;
    }
    true
}

/// 首字符是否为 CJK 统一表意文字（含扩展 A 与兼容区）
fn starts_cjk(s: &str) -> bool {
    s.chars().next().map_or(false, |c| {
        let u = c as u32;
        (0x3400..=0x4DBF).contains(&u)
            || (0x4E00..=0x9FFF).contains(&u)
            || (0xF900..=0xFAFF).contains(&u)
    })
}

/// 剥掉行首序号（"1." "1、" "1)" + 可选空格）：弱模型爱输出 "1. 检查磁盘" 式条目。
/// 非"数字+分隔符"开头原样返回（"7z x"、"2>&1" 不受影响）。
fn strip_enumerator(t: &str) -> &str {
    let mut end = 0;
    for (i, c) in t.char_indices() {
        if c.is_ascii_digit() {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 {
        return t;
    }
    let rest = &t[end..];
    for sep in [".", "、", ")"] {
        if let Some(r) = rest.strip_prefix(sep) {
            return r.trim_start();
        }
    }
    t
}

/// 判断清洗后的回复中是否含一行独立的 DONE（不区分大小写），
/// 出现即视为任务完成——即使同一回复里还夹带了杂散噪音行。
pub fn contains_done_line(s: &str) -> bool {
    s.lines().any(|l| l.trim().eq_ignore_ascii_case("DONE"))
}

/// 将模型回复解析为待执行的命令列表
/// （去围栏、按行拆分、过滤注释/空行及提示词回显等非命令行）
pub fn parse_commands(text: &str) -> Vec<String> {
    clean_response(text)
        .lines()
        .map(|l| strip_enumerator(strip_cmd_prefix(l.trim())))
        .filter(|l| !l.is_empty() && !l.starts_with('#') && is_command_line(l))
        .map(|s| s.to_string())
        .collect()
}

/// 截断文本到指定字符数，超长时追加省略标记
pub fn truncate_text(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max).collect();
        s.push_str("…[已截断]");
        s
    }
}

/// 提取模型回复中的子目标行 `GOAL <序号> <短标题>`（P57 L2）。
/// 仅取纯标题（去掉序号），容错：无序号也可。非 `GOAL` 开头行一律忽略。
/// 弱模型常把多个 GOAL 挤在同一行（"GOAL 1. x GOAL 2. y"），按 GOAL 关键字切段逐一解析。
pub fn extract_goals(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        for seg in goal_segments(line.trim()) {
            let t = seg.trim();
            let upper = t.to_ascii_uppercase();
            if !(upper.starts_with("GOAL ") || upper.starts_with("GOAL:")) {
                continue;
            }
            let body = &t[4..].trim_start_matches([':', ' ']).trim();
            if body.is_empty() {
                continue;
            }
            // 去掉常见序号前缀："1."、"1)"、"1、"、"0x" 等
            let title = strip_goal_number(body);
            if !title.is_empty() {
                out.push(title.to_string());
            }
        }
    }
    out
}

/// 把一行按 `GOAL` 关键字出现位置切成若干 "GOAL..." 段（大小写不敏感；无切分时返回整行单段）
fn goal_segments(line: &str) -> Vec<&str> {
    let upper = line.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    let mut marks: Vec<usize> = Vec::new();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if &bytes[i..i + 4] == b"GOAL" {
            marks.push(i);
            i += 4;
        } else {
            i += 1;
        }
    }
    if marks.len() <= 1 {
        return vec![line];
    }
    let mut out = Vec::with_capacity(marks.len());
    for (n, s) in marks.iter().enumerate() {
        let end = marks.get(n + 1).copied().unwrap_or(line.len());
        out.push(&line[*s..end]);
    }
    out
}

/// 去掉子目标标题的开头序号（"1." "1)" "01-" "2." 等），保留其余。
fn strip_goal_number(s: &str) -> &str {
    let t = s.trim_start();
    let v: Vec<(usize, char)> = t.char_indices().collect();
    let len = v.len();
    if len == 0 {
        return t;
    }
    let mut i = 0usize;
    // 0x/0X 十六进制序号或十进制序号
    let mut is_hex = false;
    if len >= 2 && v[0].1 == '0' && (v[1].1 == 'x' || v[1].1 == 'X') {
        is_hex = true;
        i = 2;
    }
    while i < len && v[i].1.is_numeric() {
        i += 1;
    }
    if is_hex {
        while i < len && v[i].1.is_ascii_hexdigit() {
            i += 1;
        }
    }
    // 吃掉紧跟的序号分隔符："." ")" "、" " " "-"
    while i < len && matches!(v[i].1, '.' | ')' | '、' | '-' | '：' | ':' | ' ') {
        i += 1;
    }
    if i >= len {
        return "";
    }
    &t[v[i].0..]
}

/// 提取模型回复中的失败反思行 `REFLEXION <描述>`（P57 L2 Reflexion）。
pub fn extract_reflexion(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let t = l.trim();
        let upper = t.to_ascii_uppercase();
        // "对策：/反思：" 是弱模型的中文 REFLEXION 等价物（§8.6 实测 glm4 输出格式），同等采纳
        for (tag, skip) in [
            ("REFLEXION ", 10usize),
            ("REFLEXION:", 10),
            ("对策：", 0),
            ("对策:", 0),
            ("反思：", 0),
            ("反思:", 0),
        ] {
            let hit = if skip > 0 {
                upper.starts_with(tag)
            } else {
                t.starts_with(tag)
            };
            if hit {
                let body = if skip > 0 {
                    t[skip..].trim_start_matches([':', ' ']).trim()
                } else {
                    t[tag.len()..].trim()
                };
                if !body.is_empty() {
                    return Some(body.to_string());
                }
            }
        }
        None
    })
}

/// 子目标显式终结标记（P57 L2）：`GOAL_OK` / `GOAL_DONE` / `GOAL_FAIL`。
/// 命中返回终结状态，否则返回 None。模型可用它声明某子目标完成/失败（如"校验"类空命令目标）。
pub fn goal_marker(text: &str) -> Option<bool> {
    text.lines().find_map(|l| {
        let u = l.trim().to_ascii_uppercase();
        match u.as_str() {
            "GOAL_OK" | "GOAL_DONE" => Some(true),
            "GOAL_FAIL" => Some(false),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_base_detection() {
        assert!(is_local_base(Some("http://localhost:11434/v1")));
        assert!(is_local_base(Some("http://127.0.0.1:8080")));
        assert!(is_local_base(Some("http://0.0.0.0:9000/v1")));
        assert!(is_local_base(Some("http://[::1]:11434/v1")));
        assert!(is_local_base(Some("localhost:11434/v1")));
        assert!(!is_local_base(Some("https://api.deepseek.com")));
        assert!(!is_local_base(Some("https://api.openai.com/v1")));
        assert!(!is_local_base(None));
        // 远端域名里含 localhost 子串不应误判
        assert!(!is_local_base(Some("https://localhost.evil.com/v1")));
    }

    #[test]
    fn resolve_key_prefers_field_over_env() {
        let mut cfg = crate::config::AiConfig::default();
        cfg.api_key_env = "HELM_TEST_UNSET_ENV_XYZ".into();
        // 密文落盘不可行（单测环境也能 DPAPI），用"非密文明文"路径验证自适应
        cfg.api_key = Some("sk-plain-key".into());
        assert_eq!(Agent::resolve_api_key(&cfg).unwrap(), "sk-plain-key");

        // 字段为空 + 环境变量未设置 + 非本地 → 报错
        cfg.api_key = None;
        assert!(Agent::resolve_api_key(&cfg).is_err());

        // 本地服务免 Key
        cfg.api_base_url = Some("http://localhost:11434/v1".into());
        assert_eq!(Agent::resolve_api_key(&cfg).unwrap(), "");
    }

    #[test]
    fn parse_models_response_sorts_and_filters() {
        let body = r#"{"object":"list","data":[{"id":"qwen3:8b"},{"id":"gemma4:26b"},{"id":""},{"no_id":1}]}"#;
        assert_eq!(
            parse_models_response(body).unwrap(),
            vec!["gemma4:26b".to_string(), "qwen3:8b".to_string()]
        );
        // 非 OpenAI 格式（如聊天补全响应）应报错而非 panic
        assert!(parse_models_response(r#"{"choices":[]}"#).is_err());
        assert!(parse_models_response("not json").is_err());
    }

    #[test]
    fn parse_sse_content_line() {
        let line = r#"data: {"choices":[{"delta":{"content":"你好"}}]}"#;
        assert_eq!(parse_sse_line(line), SseEvent::Content("你好".into()));
    }

    #[test]
    fn parse_sse_done_line() {
        assert_eq!(parse_sse_line("data: [DONE]"), SseEvent::Done);
    }

    #[test]
    fn parse_sse_reasoning_content_line() {
        // 推理型模型思考阶段只有 reasoning_content，无 content
        let line = r#"data: {"choices":[{"delta":{"reasoning_content":"让我想想"}}]}"#;
        assert_eq!(parse_sse_line(line), SseEvent::Reasoning("让我想想".into()));
        // 内容与思考同时存在时以 content 优先
        let both = r#"data: {"choices":[{"delta":{"reasoning_content":"想","content":"答"}}]}"#;
        assert_eq!(parse_sse_line(both), SseEvent::Content("答".into()));
    }

    #[test]
    fn parse_sse_ignore_meta_lines() {
        assert_eq!(parse_sse_line("event: message"), SseEvent::Ignore);
        assert_eq!(parse_sse_line("id: 42"), SseEvent::Ignore);
        assert_eq!(parse_sse_line(""), SseEvent::Ignore);
        assert_eq!(parse_sse_line("data: "), SseEvent::Ignore);
        assert_eq!(parse_sse_line("data: 不是JSON"), SseEvent::Ignore);
    }

    #[test]
    fn parse_sse_crlf_handled() {
        let line = "data: [DONE]\r\n";
        assert_eq!(parse_sse_line(line), SseEvent::Done);
    }

    #[test]
    fn parse_sse_stream_error_object() {
        let line = r#"data: {"error":{"message":"context length exceeded"}}"#;
        assert_eq!(
            parse_sse_line(line),
            SseEvent::Error("context length exceeded".into())
        );
        // error 为字符串形式也识别
        let line2 = r#"data: {"error":"boom"}"#;
        assert_eq!(parse_sse_line(line2), SseEvent::Error("boom".into()));
    }

    #[test]
    fn extra_body_cannot_override_core_fields() {
        let mut agent = Agent::default();
        agent.config.extra_body = json!({
            "stream": false,
            "model": "hacked",
            "temperature": 0.9,
            "max_tokens": 1,
            "messages": [],
            "response_format": {"type": "json_object"},
        });
        let body = agent.request_body(true);
        // 核心字段保持本模块组装值，不被 extra_body 覆盖
        assert_eq!(body["stream"], true);
        assert_eq!(body["model"], agent.config.model);
        assert_eq!(body["temperature"], agent.config.temperature);
        assert_ne!(body["max_tokens"], json!(1));
        // 非保留键照常透传
        assert_eq!(body["response_format"]["type"], "json_object");
    }

    #[test]
    fn clean_response_strips_multiple_fences() {
        let raw = "```bash\nls -la\n```";
        assert_eq!(clean_response(raw), "ls -la");
        let raw2 = "```\n```\npwd\n```\n```";
        assert_eq!(clean_response(raw2), "pwd");
    }

    #[test]
    fn parse_commands_splits_lines_and_skips_comments() {
        let raw = "```\nls -la\n# 注释\n\necho hi\n```";
        let cmds = parse_commands(raw);
        assert_eq!(cmds, vec!["ls -la", "echo hi"]);
    }

    #[test]
    fn parse_commands_strips_prefix() {
        let cmds = parse_commands("命令：ls\n命令: pwd");
        assert_eq!(cmds, vec!["ls", "pwd"]);
    }

    #[test]
    fn is_command_line_filters_instruction_echo() {
        assert!(is_command_line("ls -la"));
        assert!(is_command_line("find / -name hadoop*"));
        assert!(is_command_line("echo 你好")); // 含中文的合法命令应保留
        // 提示词回显：指令句式被模型原样吐出，绝不能当作命令
        assert!(!is_command_line("（如果任务完成，请输出 \"DONE\"）"));
        assert!(!is_command_line("如果任务完成，只输出 DONE"));
        assert!(!is_command_line("只输出 DONE，不要解释"));
        assert!(!is_command_line("不要解释")); // 无 ASCII 命令词元
        assert!(!is_command_line(""));
    }

    /// §8.6 泛化验证实测（glm4 输出）：中文反思散文与序号条目不得当作命令执行
    #[test]
    fn is_command_line_filters_cjk_prose() {
        // 真实观测样本："对策：使用 `systemctl status ...` 命令替代 ..."
        assert!(!is_command_line("对策：使用 `systemctl status ssh.service` 命令替代 `service ssh status` 命令来检查。"));
        assert!(!is_command_line("检查 sshd 服务状态"));
        // 剥序号后 CJK 开头的条目同样过滤
        assert!(!is_command_line("1. 检查磁盘健康状态"));
        // 但剥序号后是真命令的保留（parse_commands 会同步剥掉序号）
        assert!(is_command_line("1. ls -la"));
        // 序号与命令粘连无分隔（7z/2>&1）不受影响
        assert!(is_command_line("7z x archive.7z"));
        assert!(is_command_line("2>&1 | tee log"));
    }

    /// 同一行挤多个 GOAL（弱模型不换行）应全部解析出标题
    #[test]
    fn extract_goals_handles_multiple_per_line() {
        let goals = extract_goals("GOAL 1. 检查 sshd 服务状态 GOAL 2. 查找报错日志");
        assert_eq!(goals, vec!["检查 sshd 服务状态".to_string(), "查找报错日志".to_string()]);
        // 常规多行格式回归
        assert_eq!(extract_goals("GOAL 1. 预检\nGOAL: 2. 安装"), vec!["预检".to_string(), "安装".to_string()]);
        // 无 GOAL 行为空
        assert!(extract_goals("ls -la\necho hi").is_empty());
    }

    /// 中文反思别名（对策：/反思：）等同 REFLEXION 采纳
    #[test]
    fn extract_reflexion_accepts_chinese_aliases() {
        assert_eq!(
            extract_reflexion("对策：使用 systemctl status 替代 service ssh status"),
            Some("使用 systemctl status 替代 service ssh status".to_string())
        );
        assert_eq!(extract_reflexion("反思：端口被占用，改用 8080"), Some("端口被占用，改用 8080".to_string()));
        assert_eq!(extract_reflexion("REFLEXION: disk full"), Some("disk full".to_string()));
        assert_eq!(extract_reflexion("无反思行"), None);
    }

    /// parse_commands 端到端：散文剔除 + 序号剥除后输出干净命令
    #[test]
    fn parse_commands_strips_enumerators_and_prose() {
        let raw = "GOAL 1. 检查服务\n1. systemctl status ssh\n对策：换 journalctl 查日志\n2. journalctl -u ssh -n 50";
        assert_eq!(parse_commands(raw), vec!["systemctl status ssh".to_string(), "journalctl -u ssh -n 50".to_string()]);
    }

    #[test]
    fn contains_done_line_detects_bare_done_amid_noise() {
        assert!(contains_done_line("DONE"));
        assert!(contains_done_line("ls -la\ndone"));
        assert!(contains_done_line("  DONE  \n"));
        // 指令回显里的"请输出 DONE"不是真正的完成标记
        assert!(!contains_done_line("（如果任务完成，请输出 \"DONE\"）"));
        assert!(!contains_done_line("ls -la"));
    }

    #[test]
    fn parse_commands_drops_instruction_echo_lines() {
        let raw = "ls -la\n（如果任务完成，请输出 \"DONE\"）\n# 注释\n";
        assert_eq!(parse_commands(raw), vec!["ls -la"]);
    }

    #[test]
    fn is_command_line_filters_goal_and_reflexion_marks() {
        // P57 L2 协议标记绝不当作命令
        assert!(!is_command_line("GOAL 1. 安装 JDK"));
        assert!(!is_command_line("GOAL: 解包安装"));
        assert!(!is_command_line("REFLEXION 缺少依赖,下次先 apt"));
        assert!(!is_command_line("EXPECT file:/opt/hadoop/conf exists"));
        // 正常命令不误杀
        assert!(is_command_line("hdfs namenode -format"));
        assert!(is_command_line("export HADOOP_HOME=/opt/hadoop"));
    }

    #[test]
    fn extract_goals_parses_titles() {
        let raw = "GOAL 1. 预检环境\nGOAL: 解包安装到 /opt/hadoop\nGOAL 3 配置三个 xml\n随便一行\nGOAL ";
        assert_eq!(
            extract_goals(raw),
            vec![
                "预检环境".to_string(),
                "解包安装到 /opt/hadoop".to_string(),
                "配置三个 xml".to_string(),
            ]
        );
    }

    #[test]
    fn extract_goals_ignores_no_title_and_keeps_lowercase_case_insensitive() {
        assert!(extract_goals("GOAL").is_empty());
        assert_eq!(extract_goals("goal 启动服务"), vec!["启动服务".to_string()]); // 大小写不敏感
        assert!(extract_goals("来历不明的 GOAL 不解析").is_empty());
    }

    #[test]
    fn parse_commands_skips_goal_marker_lines() {
        let raw = "GOAL 1. 预检\njava -version\nGOAL_OK\nREFLEXION 端口被占\nfree -h";
        assert_eq!(parse_commands(raw), vec!["java -version", "free -h"]);
    }

    #[test]
    fn reflexion_and_goal_marker_extract() {
        assert_eq!(
            extract_reflexion("REFLEXION 端口被占用,改用 8044"),
            Some("端口被占用,改用 8044".to_string())
        );
        assert_eq!(extract_reflexion("REFLEXION:"), None);
        assert_eq!(goal_marker("GOAL_OK"), Some(true));
        assert_eq!(goal_marker("GOAL_DONE"), Some(true));
        assert_eq!(goal_marker("GOAL_FAIL"), Some(false));
        assert_eq!(goal_marker("随便"), None);
    }

    #[test]
    fn truncate_text_respects_limit() {
        assert_eq!(truncate_text("abcd", 10), "abcd");
        let t = truncate_text("abcdefghij", 5);
        assert_eq!(t, "abcde…[已截断]");
    }

    #[test]
    fn history_trim_keeps_system_and_recent() {
        let mut agent = Agent::default();
        agent.config.max_history = 5;
        for i in 0..10 {
            agent.push_message(ChatMessage::user(format!("msg{}", i)));
        }
        let hist = agent.history();
        assert_eq!(hist.len(), 5);
        assert_eq!(hist[0].role, "system");
        assert_eq!(hist[4].content, "msg9");
        assert!(!hist.iter().any(|m| m.content == "msg0"));
    }

    #[test]
    fn new_picks_mode_from_config() {
        std::env::set_var("HELM_TEST_MODE_KEY", "k");
        let base = AiConfig {
            model: "m".into(),
            api_key_env: "HELM_TEST_MODE_KEY".into(),
            ..AiConfig::default()
        };
        let qa_cfg = AiConfig {
            mode: "qa".into(),
            ..base.clone()
        };
        let agent_cfg = AiConfig {
            mode: "agent".into(),
            ..base
        };
        assert_eq!(Agent::new(&qa_cfg).unwrap().mode(), AgentMode::QA);
        assert_eq!(Agent::new(&agent_cfg).unwrap().mode(), AgentMode::Agent);
    }

    #[test]
    fn new_prefers_encrypted_api_key_over_env() {
        let env_key = "HELM_TEST_API_KEY_ENV";
        std::env::set_var(env_key, "env-key");
        let cipher = crate::crypto::encrypt_api_key("direct-key").unwrap();
        let cfg = AiConfig {
            model: "m".into(),
            api_key_env: env_key.into(),
            api_key: Some(cipher),
            ..AiConfig::default()
        };
        let agent = Agent::new(&cfg).unwrap();
        assert_eq!(agent.api_key, "direct-key");
    }

    #[test]
    fn new_falls_back_to_env_when_no_api_key() {
        let env_key = "HELM_TEST_API_KEY_FALLBACK";
        std::env::set_var(env_key, "env-key");
        let cfg = AiConfig {
            model: "m".into(),
            api_key_env: env_key.into(),
            api_key: None,
            ..AiConfig::default()
        };
        let agent = Agent::new(&cfg).unwrap();
        assert_eq!(agent.api_key, "env-key");
    }

    #[test]
    fn new_rejects_empty_api_key_sources() {
        let cfg = AiConfig {
            model: "m".into(),
            api_key_env: String::new(),
            api_key: None,
            ..AiConfig::default()
        };
        assert!(Agent::new(&cfg).is_err());
    }
}
