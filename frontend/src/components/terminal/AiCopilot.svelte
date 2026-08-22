<script lang="ts">
  import type { AiCard, DangerLevel } from "../../lib/api";

  // AI 常驻命令条 + 活动流(P39):所有 AI 触点收敛于此——
  // 模式切换、任务输入、步骤卡片、危险确认、停止/日志,终端保持纯净。
  interface Props {
    mode: "qa" | "agent";
    busy: boolean;
    cards: AiCard[];
    taskText: string;
    summary: { text: string; ok: boolean } | null;
    streamOpen: boolean;
    focusSeq: number;
    logOpen: boolean;
    onSubmit: (text: string) => void;
    onStop: () => void;
    onApprove: () => void;
    onReject: () => void;
    onModeChange: (m: "qa" | "agent") => void;
    onToggleStream: () => void;
    onToggleLog: () => void;
    onClear: () => void;
  }

  let {
    mode,
    busy,
    cards,
    taskText,
    summary,
    streamOpen,
    focusSeq,
    logOpen,
    onSubmit,
    onStop,
    onApprove,
    onReject,
    onModeChange,
    onToggleStream,
    onToggleLog,
    onClear,
  } = $props<Props>();

  let text = $state("");
  let inputEl = $state<HTMLInputElement | null>(null);
  let bodyEl = $state<HTMLDivElement | null>(null);
  let expanded = $state<Set<number>>(new Set());

  const levelText: Record<DangerLevel, string> = {
    Safe: "安全",
    Warning: "警告",
    Critical: "危险",
  };

  const hasContent = $derived(cards.length > 0 || !!summary || !!taskText);

  function submit() {
    const t = text.trim();
    if (!t || busy) return;
    onSubmit(t);
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
        {#each cards as c (c.id)}
          {#if c.kind === "qa"}
            <div class="card qa" class:done={c.done}>
              <div class="qa-text">{c.text}{#if !c.done}<span class="cursor">▋</span>{/if}</div>
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
    max-height: 38vh;
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
</style>
