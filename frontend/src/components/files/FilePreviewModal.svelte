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

  // P128：可调高度。原生 `resize:vertical` 手柄只有右下角 16px 且几乎看不见，
  // 实测用户"还是没办法调节"—— 故改为**可见的拖拽条**（与 FileBrowser 的 .fs-resize 同范式）。
  //
  // 关键教训：初始值**不能写死**。上一版把初始值写死 420px、上限却按视口算，
  // 视口只有 550px 时上限算出 345px —— 用户一拖就被钳到比初始值还小，看起来"越拖越小"。
  // 现在初始值与上限都由同一函数推导，保证 初始 <= 上限。
  const MIN_BODY = 140;
  const CHROME = 130; // 头部 + 页脚 + 拖拽条 + 弹窗边距的估算占位
  /** 编辑区可用高度上限（随视口变化） */
  function maxBody(): number {
    return Math.max(MIN_BODY, Math.floor(window.innerHeight * 0.9 - CHROME));
  }
  /** 初始高度 = 上限的 85%，留一点余量让用户能往上/往下都拖得动 */
  function initialBody(): number {
    return Math.max(MIN_BODY, Math.floor(maxBody() * 0.85));
  }
  let bodyHeight = $state(initialBody());
  let textareaEl = $state<HTMLTextAreaElement | null>(null);
  let gutterEl = $state<HTMLDivElement | null>(null);

  /** 行号栏跟随 textarea 滚动（两个独立滚动容器必须手动同步） */
  function syncGutter() {
    if (gutterEl && textareaEl) gutterEl.scrollTop = textareaEl.scrollTop;
  }

  function startResize(e: MouseEvent) {
    e.preventDefault();
    const startY = e.clientY;
    const startH = bodyHeight;
    const onMove = (ev: MouseEvent) => {
      // 向下拖变大
      const next = startH + (ev.clientY - startY);
      bodyHeight = Math.min(maxBody(), Math.max(MIN_BODY, next));
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
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
  <!-- 遮罩/对话框语义由 ModalShell 提供（Esc 关闭 + 点击空白关闭 + 挂载聚焦） -->
  <ModalShell labelledBy="pv-title" width="720px" {onClose}>
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
      <div class="pv-body pv-hint" style:height={`${bodyHeight}px`}>加载中…</div>
    {:else if data}
      <!-- 行号 + 编辑区。行号栏与 textarea 同步滚动（见 onscroll） -->
      <div class="pv-editor" style:height={`${bodyHeight}px`}>
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
      <!-- 可见拖拽条：向下拖变大。原生 resize 手柄太细不易发现，故显式提供。
           实测反馈"还是不能拖"—— 10px 细条在 800px 高的窗口里几乎注意不到，
           故加高到 14px 并在 grip 旁给出文字提示。
           role=separator 是拖拽手柄的正确语义；下方 ⌃/⌄ 按钮提供键盘等价操作。 -->
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_no_static_element_interactions -->
      <div
        class="pv-resize"
        onmousedown={startResize}
        role="separator"
        aria-label="拖动调整编辑区高度"
        title="按住拖动可调整高度（也可用下方 ⌃/⌄ 按钮）"
      >
        <span class="pv-resize-grip"></span>
        <span class="pv-resize-hint">拖动调高</span>
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
      <!-- 高度按钮：拖拽的键盘等价物（纯拖拽控件对键盘用户不可达） -->
      <button class="pv-btn" onclick={() => (bodyHeight = Math.max(MIN_BODY, bodyHeight - 120))} title="缩小编辑区">⌃</button>
      <button class="pv-btn" onclick={() => (bodyHeight = Math.min(maxBody(), bodyHeight + 120))} title="放大编辑区">⌄</button>
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
  /* P128：编辑区 = 行号栏 + textarea。高度由 bodyHeight 内联样式给，这里只管横向布局 */
  .pv-editor {
    display: flex;
    min-height: 0;
    background: var(--term-bg);
    overflow: hidden;
    flex-shrink: 0;
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
    /* 高度由 .pv-editor 决定（用户拖拽 → bodyHeight） */
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
    /* 关掉原生 resize 手柄：改用下方可见的 .pv-resize 拖拽条 */
    resize: none;
    outline: none;
    white-space: pre;
    overflow: auto;
  }
  /* 可见拖拽条（原生手柄太细不易发现） */
  .pv-resize {
    flex: 0 0 auto;
    height: 14px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    cursor: ns-resize;
    background: var(--modal-bg);
    border-top: 1px solid var(--border);
  }
  .pv-resize:hover {
    background: var(--hover);
  }
  .pv-resize-grip {
    width: 42px;
    height: 3px;
    border-radius: 2px;
    background: var(--border);
  }
  .pv-resize:hover .pv-resize-grip {
    background: var(--accent);
  }
  .pv-resize-hint {
    font-size: 0.68rem;
    color: var(--fg-muted);
    user-select: none;
  }
  .pv-resize:hover .pv-resize-hint {
    color: var(--accent);
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
