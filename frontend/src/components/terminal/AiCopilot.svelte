<script lang="ts">
  import type { AiCard, AiState, DangerLevel } from "../../lib/api";

  // AI 常驻命令条 + 活动流(P39):所有 AI 触点收敛于此——
  // 模式切换、任务输入、步骤卡片、危险确认、停止/日志,终端保持纯净。
  interface Props {
    mode: "qa" | "agent";
    busy: boolean;
    aiState?: AiState;
    cards: AiCard[];
    taskText: string;
    summary: { text: string; ok: boolean } | null;
    streamOpen: boolean;
    focusSeq: number;
    logOpen: boolean;
    thinking?: string;
    container?: string | null;
    onSubmit: (text: string, container?: string | null) => void;
    onStop: () => void;
    onApprove: () => void;
    onReject: () => void;
    onEditPlan: (commands: string[]) => void;
    onModeChange: (m: "qa" | "agent") => void;
    onToggleStream: () => void;
    onToggleLog: () => void;
    onClear: () => void;
  }

  let {
    mode,
    busy,
    aiState = "idle",
    cards,
    taskText,
    summary,
    streamOpen,
    focusSeq,
    logOpen,
    thinking = "",
    container = null,
    onSubmit,
    onStop,
    onApprove,
    onReject,
    onEditPlan,
    onModeChange,
    onToggleStream,
    onToggleLog,
    onClear,
  } = $props<Props>();

  let text = $state("");
  // §8.7.4 Docker 会话运行时容器选择（每次提交携带）
  let containerInput = $state(container ?? "");
  // §8.7.3 计划可改：本地编辑中状态
  let editingPlanId = $state<number | null>(null);
  let planEditText = $state("");
  let inputEl = $state<HTMLInputElement | null>(null);
  let bodyEl = $state<HTMLDivElement | null>(null);
  let expanded = $state<Set<number>>(new Set());

  const levelText: Record<DangerLevel, string> = {
    Safe: "安全",
    Warning: "警告",
    Critical: "危险",
  };

  // §8.7.1 状态 chip 文案：ai_job 广播的 State 事件是唯一判态来源，idle 不显示
  const stateText: Record<AiState, string> = {
    idle: "",
    parsing: "解析中",
    planning: "生成计划",
    awaitingConfirm: "等待确认",
    executing: "执行中",
    readingBack: "读取回显",
  };

  const hasContent = $derived(cards.length > 0 || !!summary || !!taskText);

  function submit() {
    const t = text.trim();
    if (!t || busy) return;
    onSubmit(t, containerInput.trim() || null);
    text = "";
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      submit();
    } else if (e.key === "Escape") {
      text = "";
      inputEl?.blur();
    }
  }

  function toggleOut(id: number) {
    const s = new Set(expanded);
    if (s.has(id)) s.delete(id);
    else s.add(id);
    expanded = s;
  }

  function statusIcon(s: string): string {
    return s === "ok" ? "✓" : s === "fail" ? "✗" : s === "skipped" ? "⊘" : s === "confirm" ? "⚠" : "⏳";
  }

  function startEditPlan(c: { id: number; commands: { command: string }[] }) {
    editingPlanId = c.id;
    planEditText = c.commands.map((x) => x.command).join("\n");
  }

  function applyEditPlan() {
    const cmds = planEditText
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    editingPlanId = null;
    onEditPlan(cmds);
  }

  // Alt+I → 聚焦输入框(focusSeq 递增触发)
  $effect(() => {
    if (focusSeq > 0) inputEl?.focus();
  });

  // 新内容自动滚底
  $effect(() => {
    cards;
    summary;
    requestAnimationFrame(() => {
      if (bodyEl) bodyEl.scrollTop = bodyEl.scrollHeight;
    });
  });
</script>

