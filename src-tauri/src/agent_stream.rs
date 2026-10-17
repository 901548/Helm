// ============================================================================
// agent_stream.rs - Agent API 传输层碎片（P134-C 自 agent.rs 拆出）
// SSE 行解析、请求级重试判定与退避、本地服务判定、GET /models 响应解析。
// ============================================================================

/// P106：AI 请求失败自动重试——共尝试 3 次。
/// 退避按错误类别选（`retry_backoff_ms`）：普通瞬时失败 1s/3s；429/配额类限流 15s/30s。
pub(crate) const API_RETRY_ATTEMPTS: usize = 3;
pub(crate) const API_RETRY_BACKOFF_MS: [u64; API_RETRY_ATTEMPTS - 1] = [1000, 3000];

/// 判断 base_url 是否指向本机（Ollama/LM Studio 等本地推理服务无需 API Key）
pub(crate) fn is_local_base(base: Option<&str>) -> bool {
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
pub(crate) fn parse_models_response(text: &str) -> Result<Vec<String>, String> {
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

/// P106：请求级重试判定——只重试瞬时失败。
///
/// 判定对象是错误文案（HTTP 状态码已被格式化进文案），按内容甄别：
/// 鉴权/参数/权限类错误重试没有意义，还会把"Key 无效"这类报错拖慢好几秒才浮出。
pub(crate) fn is_retryable_api_error(err: &str) -> bool {
    // 不可重试：4xx 中除 408/429 外基本都是调用方的问题
    for marker in [
        "HTTP 400", "HTTP 401", "HTTP 402", "HTTP 403", "HTTP 404", "HTTP 405", "HTTP 422",
    ] {
        if err.contains(marker) {
            return false;
        }
    }
    // 流内业务错误（服务端 200 后中途报的错，如内容策略拦截/模型异常）不重试
    if err.starts_with("API 流式错误:") {
        return false;
    }
    // 其余（连接失败/超时/429/408/5xx/响应解析失败）视为瞬时，可重试
    true
}

/// P109：按错误类别选重试退避时长（attempt 从 0 计：第几次重试）。
///
/// 429/配额类限流（如上游的 "rpm exhausted"）按**分钟窗口**计数——真机实测 1s/3s 的
/// 短退避三次全落在同一窗口内必然连败；改用 15s/30s 跨过窗口再试。
/// 其余瞬时错误（超时/网络/5xx）维持短退避快重试。
pub(crate) fn retry_backoff_ms(err: &str, attempt: usize) -> u64 {
    const SLOW: [u64; 2] = [15_000, 30_000];
    const FAST: [u64; 2] = [API_RETRY_BACKOFF_MS[0], API_RETRY_BACKOFF_MS[1]];
    let limited = err.contains("HTTP 429")
        || err.contains("quota")
        || err.contains("rate limit")
        || err.contains("rate_limit");
    let table = if limited { &SLOW } else { &FAST };
    table[attempt.min(table.len() - 1)]
}

/// SSE 单行事件
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SseEvent {
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
pub(crate) fn parse_sse_line(line: &str) -> SseEvent {
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
        _ => {
            // 推理型模型（deepseek-r1 等）的思考过程：单独归口，前端据此实时展示。
            // 字段名两套并存：DeepSeek 官方 API 用 `reasoning_content`，
            // Ollama 的 OpenAI 兼容端点（新版本实测）用 `reasoning`——都认，
            // 否则思考阶段在 UI 上表现为"长时间只有光标"（P104-5 真机实测）。
            let delta = &value["choices"][0]["delta"];
            match delta["reasoning_content"]
                .as_str()
                .or_else(|| delta["reasoning"].as_str())
            {
                Some(s) if !s.is_empty() => SseEvent::Reasoning(s.to_string()),
                _ => SseEvent::Ignore,
            }
        }
    }
}
