<script lang="ts">
  // P126：文件预览/编辑弹窗（自 FileBrowser 抽出）
  //
  // 抽取理由：这是 FileBrowser 里唯一**自成一体的功能单元**——自己的加载态、保存态、
  // 三个处理函数，与文件列表/导航/cd 联动毫无耦合。抽出后 FileBrowser 只留
  // 「列表 + 导航 + 面板」职责。
  // 外壳复用 ModalShell（dialog 语义 + Esc + 焦点陷阱 + 遮罩关闭）。
  import ModalShell from "../ui/ModalShell.svelte";
  import * as api from "../../lib/api";
  // P126：base64 编解码抽为纯函数（分块 + UTF-8 安全，带单测）
  import { textToBase64, base64ToBytes } from "../../lib/b64";

  /** 预览数据（含所属会话，保存时要用） */
  export interface PreviewData {
    name: string;
    path: string;
    content: string;
    truncated?: boolean;
    session: string;
  }

  interface Props {
    data: PreviewData | null;
    loading: boolean;
    onClose: () => void;
    /** 保存/下载后由父组件刷新列表（可选） */
    onSaved?: () => void;
  }
  let { data, loading, onClose, onSaved }: Props = $props();

  let saved = $state(false);

  // P130：弹窗尺寸由**拖动边框**控制（ModalShell resizable），
  // 故这里不自管高度，编辑区 flex:1 填满弹窗即可。
  let textareaEl = $state<HTMLTextAreaElement | null>(null);
  let gutterEl = $state<HTMLDivElement | null>(null);
  /** ModalShell 暴露的缩放接口（给 ⌃/⌄ 按钮用，作为拖拽的键盘等价操作） */
  let shellApi = $state<{ nudge: (dw: number, dh: number) => void } | null>(null);

  /** 行号栏跟随 textarea 滚动（两个独立滚动容器必须手动同步） */
  function syncGutter() {
    if (gutterEl && textareaEl) gutterEl.scrollTop = textareaEl.scrollTop;
  }

  /** 行号：与内容行数一致；末尾换行不额外多算一行 */
  const lineCount = $derived(
    (() => {
      const c = data?.content ?? "";
      if (c === "") return 1;
      const n = c.split("\n").length;
      return c.endsWith("\n") ? n - 1 : n;
    })(),
  );

  // 每次打开新文件复位"已保存"提示（父组件换 data 引用即触发）
  $effect(() => {
    void data?.path;
    saved = false;
  });

  async function save() {
    if (!data || data.truncated) return;
    try {
      const r = await api.fsUpload(data.session, data.path, textToBase64(data.content), false);
      if (r?.ok) {
        saved = true;
        onSaved?.();
      }
    } catch {
      // 失败静默：文件面板的错误条由父组件统一呈现
    }
  }

  async function download() {
    if (!data) return;
    try {
      const r = await api.fsDownload(data.session, data.path);
      if (!r?.ok) return;
      const url = URL.createObjectURL(new Blob([base64ToBytes(r.message)]));
      const a = document.createElement("a");
      a.href = url;
      a.download = data.name;
      a.click();
      URL.revokeObjectURL(url);
    } catch {
      // 同上
    }
  }
</script>

