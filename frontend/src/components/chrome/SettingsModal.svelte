<script lang="ts">
  import { testAiConnection, aiListModels } from "../../lib/api";
  import type { AiConfig, UiConfig, Theme } from "../../lib/api";

  interface Props {
    aiConfig: AiConfig | null;
    uiConfig: UiConfig;
    onCancel: () => void;
    onSave: (ai: AiConfig, ui: UiConfig) => Promise<string | null>;
  }

  let { aiConfig, uiConfig, onCancel, onSave } = $props<Props>();

  let tab = $state<"ai" | "ui">("ai");

  // 常见 OpenAI 兼容提供商预设（选中即填入地址与模型，可再手改）
  const PROVIDER_PRESETS: { label: string; base: string; model: string }[] = [
    { label: "DeepSeek", base: "https://api.deepseek.com", model: "deepseek-chat" },
    { label: "OpenAI", base: "https://api.openai.com/v1", model: "gpt-4o-mini" },
    { label: "Google Gemini", base: "https://generativelanguage.googleapis.com/v1beta/openai", model: "gemini-2.0-flash" },
    { label: "Groq", base: "https://api.groq.com/openai/v1", model: "llama-3.3-70b-versatile" },
    { label: "Ollama（本地，无需 Key）", base: "http://localhost:11434/v1", model: "qwen2.5:7b" },
    { label: "硅基流动 SiliconFlow", base: "https://api.siliconflow.cn/v1", model: "Qwen/Qwen2.5-7B-Instruct" },
    { label: "智谱 GLM", base: "https://open.bigmodel.cn/api/paas/v4", model: "glm-4-flash" },
    { label: "通义千问 Qwen", base: "https://dashscope.aliyuncs.com/compatible-mode/v1", model: "qwen-turbo" },
    { label: "月之暗面 Kimi", base: "https://api.moonshot.cn/v1", model: "moonshot-v1-8k" },
    { label: "零一万物 Yi", base: "https://api.lingyiwanwu.com/v1", model: "yi-lightning" },
  ];

  // AI 表单
  let model = $state(aiConfig?.model ?? "");
  let apiKey = $state("");
  let apiKeySaved = $state(aiConfig?.api_key != null);
  let apiBaseUrl = $state(aiConfig?.api_base_url ?? "");
  let systemPrompt = $state(aiConfig?.system_prompt ?? "");
  let temperature = $state(String(aiConfig?.temperature ?? 0.3));
  let maxTokens = $state(aiConfig?.max_tokens ? String(aiConfig.max_tokens) : "");
  let stream = $state(aiConfig?.stream ?? true);
  let maxHistory = $state(String(aiConfig?.max_history ?? 30));
  let maxSteps = $state(String(aiConfig?.max_steps ?? 50));
  let maxOutputChars = $state(String(aiConfig?.max_output_chars ?? 6000));
  let timeoutSecs = $state(String(aiConfig?.timeout_secs ?? 60));
  let commandTimeout = $state(
    aiConfig?.command_timeout_secs ? String(aiConfig.command_timeout_secs) : "",
  );
  let agentConfirm = $state(aiConfig?.agent_confirm ?? false);
  let initMode = $state(aiConfig?.mode ?? "qa");

  // UI 表单
  let winWidth = $state(String(uiConfig.window_width));
  let winHeight = $state(String(uiConfig.window_height));
  let dockSessions = $state(uiConfig.dock_sessions);
  let sessionsPct = $state(String(uiConfig.sessions_panel_pct));
  let theme = $state<Theme>(uiConfig.theme);

  let error = $state("");
  let saving = $state(false);
  let testing = $state(false);
  let testResult = $state<{ ok: boolean; msg: string } | null>(null);
  let loadingModels = $state(false);
  let modelOptions = $state<string[] | null>(null);
  let modelsError = $state("");

  function applyPreset(sel: HTMLSelectElement) {
    const p = PROVIDER_PRESETS[Number(sel.value)];
    sel.value = ""; // 复位，便于再次选择同一预设
    if (!p) return;
    model = p.model;
    apiBaseUrl = p.base;
    testResult = null;
    modelOptions = null;
    modelsError = "";
  }

  async function fetchModels() {
    loadingModels = true;
    modelsError = "";
    modelOptions = null;
    try {
      modelOptions = await aiListModels(
        model.trim() || undefined,
        apiBaseUrl.trim() || undefined,
        apiKey.trim() || undefined,
      );
    } catch (e) {
      modelsError = String(e);
    } finally {
      loadingModels = false;
    }
  }

  async function runTest() {
    testing = true;
    testResult = null;
    try {
      const msg = await testAiConnection(
        model.trim() || undefined,
        apiBaseUrl.trim() || undefined,
        apiKey.trim() || undefined,
      );
      testResult = { ok: true, msg };
    } catch (e) {
      testResult = { ok: false, msg: String(e) };
    } finally {
      testing = false;
    }
  }

  function num(v: string, def: number, label: string): number {
    const n = parseFloat(v);
    if (Number.isNaN(n) || n < 0) {
      error = `${label} 无效`;
      return def;
    }
    return n;
  }

  async function submit() {
    error = "";
    saving = true;
    const ui: UiConfig = {
      window_width: num(winWidth, 1200, "窗口宽度"),
      window_height: num(winHeight, 800, "窗口高度"),
      dock_sessions: dockSessions,
      dock_chat: true,
      sessions_panel_pct: num(sessionsPct, 22, "会话面板宽度"),
      chat_panel_pct: 28,
      theme,
    };
    if (error) {
      saving = false;
      return;
    }
    const ai: AiConfig = {
      model: model.trim(),
      api_key_env: "API_KEY",
      api_key: apiKey.trim() || null,
      api_base_url: apiBaseUrl.trim() || null,
      system_prompt: systemPrompt,
      temperature: num(temperature, 0.3, "温度"),
      max_tokens: maxTokens ? num(maxTokens, 0, "max_tokens") as number : null,
      stream,
      max_history: num(maxHistory, 30, "历史条数") as number,
      max_steps: num(maxSteps, 50, "最大步数") as number,
      max_output_chars: num(maxOutputChars, 6000, "输出上限") as number,
      timeout_secs: num(timeoutSecs, 60, "超时") as number,
      command_timeout_secs: commandTimeout.trim()
        ? (num(commandTimeout, 60, "命令超时") as number)
        : null,
      system_prompt_agent: aiConfig?.system_prompt_agent ?? null,
      agent_confirm: agentConfirm,
      mode: initMode,
      extra_headers: aiConfig?.extra_headers ?? {},
      extra_body: aiConfig?.extra_body ?? null,
    };
    if (error) {
      saving = false;
      return;
    }
    const err = await onSave(ai, ui);
    saving = false;
    if (err) error = err;
  }