<div class="ai-dock">
  {#if streamOpen && hasContent}
    <section class="ai-stream" aria-label="AI 活动流">
      <header class="ai-stream-head">
        <span class="ai-task" class:agent={mode === "agent"} title={taskText}>
          {mode === "agent" ? "▶" : "💬"} {taskText}
        </span>
        {#if summary}
          <span class="ai-summary" class:err={!summary.ok}>{summary.ok ? "✓" : "✗"} {summary.text}</span>
        {:else if busy}
          <span class="ai-summary running">⏳ 执行中…</span>
        {/if}
        <span class="spacer"></span>
        <button class="mini" onclick={onClear} title="清空活动流">清空</button>
        <button class="mini" onclick={onToggleStream} title="收起">▾</button>
      </header>
      <div class="ai-stream-body" bind:this={bodyEl}>
        {#if thinking && busy}
          <div class="card thinking-live">
            <div class="thinking-label">🤔 思考中…（推理型模型，实时过程）</div>
            <div class="thinking-text">{thinking}<span class="cursor">▋</span></div>
          </div>
        {/if}
        {#each cards as c (c.id)}
          {#if c.kind === "qa"}
            <div class="card qa" class:done={c.done}>
              <div class="qa-text">{c.text}{#if !c.done}<span class="cursor">▋</span>{/if}</div>
            </div>
          {:else if c.kind === "plan"}
            <div class="card plan {c.status}">
              <div class="card-head plan-head">
                <span class="st" aria-hidden="true">{statusIcon(c.status)}</span>
                <span class="plan-title">计划（{c.commands.length} 条命令）</span>
                {#if c.status === "plan"}
                  {#if c.needConfirm}
                    <button class="mini ghost" onclick={() => startEditPlan(c)}>修改</button>
                    <button class="mini" onclick={() => toggleOut(c.id)}>查看命令</button>
                    <button class="mini ghost" onclick={onReject}>放弃</button>
                    <button class="mini go" onclick={onApprove}>执行</button>
                  {:else}
                    <span class="safe-hint" title="全部为安全命令，无需确认">安全 · 自动执行</span>
                    <button class="mini" onclick={() => toggleOut(c.id)}>查看命令</button>
                  {/if}
                {/if}
              </div>
              {#if editingPlanId === c.id}
                <textarea class="plan-edit" bind:value={planEditText} rows={Math.max(3, c.commands.length)}></textarea>
                <div class="plan-edit-actions">
                  <button class="mini ghost" onclick={() => (editingPlanId = null)}>取消</button>
                  <button class="mini go" onclick={applyEditPlan}>应用并执行</button>
                </div>
              {:else if expanded.has(c.id)}
                <div class="plan-cmds">
                  {#each c.commands as pc (pc.command)}
                    <div class="plan-cmd">
                      <span class="plan-cmd-level" class:critical={pc.level !== "Safe"}>{(levelText as Record<string, string>)[pc.level] ?? pc.level}</span>
                      <code class="cmd">$ {pc.command}</code>
                      {#if pc.reason && pc.level !== "Safe"}
                        <span class="msg">{pc.reason}</span>
                      {/if}
                    </div>
                  {/each}
                </div>
              {/if}
            </div>
          {:else}
            <div class="card step {c.status}">
              <div class="card-head">
                <span class="st" aria-hidden="true">{statusIcon(c.status)}</span>
                <code class="cmd">$ {c.command}</code>
                {#if c.status === "confirm"}
                  <span class="danger" title={c.reason}>{levelText[c.level ?? "Warning"]} · {c.reason}</span>
                  <button class="mini ghost" onclick={onReject}>跳过</button>
                  <button class="mini go" onclick={onApprove}>执行</button>
                {:else if c.message}
                  <span class="msg">{c.message}</span>
                {/if}
                {#if c.output}
                  <button class="mini" onclick={() => toggleOut(c.id)}>{expanded.has(c.id) ? "收起输出" : "输出"}</button>
                {/if}
              </div>
              {#if c.output && expanded.has(c.id)}
                <pre class="out">{c.output}</pre>
              {/if}
            </div>
          {/if}
        {/each}
      </div>
    </section>
  {/if}

  <div class="ai-bar">
    <div class="mode-switch" role="group" aria-label="AI 模式">
      <button class="mode-btn" class:on={mode === "qa"} title="问答模式：聊天式提问" onclick={() => onModeChange("qa")}>⌘ 问答</button>
      <button class="mode-btn" class:on={mode === "agent"} title="Agent 模式：描述任务，AI 自动执行" onclick={() => onModeChange("agent")}>▶ Agent</button>
    </div>
    {#if busy && aiState !== "idle"}
      <span class="state-chip" class:wait={aiState === "awaitingConfirm"} title="AI 任务状态机当前阶段">
        <span class="dot" aria-hidden="true"></span>{stateText[aiState]}
      </span>
    {/if}
    <input
      class="ai-bar-container"
      bind:value={containerInput}
      aria-label="目标容器（Docker 会话）"
      placeholder="容器(可选)"
      title="Docker 会话运行时目标容器；留空回落会话持久化容器"
    />
    <input
      bind:this={inputEl}
      bind:value={text}
      class="ai-input"
      aria-label="AI 任务输入"
      placeholder={busy ? "AI 执行中…" : mode === "qa" ? "问 AI 任何问题，Enter 发送" : "描述任务，AI 自动执行，Enter 发送"}
      onkeydown={onKey}
      disabled={busy}
    />
    {#if busy}
      <button class="act stop" onclick={onStop} title="停止当前任务">⏹ 停止</button>
    {:else}
      <button class="act go" onclick={submit} title="发送 (Enter)">发送</button>
    {/if}
    <button class="act" class:on={logOpen} onclick={onToggleLog} title="AI 日志 (Alt+L)">日志</button>
    <button class="act" onclick={onToggleStream} disabled={!hasContent} title={streamOpen ? "收起活动流" : "展开活动流"}>{streamOpen ? "▾" : "▴"}</button>
  </div>
</div>

<style>
  .ai-dock {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    background: var(--tabbar-bg);
    border-top: 1px solid var(--tab-border);
  }

  /* ---------- 活动流 ---------- */
  .ai-stream {
    display: flex;
    flex-direction: column;
    max-height: min(38vh, 300px);
    min-height: 0;
    border-bottom: 1px solid var(--border);
    background: var(--bg-panel);
  }
  .ai-stream-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.3rem 0.6rem;
    font-size: 0.75rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .ai-task {
    color: var(--accent);
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
    flex-shrink: 1;
  }
  .ai-task.agent {
    color: var(--accent-hover);
  }
  .ai-summary {
    color: var(--ok);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex-shrink: 1;
    min-width: 0;
  }
  .ai-summary.err {
    color: var(--danger);
  }
  .ai-summary.running {
    color: var(--warning);
  }
  .spacer {
    flex: 1;
  }
  .ai-stream-body {
    overflow-y: auto;
    padding: 0.4rem 0.6rem;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    min-height: 60px;
  }

  .card {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bubble-bg);
    padding: 0.3rem 0.5rem;
    font-size: 0.8rem;
  }
  .card-head {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    min-width: 0;
    flex-wrap: wrap;
  }
  .st {
    width: 1.1em;
    text-align: center;
    flex-shrink: 0;
  }
  .step.ok .st {
    color: var(--ok);
  }
  .step.fail .st {
    color: var(--danger);
  }
  .step.skipped .st {
    color: var(--fg-muted);
  }
  .step.confirm {
    border-color: var(--warning);
    background: var(--warn-bg);
  }
  .step.confirm .st {
    color: var(--warning);
  }
  .step.running .st {
    color: var(--accent);
    animation: pulse 1s infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  .cmd {
    font-family: "Cascadia Mono", Consolas, monospace;
    font-size: 0.78rem;
    color: var(--fg);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
    flex-shrink: 1;
  }
  .msg {
    color: var(--fg-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .danger {
    color: var(--warning);
    font-size: 0.72rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
    flex: 1;
  }
  .out {
    margin: 0.3rem 0 0.1rem;
    padding: 0.4rem 0.5rem;
    background: var(--term-bg);
    color: var(--term-fg);
    border-radius: var(--radius-sm);
    font-family: "Cascadia Mono", Consolas, monospace;
    font-size: 0.72rem;
    white-space: pre-wrap;
    word-break: break-all;
    max-height: 200px;
    overflow-y: auto;
  }
  .qa-text {
    white-space: pre-wrap;
    word-break: break-word;
    line-height: 1.5;
  }
  .thinking-live {
    border-color: var(--warning);
    background: var(--warn-bg);
  }
  .thinking-label {
    font-size: 0.72rem;
    color: var(--warning);
    font-weight: 600;
    margin-bottom: 0.2rem;
  }
  .thinking-text {
    font-size: 0.78rem;
    color: var(--fg-muted);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 160px;
    overflow-y: auto;
    line-height: 1.4;
  }
  .card.qa.done .qa-text {
    color: var(--fg);
  }
  .cursor {
    animation: pulse 1s infinite;
    color: var(--accent);
  }

  .mini {
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--fg-muted);
    border-radius: var(--radius-sm);
    font-size: 0.7rem;
    padding: 0.12rem 0.5rem;
    cursor: pointer;
    flex-shrink: 0;
  }
  .mini:hover {
    color: var(--fg);
    background: var(--hover);
  }
  .mini.go {
    background: var(--warning);
    border-color: var(--warning);
    color: #fff;
    font-weight: 600;
  }
  .mini.ghost {
    border-color: transparent;
    background: transparent;
  }

  /* ---------- 命令条 ---------- */
  .ai-bar {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    min-height: 34px;
    padding: 0.25rem 0.5rem;
    font-size: 0.8rem;
  }
  .mode-switch {
    display: flex;
    gap: 0.2rem;
    background: var(--track-bg);
    border-radius: var(--radius-sm);
    padding: 2px;
    flex-shrink: 0;
  }
  .mode-btn {
    border: none;
    background: transparent;
    padding: 0.18rem 0.6rem;
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--fg-muted);
    white-space: nowrap;
  }
  .mode-btn.on {
    background: var(--bg-panel);
    color: var(--accent);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.25);
  }
  /* §8.7.1 状态机 chip：busy 期间显示当前阶段，等待确认时转警告色 */
  .state-chip {
    display: inline-flex;
    align-items: center;
    gap: 0.32rem;
    flex-shrink: 0;
    padding: 0.14rem 0.55rem;
    border-radius: 999px;
    background: var(--track-bg);
    color: var(--accent);
    font-size: 0.72rem;
    font-weight: 600;
    white-space: nowrap;
  }
  .state-chip .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentColor;
    animation: state-pulse 1.1s ease-in-out infinite;
  }
  .state-chip.wait {
    color: var(--warning);
  }
  @keyframes state-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.25; }
  }
  .ai-input {
    flex: 1;
    min-width: 0;
    background: var(--input-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    padding: 0.3rem 0.6rem;
    font-size: 0.8rem;
  }
  .ai-input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .ai-input:disabled {
    opacity: 0.6;
  }
  .act {
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--fg-muted);
    border-radius: var(--radius-sm);
    font-size: 0.75rem;
    padding: 0.22rem 0.6rem;
    cursor: pointer;
    flex-shrink: 0;
    white-space: nowrap;
  }
  .act:hover:not(:disabled) {
    color: var(--fg);
    background: var(--hover);
  }
  .act.on {
    color: var(--accent);
    border-color: var(--accent);
  }
  .act.go {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
    font-weight: 600;
  }
  .act.stop {
    background: var(--danger);
    border-color: var(--danger);
    color: #fff;
    font-weight: 600;
  }
  .act:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .ai-bar-container {
    flex-shrink: 0;
    width: 104px;
    background: var(--input-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    padding: 0.3rem 0.5rem;
    font-size: 0.75rem;
  }
  .ai-bar-container::placeholder {
    color: var(--fg-muted);
  }

  /* ---------- §8.7.2/8.7.3 计划卡 ---------- */
  .plan-title {
    font-weight: 600;
    color: var(--fg);
    margin-right: auto;
  }
  .safe-hint {
    font-size: 0.7rem;
    padding: 0.15rem 0.5rem;
    border-radius: 999px;
    background: var(--track-bg);
    color: var(--accent);
    white-space: nowrap;
  }
  .plan-cmds {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    padding: 0.4rem 0.6rem;
  }
  .plan-cmd {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .plan-cmd-level {
    flex-shrink: 0;
    font-size: 0.68rem;
    padding: 0.05rem 0.4rem;
    border-radius: 999px;
    background: var(--track-bg);
    color: var(--fg-muted);
  }
  .plan-cmd-level.critical {
    background: var(--danger);
    color: #fff;
  }
  .plan-edit {
    width: 100%;
    background: var(--input-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    padding: 0.4rem 0.6rem;
    font: 0.75rem/1.5 ui-monospace, SFMono-Regular, Consolas, monospace;
    resize: vertical;
    box-sizing: border-box;
  }
  .plan-edit-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.4rem;
    padding: 0.4rem 0.6rem 0;
  }
  .card.plan {
    border-left-color: var(--accent);
  }
</style>