{#if data || loading}
  <!-- 遮罩/对话框语义由 ModalShell 提供（Esc 关闭 + 点击空白关闭 + 挂载聚焦）
       P130：resizable 让用户**拖动边框**调整弹窗大小（内容随之重排） -->
  <!-- 给初始高度:内容里的 .pv-editor 是 flex:1,容器若无初始高度会塌成最小高度 -->
  <ModalShell
    labelledBy="pv-title"
    width="760px"
    height="520px"
    resizable
    minWidth={420}
    minHeight={260}
    bind:api={shellApi}
    {onClose}
  >
    <header class="pv-head">
      <svg class="pv-glyph" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M3.5 2h5.5l3.5 3.5V13a1.5 1.5 0 0 1-1.5 1.5h-7A1.5 1.5 0 0 1 2.5 13V3.5A1.5 1.5 0 0 1 3.5 2Z" stroke="currentColor" stroke-width="1.4"/><path d="M9 2v4h3.5" stroke="currentColor" stroke-width="1.4"/></svg>
      <div class="pv-titles">
        <span class="pv-title" id="pv-title">{data?.name ?? "读取中…"}</span>
        {#if data}
          <span class="pv-sub">{data.path}{data.truncated ? " · 已截断" : ""}</span>
        {/if}
      </div>
      <button class="pv-close" title="关闭 (Esc)" onclick={onClose}>×</button>
    </header>
    {#if loading}
      <div class="pv-body pv-hint">加载中…</div>
    {:else if data}
      <!-- 行号 + 编辑区。高度由弹窗（可拖边框）决定，故这里 flex:1 填满。
           行号栏与 textarea 同步滚动（见 syncGutter）。 -->
      <div class="pv-editor">
        <div class="pv-gutter" bind:this={gutterEl} aria-hidden="true">
          {#each { length: lineCount } as _, i (i)}
            <div class="pv-ln">{i + 1}</div>
          {/each}
        </div>
        <textarea
          class="pv-body"
          bind:this={textareaEl}
          bind:value={data.content}
          spellcheck="false"
          onscroll={syncGutter}
        ></textarea>
      </div>
    {/if}
    <footer class="pv-foot">
      {#if data?.truncated}
        <span class="pv-note">文件较大，仅预览前 60KB，保存已禁用。</span>
      {:else if saved}
        <span class="pv-saved">✓ 已保存</span>
      {:else}
        <span></span>
      {/if}
      <!-- 弹窗尺寸按钮：边框拖拽的键盘等价物（纯拖拽控件对键盘用户不可达） -->
      <button class="pv-btn" onclick={() => shellApi?.nudge(0, -120)} title="缩小弹窗高度">⌃</button>
      <button class="pv-btn" onclick={() => shellApi?.nudge(0, 120)} title="放大弹窗高度">⌄</button>
      <button class="pv-btn" onclick={() => shellApi?.nudge(-160, 0)} title="缩小弹窗宽度">⇤</button>
      <button class="pv-btn" onclick={() => shellApi?.nudge(160, 0)} title="放大弹窗宽度">⇥</button>
      <button class="pv-btn" onclick={download} disabled={!data}>下载</button>
      <button
        class="pv-btn primary"
        onclick={save}
        disabled={!data || data.truncated}
        title={data?.truncated ? "文件过大，编辑会截断内容，已禁用" : "保存并写回远端"}
      >保存</button>
    </footer>
  </ModalShell>
{/if}

<style>
  .pv-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .pv-glyph {
    width: 16px;
    height: 16px;
    color: var(--fg-muted);
    flex-shrink: 0;
  }
  .pv-titles {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .pv-title {
    font-size: 0.88rem;
    font-weight: 600;
    color: var(--fg);
  }
  .pv-sub {
    font-size: 0.7rem;
    color: var(--fg-muted);
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pv-close {
    margin-left: auto;
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-size: 1.15rem;
    line-height: 1;
    padding: 0 0.35rem;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .pv-close:hover {
    color: var(--fg);
    background: var(--hover);
  }
  /* P130：编辑区 = 行号栏 + textarea，flex:1 填满弹窗剩余空间。
     弹窗高度由用户拖动边框决定（ModalShell resizable），故这里不再自管高度。 */
  .pv-editor {
    display: flex;
    flex: 1;
    min-height: 0;
    background: var(--term-bg);
    overflow: hidden;
  }
  .pv-gutter {
    flex: 0 0 auto;
    min-width: 3.2em;
    padding: 0.6rem 0.5rem 0.6rem 0.7rem;
    text-align: right;
    color: var(--fg-muted);
    background: var(--modal-bg);
    border-right: 1px solid var(--border);
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.78rem;
    line-height: 1.5;
    user-select: none;
    overflow: hidden;
  }
  .pv-ln {
    height: 1.5em;
    font-variant-numeric: tabular-nums;
  }
  .pv-body {
    /* 高度由 .pv-editor 决定（flex:1，弹窗可拖边框） */
    flex: 1;
    min-width: 0;
    box-sizing: border-box;
    margin: 0;
    padding: 0.6rem 0.8rem;
    border: none;
    background: var(--term-bg);
    color: var(--fg);
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.78rem;
    line-height: 1.5;
    /* 关掉原生 resize 手柄：改用拖动弹窗边框 */
    resize: none;
    outline: none;
    white-space: pre;
    overflow: auto;
  }
  /* 加载态不是 textarea,居中即可 */
  .pv-hint {
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--fg-muted);
    font-family: inherit;
    flex-shrink: 0;
  }
  .pv-foot {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.6rem 1rem;
    border-top: 1px solid var(--border);
    flex-shrink: 0;
  }
  .pv-note {
    margin-right: auto;
    font-size: 0.72rem;
    color: var(--warning);
  }
  .pv-saved {
    margin-right: auto;
    font-size: 0.72rem;
    color: var(--ok);
  }
  .pv-btn {
    margin-left: auto;
    padding: 0.3rem 0.8rem;
    font-size: 0.78rem;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    cursor: pointer;
  }
  .pv-btn:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }
  .pv-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .pv-btn.primary {
    margin-left: 0;
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
    font-weight: 600;
  }
  .pv-btn.primary:hover:not(:disabled) {
    color: #fff;
    filter: brightness(1.08);
  }
</style>
