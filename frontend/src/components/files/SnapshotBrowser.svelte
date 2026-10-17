<script lang="ts">
  // P127 四期（本批）：包内成员浏览 / 单文件取回 / 单文件还原
  //
  // 定位：整包还原太重（会把整棵 /etc 拔回旧版本），多数事故只想取回**一个**文件
  // （如 nginx.conf）。这里是"小手术"入口：
  //   - 列出包内文件（后端按大小降序、超限截断）
  //   - 「查看」取回内容做只读预览（超 FETCH_RETAIN_CAP 时只保留尾部并标记）
  //   - 「保存到本机」导出为本地文件（截断时禁用 —— 存半个文件更危险）
  //   - 「还原」把单个文件写回原路径（后端自动备份当前文件为 .bak-<时间戳>）
  import ModalShell from "../ui/ModalShell.svelte";
  import * as api from "../../lib/api";
  import type { MemberEntry } from "../../lib/commands";
  import { base64ToBytes } from "../../lib/b64";

  interface Props {
    session: string;
    file: string;
    onClose: () => void;
  }
  let { session, file, onClose }: Props = $props();

  let members = $state<MemberEntry[]>([]);
  let loading = $state(true);
  let error = $state("");
  let notice = $state("");
  let filter = $state("");
  /** 正在取回/还原的成员路径（防重复点击） */
  let fetching = $state("");
  let restoring = $state("");

  interface Fetched {
    path: string;
    text: string;
    binary: boolean;
    truncated: boolean;
    size: number;
    b64: string;
  }
  let fetched = $state<Fetched | null>(null);

  const filtered = $derived(
    filter.trim()
      ? members.filter((m) =>
          m.path.toLowerCase().includes(filter.trim().toLowerCase()),
        )
      : members,
  );

  async function load() {
    loading = true;
    error = "";
    try {
      members = await api.snapshotMembers(session, file);
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    void session;
    void file;
    load();
  });

  async function doFetch(m: MemberEntry) {
    if (fetching || restoring) return;
    fetching = m.path;
    error = "";
    try {
      const r = await api.snapshotFetchFile(session, file, m.path);
      const bytes = base64ToBytes(r.contentB64);
      // NUL 字节 = 二进制，不做文本预览（TextDecoder 会出乱码）
      let binary = false;
      const probe = Math.min(bytes.length, 8000);
      for (let i = 0; i < probe; i++) {
        if (bytes[i] === 0) {
          binary = true;
          break;
        }
      }
      fetched = {
        path: m.path,
        text: binary ? "" : new TextDecoder().decode(bytes),
        binary,
        truncated: r.truncated,
        size: r.size,
        b64: r.contentB64,
      };
    } catch (e) {
      error = String(e);
    } finally {
      fetching = "";
    }
  }

  function saveLocal() {
    if (!fetched || fetched.truncated) return;
    const bytes = base64ToBytes(fetched.b64);
    const url = URL.createObjectURL(new Blob([bytes]));
    const a = document.createElement("a");
    a.href = url;
    a.download = fetched.path.split("/").pop() || "file";
    a.click();
    URL.revokeObjectURL(url);
  }

  async function doRestore(m: MemberEntry) {
    if (fetching || restoring) return;
    const ok = window.confirm(
      `将把 ${m.path} 还原到原路径。\n当前文件（若存在）会先自动备份为 .bak-<时间戳>。\n\n继续？`,
    );
    if (!ok) return;
    restoring = m.path;
    error = "";
    notice = "";
    try {
      const r = await api.snapshotRestoreFile(session, file, m.path);
      notice = r.backup
        ? `已还原 ${r.target}（原文件备份：${r.backup}）`
        : `已还原 ${r.target}（目标原本不存在）`;
    } catch (e) {
      error = String(e);
    } finally {
      restoring = "";
    }
  }

  function fmtSize(b: number): string {
    if (b < 1024) return b + "B";
    if (b < 1024 * 1024) return (b / 1024).toFixed(1) + "KB";
    return (b / 1024 / 1024).toFixed(1) + "MB";
  }
</script>

<ModalShell
  labelledBy="sb-title"
  width="720px"
  height="520px"
  resizable
  minWidth={520}
  minHeight={360}
  {onClose}