</script>

<!-- 遮罩：点击空白取消；modal 容器 stopPropagation 防误关，内部控件均为可访问的按钮/输入框 -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="veil" onclick={onCancel}>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="modal" onclick={(e) => e.stopPropagation()}>
    <header class="modal-head">
      <h3>设置</h3>
      <div class="tabs">
        <button class:on={tab === "ai"} onclick={() => (tab = "ai")}>AI</button>
        <button class:on={tab === "ui"} onclick={() => (tab = "ui")}>界面</button>
      </div>
      <button class="close" title="关闭" onclick={onCancel}>×</button>
    </header>

    <div class="modal-body">
    {#if tab === "ai"}
      <div class="form">
        <label>提供商快速填入（任意 OpenAI 兼容服务均可）
          <select onchange={(e) => applyPreset(e.currentTarget)}>
            <option value="">选择提供商（自动填入地址与模型）…</option>
            {#each PROVIDER_PRESETS as p, i}
              <option value={i}>{p.label}</option>
            {/each}
          </select>
        </label>
        <label>模型
          <div class="model-row">
            <input bind:value={model} oninput={() => (testResult = null)} placeholder="如 gpt-4o-mini / deepseek-chat / qwen-turbo" />
            <button
              class="ghost"
              onclick={fetchModels}
              disabled={loadingModels}
              title="从当前 API 地址拉取可用模型列表"
            >
              {loadingModels ? "获取中…" : "获取列表"}
            </button>
          </div>
        </label>
        {#if modelOptions}
          <select
            class="model-pick"
            onchange={(e) => {
              const v = e.currentTarget.value;
              if (v) model = v;
              e.currentTarget.value = "";
            }}
          >
            <option value="">从列表选择模型…（{modelOptions.length} 个）</option>
            {#each modelOptions as m}
              <option value={m}>{m}</option>
            {/each}
          </select>
        {/if}
        {#if modelsError}
          <p class="models-error">✗ {modelsError}</p>
        {/if}
        <label>API Key
          <input
            type="password"
            bind:value={apiKey}
            oninput={() => (testResult = null)}
            placeholder={apiKeySaved ? "已保存（留空则不修改）" : "输入 API Key"}
            autocomplete="off"
          />
        </label>
        <label>API Base URL（可选）
          <input bind:value={apiBaseUrl} oninput={() => (testResult = null)} placeholder="https://api.openai.com/v1（默认）" />
        </label>
        <div class="test-row">
          <button class="ghost" onclick={runTest} disabled={testing}>
            {testing ? "测试中…" : "测试连接"}
          </button>
          {#if testResult}
            <span class="test-result" class:ok={testResult.ok} class:bad={!testResult.ok}>
              {testResult.ok ? "✓ " : "✗ "}{testResult.msg}
            </span>
          {/if}
        </div>
        <label>自定义提示词
          <textarea bind:value={systemPrompt} rows="4"></textarea>
        </label>
        <div class="row3">
          <label>温度
            <input bind:value={temperature} type="number" step="0.1" />
          </label>
          <label>max_tokens
            <input bind:value={maxTokens} type="number" />
          </label>
          <label>超时(秒)
            <input bind:value={timeoutSecs} type="number" />
          </label>
        </div>
        <div class="row3">
          <label>历史条数
            <input bind:value={maxHistory} type="number" />
          </label>
          <label>最大步数
            <input bind:value={maxSteps} type="number" />
          </label>
          <label>命令超时(秒)
            <input bind:value={commandTimeout} type="number" placeholder="默认 60" title="Agent 单条命令执行超时，0 表示使用默认 60 秒" />
          </label>
        </div>
        <div class="row3">
          <label>输出上限
            <input bind:value={maxOutputChars} type="number" />
          </label>
          <span></span>
          <span></span>
        </div>
        <div class="checks">
          <label class="check"><input type="checkbox" bind:checked={stream} /> 流式输出</label>
          <label class="check" title="开启后所有命令都要确认；关闭时仅危险命令需要确认"><input type="checkbox" bind:checked={agentConfirm} /> 全部命令确认（危险命令始终确认）</label>
        </div>
        <label>初始模式
          <select bind:value={initMode}>
            <option value="qa">问答</option>
            <option value="agent">Agent</option>
          </select>
        </label>
      </div>
    {:else}
      <div class="form">
        <div class="row2">
          <label>窗口宽度
            <input bind:value={winWidth} type="number" />
          </label>
          <label>窗口高度
            <input bind:value={winHeight} type="number" />
          </label>
        </div>
        <div class="row2">
          <label>会话面板宽度%
            <input bind:value={sessionsPct} type="number" min="12" max="40" />
          </label>
          <label>主题
            <select bind:value={theme}>
              <option value="light">白天</option>
              <option value="dark">黑夜</option>
              <option value="system">跟随系统</option>
            </select>
          </label>
        </div>
        <div class="checks">
          <label class="check"><input type="checkbox" bind:checked={dockSessions} /> 显示会话面板</label>
        </div>
      </div>
    {/if}

    {#if error}
      <p class="error">{error}</p>
    {/if}
    </div>

    <footer class="modal-foot">
      <button class="ghost" onclick={onCancel}>取消</button>
      <button class="primary" onclick={submit} disabled={saving}>
        {saving ? "保存中…" : "保存"}
      </button>
    </footer>
  </div>
</div>

<style>
  .veil {
    position: fixed;
    inset: 0;
    background: rgba(8, 12, 18, 0.5);
    backdrop-filter: blur(2px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
  }
  .modal {
    background: var(--modal-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    width: 490px;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .modal-head {
    display: flex;
    align-items: center;
    gap: 0.8rem;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  h3 {
    margin: 0;
    font-size: 0.95rem;
  }
  .tabs {
    display: flex;
    gap: 0.25rem;
    margin-left: auto;
    background: var(--track-bg);
    border-radius: var(--radius-sm);
    padding: 2px;
  }
  .tabs button {
    border: none;
    background: transparent;
    padding: 0.22rem 0.9rem;
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-weight: 600;
    font-size: 0.8rem;
    color: var(--fg-muted);
  }
  .tabs button.on {
    background: var(--bg-panel);
    color: var(--accent);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.25);
  }
  .close {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-size: 1.15rem;
    cursor: pointer;
    padding: 0 0.35rem;
    border-radius: var(--radius-sm);
    line-height: 1;
  }
  .close:hover {
    color: var(--fg);
    background: var(--hover);
  }
  .modal-body {
    padding: 0.7rem 1rem 0.9rem;
    overflow-y: auto;
  }
  .form label {
    display: block;
    font-size: 0.8rem;
    color: var(--fg-muted);
    margin: 0.55rem 0 0.2rem;
  }
  .form label:first-child {
    margin-top: 0;
  }
  input,
  select,
  textarea {
    width: 100%;
    padding: 0.45rem 0.55rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-size: 0.9rem;
    font-family: inherit;
    background: var(--input-bg);
    color: var(--fg);
    transition: border-color 0.12s ease;
  }
  input:focus,
  select:focus,
  textarea:focus {
    outline: none;
    border-color: var(--accent);
  }
  textarea {
    resize: vertical;
  }
  .row2,
  .row3 {
    display: grid;
    gap: 0.6rem;
  }
  .row2 {
    grid-template-columns: 1fr 1fr;
  }
  .row3 {
    grid-template-columns: 1fr 1fr 1fr;
  }
  .checks {
    display: flex;
    gap: 1rem;
    margin-top: 0.6rem;
  }
  .check {
    display: inline-flex !important;
    align-items: center;
    gap: 0.3rem;
    margin: 0 !important;
    color: var(--fg) !important;
    font-size: 0.9rem !important;
  }
  .check input {
    width: auto;
  }
  .error {
    color: var(--danger);
    font-size: 0.85rem;
    margin: 0.6rem 0 0;
  }
  .test-row {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    margin-top: 0.45rem;
  }
  .test-row button {
    padding: 0.32rem 0.9rem;
    font-size: 0.82rem;
    flex-shrink: 0;
  }
  .test-result {
    font-size: 0.8rem;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
  }
  .test-result.ok {
    color: var(--ok);
  }
  .test-result.bad {
    color: var(--danger);
  }
  .model-row {
    display: flex;
    gap: 0.5rem;
  }
  .model-row input {
    flex: 1;
    min-width: 0;
  }
  .model-row button {
    padding: 0.32rem 0.8rem;
    font-size: 0.82rem;
    flex-shrink: 0;
  }
  .model-pick {
    margin-top: 0.35rem;
  }
  .models-error {
    color: var(--danger);
    font-size: 0.8rem;
    margin: 0.3rem 0 0;
  }
  .modal-foot {
    display: flex;
    justify-content: flex-end;
    gap: 0.6rem;
    padding: 0.7rem 1rem;
    border-top: 1px solid var(--border);
    flex-shrink: 0;
  }
  button {
    padding: 0.45rem 1.2rem;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--fg);
    cursor: pointer;
    font-size: 0.88rem;
    transition: background 0.12s ease;
  }
  button:hover {
    background: var(--hover);
  }
  button.primary {
    background: var(--accent);
    color: #fff;
    border-color: var(--accent);
    font-weight: 600;
  }
  button.primary:hover {
    background: var(--accent-hover);
  }
  button.ghost:hover {
    background: var(--hover);
  }
</style>
