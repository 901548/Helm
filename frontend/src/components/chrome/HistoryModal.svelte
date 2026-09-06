<script lang="ts">
  // P87 操作历史面板：记录器 JSONL 的可视化审核 + 训练数据导出
  import { onMount } from "svelte";
  import { historyRead, historyExport } from "../../lib/api";

  interface Props {
    onClose: () => void;
  }
  let { onClose }: Props = $props();

  type Rec = Record<string, any>;

  let records = $state<Rec[]>([]);
  let loading = $state(true);
  let error = $state("");
  let sessionFilter = $state("");
  let typeFilter = $state("");
  let expanded = $state<string | null>(null);
  let exportNote = $state("");

  const TYPE_META: Record<string, { label: string; color: string }> = {
    input: { label: "输入", color: "var(--accent)" },
    ai_task: { label: "AI 任务", color: "var(--warning)" },
    ai_step: { label: "AI 步骤", color: "var(--ok)" },
    qa: { label: "问答", color: "#9d7bd8" },
  };

  async function load() {
    loading = true;
    error = "";
    try {
      records = await historyRead(500);
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  onMount(load);

  const filtered = $derived(
    records.filter((r) => {
      if (sessionFilter && r.session !== sessionFilter) return false;
      if (typeFilter && r.type !== typeFilter) return false;
      return true;
    }),
  );

  const sessions = $derived([...new Set(records.map((r) => String(r.session)))].sort());
  const stats = $derived(
    (() => {
      const c: Record<string, number> = {};
      for (const r of filtered) c[r.type] = (c[r.type] ?? 0) + 1;
      return c;
    })(),
  );

  function preview(r: Rec): string {
    const t = r.type;
    if (t === "input") return String(r.data ?? "");
    if (t === "ai_task") return String(r.task ?? "");
    if (t === "qa") return `问：${String(r.q ?? "")} → 答：${String(r.a ?? "")}`;
    if (t === "ai_step") return String(r.command ?? "");
    return "";
  }

  function detail(r: Rec): string {
    const parts: string[] = [];
    if (r.exit_code !== undefined) parts.push(`退出码 ${r.exit_code}`);
    if (r.level) parts.push(`风险 ${r.level}`);
    if (r.pwd) parts.push(`目录 ${r.pwd}`);
    if (r.output) parts.push(`输出：${String(r.output).slice(0, 600)}`);
    if (r.a) parts.push(`回答：${String(r.a).slice(0, 600)}`);
    if (r.source === "skill") parts.push("[内置技能轨迹]");
    return parts.join("\n");
  }

  async function doExport() {
    exportNote = "";
    try {
      const p = await historyExport();
      exportNote = `已导出：${p}`;
    } catch (e) {
      exportNote = `导出失败：${String(e)}`;
    }
  }

  function typeLabel(t: string): string {
    return TYPE_META[t]?.label ?? t;
  }
  function typeColor(t: string): string {
    return TYPE_META[t]?.color ?? "var(--fg-muted)";
  }
</script>

<!-- 遮罩：点击空白关闭；modal 容器 stopPropagation 防误关 -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="veil" onclick={onClose}>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="modal" onclick={(e) => e.stopPropagation()}>
    <header class="modal-head">
      <h3>操作历史</h3>
      <button class="close" title="关闭" onclick={onClose}>×</button>
    </header>

    <div class="toolbar">
      <select bind:value={typeFilter} onchange={() => (expanded = null)}>
        <option value="">全部类型</option>
        <option value="input">输入</option>
        <option value="ai_task">AI 任务</option>
        <option value="ai_step">AI 步骤</option>
        <option value="qa">问答</option>
      </select>
      <select bind:value={sessionFilter} onchange={() => (expanded = null)}>
        <option value="">全部会话</option>
        {#each sessions as s}
          <option value={s}>{s}</option>
        {/each}
      </select>
      <button class="export" onclick={doExport} title="合并全部 JSONL 导出到 Downloads/helm-exports/">导出</button>
    </div>

    <div class="modal-body">
      {#if loading}
        <p class="hint">加载中…</p>
      {:else if error}
        <p class="hint err">{error}</p>
      {:else if filtered.length === 0}
        <p class="hint">暂无记录</p>
      {:else}
        {#each filtered as r, i (i)}
          {@const key = `${r.ts}-${i}`}
          <div class="rec" role="button" tabindex="0" onclick={() => (expanded = expanded === key ? null : key)} onkeydown={(e) => { if (e.key === "Enter" || e.key === " ") { expanded = expanded === key ? null : key; } }}>
            <span class="badge" style:color={typeColor(r.type)}>{typeLabel(r.type)}</span>
            <span class="sess">{r.session}</span>
            <span class="ts">{r.ts}</span>
            <div class="prev" class:open={expanded === key}>
              <div class="prev-main">{preview(r)}</div>
              {#if expanded === key}
                <pre class="detail">{detail(r)}</pre>
              {/if}
            </div>
          </div>
        {/each}
      {/if}
    </div>

    <footer class="modal-foot">
      <span class="stats">{filtered.length} 条记录（新→旧）</span>
      <button class="primary" onclick={onClose}>关闭</button>
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
    width: 720px;
    max-height: 88vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .modal-head {
    display: flex;
    align-items: center;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  h3 {
    margin: 0;
    font-size: 0.95rem;
  }
  .close {
    margin-left: auto;
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
  .toolbar {
    display: flex;
    gap: 0.5rem;
    padding: 0.5rem 1rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .toolbar select {
    padding: 0.3rem 0.5rem;
    font-size: 0.8rem;
  }
  .export {
    margin-left: auto;
    padding: 0.3rem 0.8rem;
    font-size: 0.8rem;
  }
  .modal-body {
    flex: 1;
    min-height: 200px;
    overflow-y: auto;
    padding: 0.5rem 1rem;
  }
  .hint {
    color: var(--fg-muted);
    font-size: 0.85rem;
  }
  .hint.err {
    color: var(--danger);
  }
  .rec {
    display: flex;
    align-items: baseline;
    gap: 0.6rem;
    padding: 0.35rem 0.4rem;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .rec:hover {
    background: var(--hover);
  }
  .badge {
    flex-shrink: 0;
    width: 3.6rem;
    font-size: 0.72rem;
    font-weight: 600;
  }
  .sess {
    flex-shrink: 0;
    width: 4.2rem;
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  .ts {
    flex-shrink: 0;
    font-size: 0.72rem;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .prev {
    min-width: 0;
    flex: 1;
  }
  .prev-main {
    font-size: 0.8rem;
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .prev.open .prev-main {
    white-space: pre-wrap;
    word-break: break-all;
  }
  .detail {
    margin: 0.3rem 0 0;
    padding: 0.4rem 0.6rem;
    background: var(--track-bg);
    border-radius: var(--radius-sm);
    font-size: 0.75rem;
    white-space: pre-wrap;
    word-break: break-all;
    color: var(--fg);
  }
  .modal-foot {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 0.8rem;
    padding: 0.7rem 1rem;
    border-top: 1px solid var(--border);
    flex-shrink: 0;
  }
  .stats {
    margin-right: auto;
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  button.primary {
    background: var(--accent);
    color: #fff;
    border: none;
    font-weight: 600;
    cursor: pointer;
  }
</style>