>
  <header class="sb-head">
    <div class="sb-head-main">
      <h3 id="sb-title">包内文件</h3>
      <span class="sb-file" title={file}>{file}</span>
    </div>
    <button class="sb-close" title="关闭 (Esc)" onclick={onClose}>×</button>
  </header>

  <div class="sb-body">
    {#if fetched}
      <!-- 取回预览：置顶展示，避免要滚动才看到 -->
      <div class="sb-prev">
        <div class="sb-prev-head">
          <code class="sb-prev-path" title={fetched.path}>{fetched.path}</code>
          <span class="sb-prev-size">{fmtSize(fetched.size)}</span>
          {#if fetched.truncated}
            <span class="sb-badge warn" title="超过单文件取回上限，只保留了尾部 —— 不能用于保存/还原">已截断</span>
          {/if}
          {#if fetched.binary}<span class="sb-badge">二进制</span>{/if}
          <button class="sb-act" onclick={saveLocal} disabled={fetched.truncated}>保存到本机</button>
          <button class="sb-act" onclick={() => (fetched = null)}>关闭预览</button>
        </div>
        {#if fetched.binary}
          <p class="sb-hint">二进制文件，不支持文本预览（可保存到本机查看）。</p>
        {:else}
          <pre class="sb-pre">{fetched.truncated ? `…（仅显示尾部）\n` : ""}{fetched.text}</pre>
        {/if}
      </div>
    {/if}

    <div class="sb-tools">
      <input
        class="sb-filter"
        type="text"
        placeholder="筛选路径…"
        bind:value={filter}
      />
      <span class="sb-count">{filtered.length} / {members.length} 个文件</span>
    </div>

    {#if loading}
      <p class="sb-hint">读取包内清单…</p>
    {:else if filtered.length === 0}
      <p class="sb-hint">{members.length === 0 ? "包内没有文件" : "没有匹配的文件"}</p>
    {:else}
      <div class="sb-list">
        {#each filtered as m (m.path)}
          <div class="sb-row">
            <code class="sb-path" title={m.path}>{m.path}</code>
            <span class="sb-size">{fmtSize(m.size)}</span>
            <button
              class="sb-act"
              title="取回内容做只读预览"
              onclick={() => doFetch(m)}
              disabled={fetching === m.path}
            >{fetching === m.path ? "取回中…" : "查看"}</button>
            <button
              class="sb-act danger"
              title="把此文件写回原路径（自动备份当前文件）"
              onclick={() => doRestore(m)}
              disabled={restoring === m.path}
            >{restoring === m.path ? "还原中…" : "还原"}</button>
          </div>
        {/each}
      </div>
    {/if}

    <div class="sb-note">单文件还原会覆盖原路径文件（自动备份为 .bak-&lt;时间戳&gt;）；整包还原请用列表里的「还原」。</div>
    {#if error}<div class="sb-error">{error}</div>{/if}
    {#if notice}<div class="sb-notice">{notice}</div>{/if}
  </div>
</ModalShell>

<style>
  .sb-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.55rem 0.9rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .sb-head-main {
    min-width: 0;
    flex: 1;
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
  }
  .sb-head h3 {
    margin: 0;
    font-size: 0.88rem;
    font-weight: 600;
    flex-shrink: 0;
  }
  .sb-file {
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.72rem;
    color: var(--fg-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sb-close {
    margin-left: auto;
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-size: 1.1rem;
    line-height: 1;
    padding: 0.1rem 0.35rem;
    border-radius: var(--radius-sm);
    cursor: pointer;
    flex-shrink: 0;
  }
  .sb-close:hover {
    color: var(--fg);
    background: var(--hover);
  }

  /* 三段式：固定头 + 滚动主体（底栏无，提示并入主体） */
  .sb-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0 0.9rem 0.9rem;
    display: flex;
    flex-direction: column;
  }

  /* 取回预览 */
  .sb-prev {
    margin-top: 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--input-bg);
    flex-shrink: 0;
  }
  .sb-prev-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.35rem 0.55rem;
    border-bottom: 1px solid var(--border);
  }
  .sb-prev-path {
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.74rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
  }
  .sb-prev-size {
    font-size: 0.7rem;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }
  .sb-badge {
    font-size: 0.64rem;
    padding: 0.02rem 0.35rem;
    border-radius: 999px;
    background: var(--track-bg);
    color: var(--fg-muted);
    flex-shrink: 0;
  }
  .sb-badge.warn {
    background: var(--warn-bg);
    color: var(--warning);
  }
  .sb-pre {
    margin: 0;
    padding: 0.5rem 0.55rem;
    max-height: 200px;
    overflow: auto;
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.72rem;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-all;
    color: var(--fg);
  }

  /* 工具行 */
  .sb-tools {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    margin-top: 0.6rem;
  }
  .sb-filter {
    flex: 1 1 auto;
    min-width: 0;
    padding: 0.3rem 0.55rem;
    font-size: 0.76rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--input-bg);
    color: var(--fg);
  }
  .sb-count {
    flex-shrink: 0;
    font-size: 0.72rem;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }

  /* 文件列表 */
  .sb-list {
    margin-top: 0.35rem;
  }
  .sb-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.22rem 0.4rem;
    border-radius: var(--radius-sm);
  }
  .sb-row:hover {
    background: var(--hover);
  }
  .sb-path {
    flex: 1;
    min-width: 0;
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.74rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .sb-size {
    flex-shrink: 0;
    font-size: 0.7rem;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .sb-act {
    flex-shrink: 0;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    cursor: pointer;
    padding: 0.1rem 0.45rem;
    border-radius: var(--radius-sm);
    font-size: 0.7rem;
  }
  .sb-act:hover:not(:disabled) {
    color: var(--accent);
    border-color: var(--accent);
  }
  .sb-act.danger:hover:not(:disabled) {
    color: var(--warning);
    border-color: var(--warning);
  }
  .sb-act:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .sb-note {
    margin-top: 0.6rem;
    font-size: 0.7rem;
    color: var(--fg-muted);
    opacity: 0.8;
  }
  .sb-error {
    margin: 0.4rem 0 0;
    padding: 0.4rem 0.6rem;
    background: var(--warn-bg);
    border-radius: var(--radius-sm);
    font-size: 0.74rem;
    color: var(--danger);
  }
  .sb-notice {
    margin: 0.4rem 0 0;
    font-size: 0.74rem;
    color: var(--ok);
  }
  .sb-hint {
    margin: 0.4rem 0;
    font-size: 0.76rem;
    color: var(--fg-muted);
  }
</style>
