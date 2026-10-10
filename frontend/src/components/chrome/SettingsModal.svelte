<script lang="ts">
  import { onMount } from "svelte";
  import { testAiConnection, aiListModels } from "../../lib/api";
  import { DEFAULT_API_KEY_ENV } from "../../lib/constants";
  import type { AiConfig, UiConfig, Theme } from "../../lib/api";

  interface Props {
    aiConfig: AiConfig | null;
    uiConfig: UiConfig;
    onCancel: () => void;
    onSave: (ai: AiConfig, ui: UiConfig) => Promise<string | null>;
    /** P105：主题即时预览（App 负责生效；取消时由 App 还原为已保存值） */
    onPreviewTheme?: (t: Theme) => void;
  }

  let { aiConfig, uiConfig, onCancel, onSave, onPreviewTheme }: Props = $props();

  type Cat = "conn" | "ai" | "term" | "ui";
  let cat = $state<Cat>("conn");

  const CATS: { key: Cat; label: string; hint: string }[] = [
    { key: "conn", label: "连接", hint: "模型服务与凭据" },
    { key: "ai", label: "AI", hint: "行为、提示词与限额" },
    { key: "term", label: "终端", hint: "显示与操作记录" },
    { key: "ui", label: "界面", hint: "布局与外观" },
  ];

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

  // ---- AI 表单 ----
  let model = $state(aiConfig?.model ?? "");
  let apiKey = $state("");
  let apiKeySaved = $state(aiConfig?.api_key != null);
  let apiBaseUrl = $state(aiConfig?.api_base_url ?? "");
  let systemPrompt = $state(aiConfig?.system_prompt ?? "");
  let systemPromptAgent = $state(aiConfig?.system_prompt_agent ?? "");
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
  let agentFc = $state(aiConfig?.agent_fc ?? false);
  let aiEchoTerminal = $state(aiConfig?.ai_echo_terminal ?? false);
  let initMode = $state(aiConfig?.mode ?? "qa");

  // ---- 界面 / 终端表单（窗口几何由拖拽 + 退出落盘管理，此处不提供）----
  let dockSessions = $state(uiConfig.dock_sessions);
  let sessionsPct = $state(String(uiConfig.sessions_panel_pct));
  let theme = $state<Theme>(uiConfig.theme);
  let termFontSize = $state(String(uiConfig.term_font_size ?? 14));
  let termScrollback = $state(String(uiConfig.term_scrollback ?? 5000));
  let recording = $state(uiConfig.recording_enabled ?? true);

  let error = $state("");
  let saving = $state(false);
  let testing = $state(false);
  let testResult = $state<{ ok: boolean; msg: string } | null>(null);
  let loadingModels = $state(false);
  let modelOptions = $state<string[] | null>(null);
  let modelsError = $state("");

  let dialogEl = $state<HTMLDivElement | null>(null);

  // 打开时确定提供商栏的初始选择：
  // 命中预设 → 该预设；否则地址非空 → 自定义模式；地址为空 → 占位符
  const presetInitIdx = PROVIDER_PRESETS.findIndex(
    (p) => p.base === (aiConfig?.api_base_url ?? "").trim(),
  );
  let presetSel = $state(
    presetInitIdx >= 0 ? String(presetInitIdx) : (aiConfig?.api_base_url ?? "").trim() ? "custom" : "",
  );

  // 字号预览用：与终端保持同一字体栈
  const previewSize = $derived(Math.min(28, Math.max(8, parseFloat(termFontSize) || 14)));

  onMount(() => dialogEl?.focus());

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onCancel();
    }
  }

  function onPresetChange(e: Event & { currentTarget: EventTarget & HTMLSelectElement }) {
    presetSel = e.currentTarget.value;
    testResult = null;
    modelOptions = null;
    modelsError = "";
    // 自定义 / 占位符：不改动已填的地址与模型，交给下方输入框
    if (presetSel === "custom" || presetSel === "") return;
    const p = PROVIDER_PRESETS[Number(presetSel)];
    if (!p) return;
    model = p.model;
    apiBaseUrl = p.base;
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
      // 窗口几何只透传当前值：后端 update_ui_config 一律保留磁盘现值（P65/P26-6）
      window_width: uiConfig.window_width,
      window_height: uiConfig.window_height,
      window_x: uiConfig.window_x ?? null,
      window_y: uiConfig.window_y ?? null,
      dock_sessions: dockSessions,
      sessions_panel_pct: num(sessionsPct, 22, "会话面板宽度"),
      theme,
      term_font_size: Math.min(28, Math.max(8, num(termFontSize, 14, "字体大小"))),
      term_scrollback: Math.min(100000, Math.max(500, num(termScrollback, 5000, "滚动缓冲"))),
      recording_enabled: recording,
    };
    if (error) {
      saving = false;
      return;
    }
    const ai: AiConfig = {
      model: model.trim(),
      api_key_env: DEFAULT_API_KEY_ENV,
      api_key: apiKey.trim() || null,
      api_base_url: apiBaseUrl.trim() || null,
      system_prompt: systemPrompt,
      temperature: num(temperature, 0.3, "温度"),
      max_tokens: maxTokens ? (num(maxTokens, 0, "max_tokens") as number) : null,
      stream,
      max_history: num(maxHistory, 30, "历史条数") as number,
      max_steps: num(maxSteps, 50, "最大步数") as number,
      max_output_chars: num(maxOutputChars, 6000, "输出上限") as number,
      timeout_secs: num(timeoutSecs, 60, "超时") as number,
      command_timeout_secs: commandTimeout.trim()
        ? (num(commandTimeout, 60, "命令超时") as number)
        : null,
      system_prompt_agent: systemPromptAgent.trim() || null,
      agent_fc: agentFc,
      ai_echo_terminal: aiEchoTerminal,
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

<svelte:window onkeydown={onKey} />

<!-- 遮罩：点击空白取消；modal 容器 stopPropagation 防误关 -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="veil" onclick={onCancel}>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="modal"
    role="dialog"
    aria-modal="true"
    aria-labelledby="set-title"
    tabindex="-1"
    bind:this={dialogEl}
    onclick={(e) => e.stopPropagation()}
  >
    <header class="head">
      <h3 id="set-title">设置</h3>
      <span class="head-sub">{CATS.find((c) => c.key === cat)?.hint ?? ""}</span>
      <button class="close" title="关闭 (Esc)" aria-label="关闭设置" onclick={onCancel}>
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><path d="M4 4l8 8M12 4l-8 8" /></svg>
      </button>
    </header>

    <div class="body">
      <!-- 左侧分类导航 -->
      <nav class="rail" aria-label="设置分类">
        {#each CATS as c (c.key)}
          <button class="rail-item" class:on={cat === c.key} onclick={() => (cat = c.key)} aria-current={cat === c.key}>
            <span class="rail-ico" aria-hidden="true">
              {#if c.key === "conn"}
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><path d="M9 17H7A5 5 0 0 1 7 7h2" /><path d="M15 7h2a5 5 0 0 1 0 10h-2" /><path d="M8 12h8" /></svg>
              {:else if c.key === "ai"}
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><rect x="6" y="6" width="12" height="12" rx="2.5" /><path d="M9.5 2.5v3M14.5 2.5v3M9.5 18.5v3M14.5 18.5v3M2.5 9.5h3M2.5 14.5h3M18.5 9.5h3M18.5 14.5h3" /></svg>
              {:else if c.key === "term"}
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M4 17l6-6-6-6" /><path d="M12 19h8" /></svg>
              {:else}
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><rect x="3" y="4" width="18" height="16" rx="2.5" /><path d="M3 10h18M9.5 10v10" /></svg>
              {/if}
            </span>
            <span class="rail-label">{c.label}</span>
          </button>
        {/each}
        <div class="rail-foot">
          <div class="rf-cap">将应用</div>
          <div class="rf-model" title={model.trim() || "未设置模型"}>{model.trim() || "未设置模型"}</div>
          <div class="rf-base" title={apiBaseUrl.trim() || "默认地址"}>{apiBaseUrl.trim() || "默认地址"}</div>
          <div class="rf-note">点「保存」写入 <code>config.yaml</code></div>
        </div>
      </nav>

      <!-- 右侧内容 -->
      <div class="pane">
        {#if cat === "conn"}
          <section class="sec">
            <span class="sec-ico" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><circle cx="12" cy="12" r="3" /><path d="M12 3v3M12 18v3M4.5 7.5l2.6 1.5M16.9 15l2.6 1.5M4.5 16.5l2.6-1.5M16.9 9l2.6-1.5" /></svg></span>
            <span class="sec-name">模型服务</span>
          </section>

          <div class="row">
            <div class="lbl-wrap">
              <span class="lbl">提供商预设</span>
              <span class="desc">
                {#if presetSel === "custom"}
                  手动填写地址与模型
                {:else}
                  选中即填入地址与模型，可再手改
                {/if}
              </span>
            </div>
            <select class="inp" bind:value={presetSel} onchange={onPresetChange} aria-label="提供商预设">
              <option value="">选择提供商…</option>
              <option value="custom">自定义（手动填写地址与模型）</option>
              <option value="__sep" disabled>──────────────</option>
              {#each PROVIDER_PRESETS as p, i}
                <option value={String(i)}>{p.label}</option>
              {/each}
            </select>
          </div>

          <div class="row">
            <div class="lbl-wrap">
              <span class="lbl">模型</span>
              <span class="desc">任意 OpenAI 兼容服务的模型名</span>
            </div>
            <div class="ctl-row">
              <input class="inp mono" bind:value={model} oninput={() => (testResult = null)} placeholder="gpt-4o-mini / deepseek-chat / glm4:latest" aria-label="模型" />
              <button class="btn" onclick={fetchModels} disabled={loadingModels} title="从当前 API 地址拉取可用模型列表">
                {loadingModels ? "获取中…" : "获取列表"}
              </button>
            </div>
          </div>

          {#if modelOptions}
            <div class="row">
              <div class="lbl-wrap"><span class="lbl">可用模型</span><span class="desc">共 {modelOptions.length} 个，点击选用</span></div>
              <div class="pick">
                {#each modelOptions as m (m)}
                  <button class="pick-item" onclick={() => { model = m; modelOptions = null; }}>
                    <span class="mono">{m}</span>
                  </button>
                {/each}
              </div>
            </div>
          {/if}
          {#if modelsError}
            <p class="note bad">✗ {modelsError}</p>
          {/if}

          <section class="sec">
            <span class="sec-ico" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><rect x="5" y="11" width="14" height="9" rx="2.2" /><path d="M8.5 11V8a3.5 3.5 0 0 1 7 0v3" /></svg></span>
            <span class="sec-name">认证</span>
          </section>

          <div class="row">
            <div class="lbl-wrap">
              <span class="lbl">API Key</span>
              <span class="desc">{apiKeySaved ? "已加密保存（DPAPI），留空不改动" : "本地推理服务可留空"}</span>
            </div>
            <input class="inp mono" type="password" bind:value={apiKey} oninput={() => (testResult = null)} placeholder={apiKeySaved ? "••••••••••••" : "输入 API Key"} autocomplete="off" aria-label="API Key" />
          </div>

          <div class="row">
            <div class="lbl-wrap">
              <span class="lbl">API Base URL</span>
              <span class="desc">留空走默认；本地 Ollama 例：http://localhost:11434/v1</span>
            </div>
            <input
              class="inp mono"
              bind:value={apiBaseUrl}
              oninput={(e) => {
                testResult = null;
                // 地址被手改到与所选预设不一致时，回落到「自定义」（清空则回占位符）
                const p = PROVIDER_PRESETS[Number(presetSel)];
                if (!p || e.currentTarget.value.trim() !== p.base) {
                  presetSel = e.currentTarget.value.trim() ? "custom" : "";
                }
              }}
              placeholder="https://api.openai.com/v1"
              aria-label="API Base URL"
            />
          </div>

          <div class="row">
            <div class="lbl-wrap"><span class="lbl">连通性</span><span class="desc">用当前表单值发一次最小请求</span></div>
            <div class="ctl-row">
              <button class="btn" onclick={runTest} disabled={testing}>{testing ? "测试中…" : "测试连接"}</button>
              {#if testResult}
                <span class="pill" class:ok={testResult.ok} class:bad={!testResult.ok}>{testResult.ok ? "✓ " : "✗ "}{testResult.msg}</span>
              {/if}
            </div>
          </div>
        {:else if cat === "ai"}
          <section class="sec">
            <span class="sec-ico" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><path d="M4 8h9M19 8h1M4 16h5M13 16h7" /><circle cx="16" cy="8" r="2.2" /><circle cx="11" cy="16" r="2.2" /></svg></span>
            <span class="sec-name">行为</span>
          </section>

          <div class="row">
            <div class="lbl-wrap"><span class="lbl">初始模式</span><span class="desc">启动时 AI dock 的默认模式</span></div>
            <div class="seg" role="group" aria-label="初始模式">
              <button class:on={initMode === "qa"} onclick={() => (initMode = "qa")}>问答</button>
              <button class:on={initMode === "agent"} onclick={() => (initMode = "agent")}>Agent</button>
            </div>
          </div>

          <div class="row sw-row">
            <div class="lbl-wrap"><span class="lbl">流式输出</span><span class="desc">逐字返回，长回答更快看到开头</span></div>
            <label class="sw"><input type="checkbox" bind:checked={stream} /><span class="track"><span class="knob"></span></span></label>
          </div>

          <div class="row sw-row">
            <div class="lbl-wrap"><span class="lbl">全部命令确认</span><span class="desc">开启后每条命令都要确认；关闭时仅危险命令确认</span></div>
            <label class="sw"><input type="checkbox" bind:checked={agentConfirm} /><span class="track"><span class="knob"></span></span></label>
          </div>

          <div class="row sw-row">
            <div class="lbl-wrap"><span class="lbl">工具调用（FC）</span><span class="desc">用结构化 tool_calls 替代文本协议；模型不支持时自动降级</span></div>
            <label class="sw"><input type="checkbox" bind:checked={agentFc} /><span class="track"><span class="knob"></span></span></label>
          </div>

          <div class="row sw-row">
            <div class="lbl-wrap"><span class="lbl">终端回显 AI 过程</span><span class="desc">关闭时终端只留 shell 输出，执行过程看 AI 卡片</span></div>
            <label class="sw"><input type="checkbox" bind:checked={aiEchoTerminal} /><span class="track"><span class="knob"></span></span></label>
          </div>

          <section class="sec">
            <span class="sec-ico" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" /><path d="M14 3v5h5M9 13h6M9 17h4" /></svg></span>
            <span class="sec-name">提示词</span>
          </section>

          <div class="row col">
            <div class="lbl-wrap"><span class="lbl">自定义提示词</span><span class="desc">问答模式使用</span></div>
            <textarea class="inp ta" bind:value={systemPrompt} rows="3" aria-label="自定义提示词"></textarea>
          </div>

          <div class="row col">
            <div class="lbl-wrap"><span class="lbl">Agent 提示词</span><span class="desc">留空使用内置（含 GOAL / REFLEXION 子目标协议）</span></div>
            <textarea class="inp ta" bind:value={systemPromptAgent} rows="4" placeholder="留空使用内置 Agent 提示词" aria-label="Agent 提示词"></textarea>
          </div>

          <section class="sec">
            <span class="sec-ico" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><circle cx="12" cy="12" r="3" /><path d="M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3" /></svg></span>
            <span class="sec-name">生成参数</span>
          </section>

          <div class="row">
            <div class="lbl-wrap"><span class="lbl">温度</span><span class="desc">越高越发散，运维任务建议 0.2 – 0.4</span></div>
            <input class="inp num" bind:value={temperature} type="number" step="0.1" aria-label="温度" />
          </div>
          <div class="row">
            <div class="lbl-wrap"><span class="lbl">max_tokens</span><span class="desc">留空由服务端决定</span></div>
            <input class="inp num" bind:value={maxTokens} type="number" placeholder="默认" aria-label="max_tokens" />
          </div>
          <div class="row">
            <div class="lbl-wrap"><span class="lbl">命令输出上限</span><span class="desc">回喂模型的单条命令输出字符数</span></div>
            <input class="inp num" bind:value={maxOutputChars} type="number" aria-label="命令输出上限" />
          </div>

          <section class="sec">
            <span class="sec-ico" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><circle cx="12" cy="12" r="8.5" /><path d="M12 7.5V12l3 2" /></svg></span>
            <span class="sec-name">执行限制</span>
          </section>

          <div class="row">
            <div class="lbl-wrap"><span class="lbl">请求超时</span><span class="desc">单次模型调用上限（秒）</span></div>
            <input class="inp num" bind:value={timeoutSecs} type="number" aria-label="请求超时" />
          </div>
          <div class="row">
            <div class="lbl-wrap"><span class="lbl">命令超时</span><span class="desc">Agent 单条命令上限（秒），留空按 60</span></div>
            <input class="inp num" bind:value={commandTimeout} type="number" placeholder="60" aria-label="命令超时" />
          </div>
          <div class="row">
            <div class="lbl-wrap"><span class="lbl">历史条数</span><span class="desc">带入模型的最大对话条数</span></div>
            <input class="inp num" bind:value={maxHistory} type="number" aria-label="历史条数" />
          </div>
          <div class="row">
            <div class="lbl-wrap"><span class="lbl">最大步数</span><span class="desc">Agent 单任务最多执行步数</span></div>
            <input class="inp num" bind:value={maxSteps} type="number" aria-label="最大步数" />
          </div>
        {:else if cat === "term"}
          <section class="sec">
            <span class="sec-ico" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><path d="M5 7V5h14v2M12 5v14M9 19h6" /></svg></span>
            <span class="sec-name">显示</span>
          </section>

          <div class="row">
            <div class="lbl-wrap"><span class="lbl">字体大小</span><span class="desc">8 – 28 px，保存后立即生效</span></div>
            <input class="inp num" bind:value={termFontSize} type="number" min="8" max="28" aria-label="终端字体大小" />
          </div>
          <div class="row">
            <div class="lbl-wrap"><span class="lbl">滚动缓冲</span><span class="desc">500 – 100000 行</span></div>
            <input class="inp num" bind:value={termScrollback} type="number" min="500" max="100000" aria-label="滚动缓冲" />
          </div>
          <div class="row col">
            <div class="lbl-wrap"><span class="lbl">预览</span><span class="desc">实际终端字体渲染效果</span></div>
            <div class="fprev" style:font-size={`${previewSize}px`}>
              <span class="fp-dim">root@helm</span>:<span class="fp-path">/opt/app</span>$ df -h
            </div>
          </div>

          <section class="sec">
            <span class="sec-ico" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><ellipse cx="12" cy="6" rx="7.5" ry="3" /><path d="M4.5 6v12c0 1.7 3.4 3 7.5 3s7.5-1.3 7.5-3V6" /><path d="M4.5 12c0 1.7 3.4 3 7.5 3s7.5-1.3 7.5-3" /></svg></span>
            <span class="sec-name">操作记录</span>
          </section>

          <div class="row sw-row">
            <div class="lbl-wrap"><span class="lbl">记录终端操作（训练数据）</span><span class="desc">按天写入 config.yaml 同目录 logs/terminal-YYYYMMDD.jsonl</span></div>
            <label class="sw"><input type="checkbox" bind:checked={recording} /><span class="track"><span class="knob"></span></span></label>
          </div>
          <p class="note">
            记录内容：键入命令 + AI 每步的命令 / 退出码 / 输出（截断 2000 字）。含敏感输出，用于训练终端操作模型前请注意脱敏。
          </p>
        {:else}
          <section class="sec">
            <span class="sec-ico" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><rect x="3" y="4" width="18" height="16" rx="2.5" /><path d="M3 10h18M9.5 10v10" /></svg></span>
            <span class="sec-name">布局</span>
          </section>

          <div class="row">
            <div class="lbl-wrap"><span class="lbl">会话面板宽度</span><span class="desc">占窗口宽度百分比（12 – 40）</span></div>
            <div class="ctl-row">
              <input class="inp num" bind:value={sessionsPct} type="number" min="12" max="40" aria-label="会话面板宽度" />
              <span class="unit">%</span>
            </div>
          </div>

          <div class="row sw-row">
            <div class="lbl-wrap"><span class="lbl">显示会话面板</span><span class="desc">关闭后可用左上角展开柄随时唤回</span></div>
            <label class="sw"><input type="checkbox" bind:checked={dockSessions} /><span class="track"><span class="knob"></span></span></label>
          </div>

          <section class="sec">
            <span class="sec-ico" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><circle cx="12" cy="12" r="4" /><path d="M12 2.5v2.5M12 19v2.5M2.5 12H5M19 12h2.5M5.2 5.2l1.8 1.8M17 17l1.8 1.8M18.8 5.2L17 7M7 17l-1.8 1.8" /></svg></span>
            <span class="sec-name">外观</span>
          </section>

          <div class="row">
            <div class="lbl-wrap"><span class="lbl">主题</span><span class="desc">终端的明暗配色同步切换</span></div>
            <div class="seg" role="group" aria-label="主题">
              <button class:on={theme === "light"} onclick={() => { theme = "light"; onPreviewTheme?.(theme); }}>白天</button>
              <button class:on={theme === "dark"} onclick={() => { theme = "dark"; onPreviewTheme?.(theme); }}>黑夜</button>
              <button class:on={theme === "system"} onclick={() => { theme = "system"; onPreviewTheme?.(theme); }}>跟随系统</button>
            </div>
          </div>
          <p class="note">主题即时预览；点「保存」写入配置，点「取消」还原。</p>
        {/if}

        {#if error}
          <p class="note bad strong">✗ {error}</p>
        {/if}
      </div>
    </div>

    <footer class="foot">
      <span class="foot-hint">
        {#if cat === "conn"}连接信息保存在本地配置，API Key 以 DPAPI 密文存储
        {:else if cat === "ai"}Agent 相关改动对下一次任务生效
        {:else if cat === "term"}记录开关即时生效，其余保存后生效
        {:else}界面改动保存后立即生效{/if}
      </span>
      <span class="sp"></span>
      <button class="btn" onclick={onCancel}>取消</button>
      <button class="btn primary" onclick={submit} disabled={saving}>{saving ? "保存中…" : "保存"}</button>
    </footer>
  </div>
</div>

<style>
  .veil {
    position: fixed;
    inset: 0;
    background: rgba(6, 9, 14, 0.52);
    backdrop-filter: blur(3px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
  }
  .modal {
    width: min(820px, calc(100vw - 80px));
    height: auto;
    max-height: min(640px, calc(100vh - 92px));
    background: var(--modal-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: 0 18px 48px rgba(0, 0, 0, 0.45);
    display: flex;
    flex-direction: column;
    overflow: hidden;
    outline: none;
  }

  /* ---------- 头部 ---------- */
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 46px;
    padding: 0 12px 0 16px;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .head h3 {
    margin: 0;
    font-size: 13.5px;
    font-weight: 600;
  }
  .head-sub {
    font-size: 11.5px;
    color: var(--fg-muted);
  }
  .close {
    margin-left: auto;
    width: 26px;
    height: 26px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: none;
    background: transparent;
    color: var(--fg-muted);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .close svg {
    width: 14px;
    height: 14px;
  }
  .close:hover {
    background: var(--hover);
    color: var(--danger);
  }

  /* ---------- 主体：左导航 + 右内容 ---------- */
  .body {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .rail {
    width: 186px;
    flex-shrink: 0;
    border-right: 1px solid var(--border);
    padding: 10px 8px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    background: var(--bg-panel);
  }
  .rail-item {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 8px 10px;
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-family: inherit;
    font-size: 12.5px;
    border-radius: var(--radius-sm);
    cursor: pointer;
    text-align: left;
  }
  .rail-item:hover {
    background: var(--hover);
    color: var(--fg);
  }
  .rail-item.on {
    background: var(--accent-dim);
    color: var(--accent);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .rail-ico svg {
    width: 15px;
    height: 15px;
    display: block;
  }
  .rail-foot {
    margin-top: auto;
    padding: 10px 10px 3px;
    font-size: 10.5px;
    line-height: 1.5;
    color: var(--fg-muted);
    min-width: 0;
  }
  .rf-cap {
    font-size: 9.5px;
    letter-spacing: 0.6px;
    color: var(--fg-muted);
    opacity: 0.75;
    margin-bottom: 3px;
  }
  .rf-model {
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 11px;
    color: var(--accent);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rf-base {
    font-size: 10px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rf-note {
    margin-top: 7px;
    padding-top: 7px;
    border-top: 1px solid var(--border);
  }
  .rail-foot code {
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 10px;
  }

  .pane {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: 16px 20px 20px;
  }

  /* ---------- 分区标题 ---------- */
  .sec {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 18px 0 6px;
  }
  .sec:first-child {
    margin-top: 0;
  }
  .sec-ico {
    display: inline-flex;
    color: var(--accent);
  }
  .sec-ico svg {
    width: 14px;
    height: 14px;
  }
  .sec-name {
    font-size: 12.5px;
    font-weight: 600;
    letter-spacing: 0.2px;
  }
  .sec::after {
    content: "";
    flex: 1;
    height: 1px;
    background: var(--border);
    margin-left: 4px;
  }

  /* ---------- 行 ---------- */
  .row {
    display: grid;
    grid-template-columns: 178px minmax(0, 1fr);
    gap: 14px;
    align-items: center;
    padding: 7px 0;
  }
  .row.col {
    grid-template-columns: 1fr;
    gap: 7px;
  }
  .row.sw-row {
    grid-template-columns: minmax(0, 1fr) auto;
  }
  .lbl-wrap {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .lbl {
    font-size: 12.5px;
    color: var(--fg);
  }
  .desc {
    font-size: 10.5px;
    line-height: 1.45;
    color: var(--fg-muted);
  }
  .ctl-row {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .ctl-row .inp {
    flex: 1 1 auto;
    min-width: 0;
  }
  /* 数字框在行内不参与拉伸（宽度固定，避免 3 位数占满整行） */
  .ctl-row .inp.num {
    flex: 0 0 auto;
  }
  .ctl-row .btn,
  .ctl-row .pill,
  .ctl-row .unit {
    flex: 0 0 auto;
  }
  .unit {
    font-size: 12px;
    color: var(--fg-muted);
  }

  /* ---------- 控件 ---------- */
  .inp {
    width: 100%;
    height: 30px;
    padding: 0 9px;
    background: var(--input-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    font-family: inherit;
    font-size: 12.5px;
  }
  .inp.mono {
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 12px;
  }
  .inp.num {
    width: 116px;
  }
  .inp.ta {
    height: auto;
    padding: 7px 9px;
    line-height: 1.55;
    resize: vertical;
  }
  .inp:focus {
    outline: none;
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }
  select.inp {
    cursor: pointer;
  }

  .seg {
    display: inline-flex;
    justify-self: start;
    gap: 2px;
    padding: 2px;
    background: var(--track-bg);
    border: 1px solid var(--border);
    border-radius: 999px;
  }
  .seg button {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-family: inherit;
    font-size: 12px;
    padding: 4px 13px;
    border-radius: 999px;
    cursor: pointer;
  }
  .seg button:hover {
    color: var(--fg);
  }
  .seg button.on {
    background: var(--modal-bg);
    color: var(--fg);
    box-shadow: var(--shadow);
  }

  .sw {
    display: inline-flex;
    align-items: center;
    cursor: pointer;
  }
  .sw input {
    position: absolute;
    opacity: 0;
    width: 0;
    height: 0;
  }
  .track {
    width: 34px;
    height: 19px;
    padding: 2px;
    border-radius: 999px;
    background: var(--track-bg);
    border: 1px solid var(--border);
    display: inline-flex;
    align-items: center;
    transition: background 0.16s, border-color 0.16s;
  }
  .knob {
    width: 13px;
    height: 13px;
    border-radius: 50%;
    background: var(--fg-muted);
    transition: transform 0.16s, background 0.16s;
  }
  .sw input:checked + .track {
    background: var(--accent-dim);
    border-color: var(--accent);
  }
  .sw input:checked + .track .knob {
    transform: translateX(15px);
    background: var(--accent);
  }
  .sw input:focus-visible + .track {
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  .btn {
    height: 30px;
    padding: 0 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--fg);
    font-family: inherit;
    font-size: 12.5px;
    cursor: pointer;
    white-space: nowrap;
  }
  .btn:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }
  .btn:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
  .btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
    font-weight: 500;
  }
  .btn.primary:hover:not(:disabled) {
    background: var(--accent-hover);
    border-color: var(--accent-hover);
    color: #fff;
  }

  .pill {
    font-size: 11.5px;
    padding: 3px 9px;
    border-radius: 999px;
    border: 1px solid var(--border);
    color: var(--fg-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 360px;
  }
  .pill.ok {
    color: var(--ok);
    border-color: var(--ok);
  }
  .pill.bad {
    color: var(--danger);
    border-color: var(--danger);
  }

  .pick {
    max-height: 152px;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--input-bg);
  }
  .pick-item {
    display: block;
    width: 100%;
    text-align: left;
    padding: 6px 10px;
    border: none;
    background: transparent;
    color: var(--fg);
    font-family: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .pick-item:hover {
    background: var(--hover);
    color: var(--accent);
  }
  .mono {
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
  }

  .fprev {
    background: var(--term-bg);
    color: var(--term-fg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 9px 11px;
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .fp-dim {
    color: var(--ok);
  }
  .fp-path {
    color: var(--accent);
  }

  .note {
    margin: 8px 0 0;
    font-size: 11px;
    line-height: 1.6;
    color: var(--fg-muted);
  }
  .note.bad {
    color: var(--danger);
  }
  .note.strong {
    margin-top: 14px;
    padding: 8px 11px;
    border: 1px solid var(--danger);
    border-radius: var(--radius-sm);
    font-size: 12px;
  }

  /* ---------- 底部 ---------- */
  .foot {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 52px;
    padding: 0 14px;
    border-top: 1px solid var(--border);
    flex-shrink: 0;
  }
  .foot-hint {
    font-size: 11px;
    color: var(--fg-muted);
  }
  .sp {
    flex: 1;
  }
</style>
