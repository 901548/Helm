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
      <div class="pv-body pv-hint">加载中…</div>
    {:else if data}
      <!-- 可拖右下角调高(原生 resize:vertical);上限见 CSS 的 max-height -->
      <textarea
        class="pv-body"
        bind:value={data.content}
        spellcheck="false"
        title="拖动右下角可调整高度"
      ></textarea>
    {/if}
    <footer class="pv-foot">
      {#if data?.truncated}
        <span class="pv-note">文件较大，仅预览前 60KB，保存已禁用。</span>
      {:else if saved}
        <span class="pv-saved">✓ 已保存</span>
      {:else}
        <span></span>
      {/if}
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
  .pv-body {
    /* 高度由用户拖拽决定(resize:vertical),不再用 flex:1 被弹窗锁死。
       初始 46vh,最小 200px;上限取 min(68vh, 90vh-头尾)——
       后者防止窗口很矮时 min-height 顶破 dialog 的 90vh 上限把弹窗裁掉。 */
    height: 46vh;
    min-height: min(200px, calc(90vh - 96px));
    max-height: min(68vh, calc(90vh - 96px));
    /* 不许被弹窗的 flex 压缩,否则用户拖出来的高度会被缩回去 */
    flex-shrink: 0;
    box-sizing: border-box;
    margin: 0;
    padding: 0.6rem 0.8rem;
    border: none;
    background: var(--term-bg);
    color: var(--fg);
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.78rem;
    line-height: 1.5;
    resize: vertical;
    outline: none;
    white-space: pre;
    overflow: auto;
  }
  /* 加载态不是 textarea,撑满即可 */
  .pv-hint {
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--fg-muted);
    font-family: inherit;
    resize: none;
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
