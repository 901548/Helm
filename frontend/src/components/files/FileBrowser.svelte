<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../lib/api";
  import type { FileEntry, SessionKind } from "../../lib/api";
  import { fmtSize as fmtSizeLib, isValidEntryName, joinPath as joinPathLib, parentPathWindows, resolvePath as resolvePathLib, shq } from "../../lib/paths";

  interface Props {
    activeTab: string | null;
    cdReq?: { name: string; target: string; seq: number } | null;
    kinds?: Record<string, SessionKind>;
  }

  let { activeTab, cdReq, kinds = {} } = $props<Props>();

  let entries = $state<FileEntry[]>([]);
  let cwd = $state("");
  let loading = $state(false);
  let error = $state("");
  // P31:默认折叠,终端空间最大化;点击展开
  let collapsed = $state(true);
  let panelHeight = $state(180);
  let fileInput: HTMLInputElement;

  let ctxEntry = $state<FileEntry | null>(null);
  let ctxPos = $state<{ x: number; y: number } | null>(null);
  let pendingName = $state<{ type: "mkdir" | "rename" | "delete"; name?: string } | null>(null);
  let nameInput = $state("");
  let lastFile = $state<FileEntry | null>(null);
  let preview = $state<{ name: string; path: string; content: string; truncated: boolean; session: string } | null>(null);
  let previewLoading = $state(false);
  let saved = $state(false);

  const fmtSize = fmtSizeLib;

  let refreshSeq = 0;

  const kind = $derived(activeTab ? kinds[activeTab] ?? "linux" : "linux");
  const isWindows = $derived(kind === "windows");
  const isBlocked = $derived(kind !== "linux" && kind !== "windows");
  const sep = $derived(isWindows ? "\\" : "/");

  const joinPath = (base: string, name: string) => joinPathLib(base, name, sep);

  async function refresh(dir?: string) {
    if (!activeTab) return;
    if (isBlocked) {
      // P94：作废在途刷新（rdp 会话/不支持平台）——否则旧会话在途结果 seq 仍等于 refreshSeq，
      // 会照常写回 entries/cwd，短暂显示旧会话路径
      refreshSeq++;
      entries = [];
      cwd = "";
      loading = false;
      error = "";
      return;
    }
    const seq = ++refreshSeq;
    loading = true;
    error = "";
    try {
      const list = await api.fsListDir(activeTab, dir ?? "");
      if (seq !== refreshSeq) return;
      entries = list;
      const c = await api.fsCurrentDir(activeTab);
      if (seq !== refreshSeq) return;
      cwd = c;
    } catch (e) {
      if (seq !== refreshSeq) return;
      error = String(e);
      entries = [];
    } finally {
      if (seq === refreshSeq) loading = false;
    }
  }

  $effect(() => {
    if (activeTab) refresh();
    else {
      // P94：关闭全部标签时作废在途刷新，防过期结果写回
      refreshSeq++;
      entries = [];
      cwd = "";
    }
    // 切换会话时清掉上次选中的文件,防止「⬇ 下载/右键下载」作用于新会话的同名路径
    lastFile = null;
  });

  $effect(() => {
    const r = cdReq;
    if (r && r.name === activeTab && kind === "linux") {
      const target = resolvePath(r.target);
      refresh(target);
    }
  });

  // shq 来自 lib/paths(bash 单引号安全包裹)

  function syncTerminal(path: string) {
    if (!activeTab || kind !== "linux") return;
    api.sendInput(activeTab, new TextEncoder().encode(`cd ${shq(path)}\r`)).catch(() => {});
  }

  function openDir(e: FileEntry) {
    if (!e.isDir) return;
    const next = joinPath(cwd, e.name);
    syncTerminal(next);
    refresh(next);
  }

  function selectEntry(e: FileEntry) {
    if (e.isDir) return;
    lastFile = e;
  }

  async function openFile(e: FileEntry) {
    if (e.isDir) return;
    if (!activeTab) return;
    lastFile = e;
    previewLoading = true;
    error = "";
    saved = false;
    const sess = activeTab;
    const path = joinPath(cwd, e.name);
    try {
      const r = await api.fsReadFile(sess, path, 60000);
      if (!r.ok) {
        error = r.message;
        return;
      }
      // 绑定打开时的会话:预览期间切换标签,保存/下载仍作用于原会话,避免写错机器
      preview = { name: e.name, path, content: r.message, truncated: r.truncated, session: sess };
    } catch (err) {
      error = String(err);
    } finally {
      previewLoading = false;
    }
  }

  function resolvePath(raw: string): string {
    return resolvePathLib(raw, cwd, sep);
  }

  async function savePreview() {
    if (!preview || preview.truncated) return;
    // 用预览绑定的会话,而非当前活动标签(防切换会话后写错机器)
    const bytes = new TextEncoder().encode(preview.content);
    let bin = "";
    for (const b of bytes) bin += String.fromCharCode(b);
    const b64 = btoa(bin);
    try {
      const r = await api.fsUpload(preview.session, preview.path, b64, false);
      if (!r.ok) {
        error = r.message;
        return;
      }
      saved = true;
      refresh();
    } catch (e) {
      error = String(e);
    }
  }

  function goUp() {
    if (!cwd) return;
    if (isWindows) {
      const parent = parentPathWindows(cwd);
      if (parent !== null) refresh(parent);
      return;
    }
    if (cwd === "/") return;
    const parts = cwd.split("/").filter(Boolean);
    parts.pop();
    const parent = "/" + parts.join("/");
    syncTerminal(parent);
    refresh(parent);
  }

  function openContext(e: MouseEvent, entry: FileEntry) {
    e.preventDefault();
    ctxEntry = entry;
    if (!entry.isDir) lastFile = entry;
    ctxPos = { x: e.clientX, y: e.clientY };
  }

  function ctxMenu(action: "mkdir" | "rename" | "delete" | "download") {
    if (!ctxEntry && action !== "mkdir") {
      ctxPos = null;
      return;
    }
    ctxPos = null;
    if (action === "mkdir") {
      pendingName = { type: "mkdir" };
      nameInput = "";
      return;
    }
    if (!ctxEntry) return;
    if (action === "rename") {
      pendingName = { type: "rename", name: ctxEntry.name };
      nameInput = ctxEntry.name;
    } else if (action === "delete") {
      pendingName = { type: "delete", name: ctxEntry.name };
    } else if (action === "download") {
      doDownload(ctxEntry);
    }
    ctxEntry = null;
  }

  async function submitName() {
    if (!activeTab || !pendingName) return;
    const name = nameInput.trim();
    // P91：必须是单一文件名分量——拒绝 `../x`（逃逸出目录/覆盖他文件）、含 `/`/`\` 的路径
    if (!isValidEntryName(name)) {
      error = "名称不能为空，且不能包含 /、\\ 或 ..";
      return;
    }
    try {
      if (pendingName.type === "mkdir") {
        // mkdir 的目标路径来自输入框,pendingName.name 此时为 undefined
        await api.fsMkdir(activeTab, joinPath(cwd, name));
      } else if (pendingName.type === "rename" && pendingName.name) {
        const oldPath = joinPath(cwd, pendingName.name);
        await api.fsRename(activeTab, oldPath, joinPath(cwd, name));
      }
      pendingName = null;
      refresh();
    } catch (e) {
      error = String(e);
    }
  }

  async function doDelete() {
    if (!activeTab || !pendingName?.name) return;
    const path = joinPath(cwd, pendingName.name);
    try {
      const r = await api.fsRemove(activeTab, path);
      if (!r.ok) error = r.message;
      pendingName = null;
      refresh();
    } catch (e) {
      error = String(e);
    }
  }

  // 下载到浏览器（base64 解码 → Blob → a.click 保存），按传入会话/路径取文件
  async function downloadToBrowser(session: string, path: string, filename: string) {
    const r = await api.fsDownload(session, path);
    if (!r.ok) {
      error = r.message;
      return;
    }
    const binary = atob(r.message);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    const blob = new Blob([bytes]);
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    a.click();
    URL.revokeObjectURL(url);
  }

  async function doDownload(entry: FileEntry) {
    if (!activeTab) return;
    const path = joinPath(cwd, entry.name);
    try {
      await downloadToBrowser(activeTab, path, entry.name);
    } catch (e) {
      error = String(e);
    }
  }

  // P91：预览弹窗「下载」用预览绑定的会话/路径（与保存一致），切标签不会下错文件
  async function downloadPreview() {
    if (!preview) return;
    try {
      await downloadToBrowser(preview.session, preview.path, preview.name);
    } catch (e) {
      error = String(e);
    }
  }

  let uploading = $state(false);

  async function doUpload(file: File) {
    if (!activeTab) return;
    // 互斥:并发上传同名文件会让 64KB 分块交错写坏远端文件
    if (uploading) {
      error = "已有上传进行中，请等待完成";
      return;
    }
    uploading = true;
    const reader = new FileReader();
    reader.onerror = () => {
      error = `读取本地文件失败: ${file.name}`;
      uploading = false;
    };
    reader.onload = async () => {
      const buf = reader.result as ArrayBuffer;
      const bytes = new Uint8Array(buf);
      const sess = activeTab;
      const path = joinPath(cwd, file.name);
      let append = false;
      try {
        // 64KB 分块，避免单条命令过长
        for (let off = 0; off < bytes.length; off += 65536) {
          const chunk = bytes.slice(off, off + 65536);
          let bin = "";
          for (let i = 0; i < chunk.length; i++) bin += String.fromCharCode(chunk[i]);
          const b64 = btoa(bin);
          const r = await api.fsUpload(sess, path, b64, append);
          if (!r.ok) {
            error = r.message;
            return;
          }
          append = true;
        }
        refresh();
      } catch (e) {
        error = String(e);
      } finally {
        uploading = false;
      }
    };
    reader.readAsArrayBuffer(file);
  }

  onMount(() => {
    // onConnection 返回 Promise<UnlistenFn>,须暂存后异步注销
    let unConn: (() => void) | null = null;
    api.onConnection((p) => {
      if (p.status === "connected") refresh();
    }).then((fn) => (unConn = fn));
    return () => {
      unConn?.();
    };
  });

  function startResize(e: MouseEvent) {
    e.preventDefault();
    const startY = e.clientY;
    const startH = panelHeight;
    const onMove = (ev: MouseEvent) => {
      panelHeight = Math.min(600, Math.max(80, startH + (startY - ev.clientY)));
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }
</script>

<aside class="fs-panel" class:collapsed style:height={`${collapsed ? 32 : panelHeight}px`}>
  <header class="fs-header">
    <button class="fold" title={collapsed ? "展开文件" : "折叠文件"} onclick={() => (collapsed = !collapsed)}>
      {collapsed ? "▲" : "▼"}
    </button>
    {#if collapsed}
      <span class="fs-title-collapsed" title="文件面板">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
          <path d="M1.5 4.5A1.5 1.5 0 0 1 3 3h3l1.5 2H13a1.5 1.5 0 0 1 1.5 1.5v6A1.5 1.5 0 0 1 13 14H3a1.5 1.5 0 0 1-1.5-1.5v-8Z" stroke="currentColor" stroke-width="1.4"/>
        </svg>
        <span>文件</span>
      </span>
    {:else}
      <span class="fs-title">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
          <path d="M1.5 4.5A1.5 1.5 0 0 1 3 3h3l1.5 2H13a1.5 1.5 0 0 1 1.5 1.5v6A1.5 1.5 0 0 1 13 14H3a1.5 1.5 0 0 1-1.5-1.5v-8Z" stroke="currentColor" stroke-width="1.4"/>
        </svg>
        文件
      </span>
      <span class="fs-cwd" title={cwd}>{cwd || (activeTab ? `— (${activeTab})` : "未连接会话")}</span>
      <div class="fs-nav">
        <button class="icon-btn" title="上一级" onclick={goUp} disabled={cwd === "/" || !cwd}>↑</button>
        <button class="icon-btn" title="刷新" onclick={() => refresh()}>⟳</button>
        <button class="icon-btn" title="新建目录" onclick={() => (pendingName = { type: "mkdir" }, nameInput = "")}>＋</button>
        <span class="nav-sep"></span>
        <button
          class="icon-btn"
          title={uploading ? "上传中…" : "上传到当前目录"}
          disabled={uploading || !activeTab}
          onclick={() => fileInput.click()}
          class:busy={uploading}
        >{uploading ? "…" : "⬆"}</button>
        <button
          class="icon-btn"
          title={lastFile ? `下载 ${lastFile.name}` : "先点击一个文件再下载"}
          disabled={!lastFile}
          onclick={() => lastFile && doDownload(lastFile)}
        >⬇</button>
      </div>
    {/if}
  </header>

  {#if !collapsed}
    {#if isBlocked}
      <div class="fs-hint">RDP 会话暂不支持文件面板</div>
    {:else}

    {#if pendingName}
      <div class="name-dialog">
        {#if pendingName.type === "delete"}
          <span>删除 <code>{pendingName.name}</code>？</span>
          <div class="name-actions">
            <button class="ghost" onclick={() => (pendingName = null)}>取消</button>
            <button class="danger" onclick={doDelete}>删除</button>
          </div>
        {:else}
          <input bind:value={nameInput} placeholder={pendingName.type === "mkdir" ? "目录名" : "新名称"} onkeydown={(e) => e.key === "Enter" && submitName()} />
          <div class="name-actions">
            <button class="ghost" onclick={() => (pendingName = null)}>取消</button>
            <button class="primary" onclick={submitName}>{pendingName.type === "mkdir" ? "创建" : "重命名"}</button>
          </div>
        {/if}
      </div>
    {/if}

    {#if error}
      <div class="fs-error">{error}</div>
    {/if}

    <!-- 容器级右键兜底（行级 openContext 已 preventDefault），空区域不弹原生菜单 -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="fs-list" oncontextmenu={(e) => e.preventDefault()}>
      <div class="fs-cols">
        <span class="fs-name">名称</span>
        <span class="fs-size">大小</span>
        <span class="fs-perms">权限</span>
        <span class="fs-mtime">修改时间</span>
      </div>
      {#if loading}
        <div class="fs-hint">加载中…</div>
      {:else if entries.length === 0 && !error}
        <div class="fs-hint">（空目录或未连接）</div>
      {/if}
      {#each entries as e (e.name)}
        <div
          class="fs-row {e.name === lastFile?.name && !e.isDir ? "selected" : ""}"
          role="button"
          tabindex="0"
          ondblclick={() => (e.isDir ? openDir(e) : openFile(e))}
          onclick={() => selectEntry(e)}
          oncontextmenu={(ev) => openContext(ev, e)}
          onkeydown={(ev) => {
            if (ev.key === "Enter") {
              ev.preventDefault();
              if (e.isDir) openDir(e);
              else openFile(e);
            } else if (ev.key === " ") {
              ev.preventDefault();
              selectEntry(e);
            }
          }}
        >
          <span class="fs-icon" class:dir={e.isDir} aria-hidden="true">
            {#if e.isDir}
              <svg viewBox="0 0 16 16" fill="none"><path d="M1.5 4.5A1.5 1.5 0 0 1 3 3h3l1.5 2H13a1.5 1.5 0 0 1 1.5 1.5v6A1.5 1.5 0 0 1 13 14H3a1.5 1.5 0 0 1-1.5-1.5v-8Z" stroke="currentColor" stroke-width="1.4"/></svg>
            {:else}
              <svg viewBox="0 0 16 16" fill="none"><path d="M3.5 2h5.5l3.5 3.5V13a1.5 1.5 0 0 1-1.5 1.5h-7A1.5 1.5 0 0 1 2.5 13V3.5A1.5 1.5 0 0 1 3.5 2Z" stroke="currentColor" stroke-width="1.4"/><path d="M9 2v4h3.5" stroke="currentColor" stroke-width="1.4"/></svg>
            {/if}
          </span>
          <span class="fs-name" title={e.name}>{e.name}</span>
          <span class="fs-size">{e.isDir ? "—" : fmtSize(e.size)}</span>
          <span class="fs-perms">{e.perms}</span>
          <span class="fs-mtime">{e.mtime}</span>
        </div>
      {/each}
    </div>

    {#if ctxPos}
      <div class="ctx-menu" style:left={ctxPos.x + "px"} style:top={ctxPos.y + "px"}>
        {#if ctxEntry?.isDir}
          <button onclick={() => (ctxEntry ? openDir(ctxEntry) : null)}>打开</button>
        {:else}
          <button onclick={() => (ctxEntry ? openFile(ctxEntry) : null)}>打开（预览）</button>
        {/if}
        <button onclick={() => ctxMenu("rename")}>重命名</button>
        {#if !ctxEntry?.isDir}
          <button onclick={() => ctxMenu("download")}>下载</button>
        {/if}
        <button onclick={() => ctxMenu("mkdir")}>新建目录</button>
        <button class="danger" onclick={() => ctxMenu("delete")}>删除</button>
      </div>
      <!-- 遮罩：点击空白处关闭菜单，无独立语义 -->
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <div class="ctx-veil" onclick={() => (ctxPos = null, ctxEntry = null)}></div>
    {/if}

    {#if preview || previewLoading}
      <!-- 遮罩：点击空白关闭预览；pv-modal 容器 stopPropagation 防误关 -->
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <div class="pv-veil" onclick={() => (preview = null)}>
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <div class="pv-modal" onclick={(e) => e.stopPropagation()}>
          <header class="pv-head">
            <svg class="pv-glyph" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M3.5 2h5.5l3.5 3.5V13a1.5 1.5 0 0 1-1.5 1.5h-7A1.5 1.5 0 0 1 2.5 13V3.5A1.5 1.5 0 0 1 3.5 2Z" stroke="currentColor" stroke-width="1.4"/><path d="M9 2v4h3.5" stroke="currentColor" stroke-width="1.4"/></svg>
            <div class="pv-titles">
              <span class="pv-title">{preview?.name ?? "读取中…"}</span>
              {#if preview}
                <span class="pv-sub">{preview.path}{preview.truncated ? " · 已截断" : ""}</span>
              {/if}
            </div>
            <button class="pv-close" title="关闭 (Esc)" onclick={() => (preview = null)}>×</button>
          </header>
          {#if previewLoading}
            <div class="pv-body pv-hint">加载中…</div>
          {:else if preview}
            <textarea class="pv-body" bind:value={preview.content} spellcheck="false"></textarea>
          {/if}
          <footer class="pv-foot">
            {#if preview?.truncated}
              <span class="pv-note">文件较大，仅预览前 60KB，保存已禁用。</span>
            {:else if saved}
              <span class="pv-saved">✓ 已保存</span>
            {:else}
              <span></span>
            {/if}
            <button class="pv-btn" onclick={downloadPreview} disabled={!preview}>下载</button>
            <button
              class="pv-btn primary"
              onclick={savePreview}
              disabled={!preview || preview.truncated}
              title={preview?.truncated ? "文件过大，编辑会截断内容，已禁用" : "保存并写回远端"}
            >保存</button>
          </footer>
        </div>
      </div>
    {/if}
    {/if}
  {/if}

  <input
    type="file"
    bind:this={fileInput}
    style="display:none"
    onchange={(e) => {
      const f = fileInput.files?.[0];
      if (f) doUpload(f);
      fileInput.value = "";
    }}
  />

  <!-- 拖拽调高手柄：本质是鼠标手势控件，不提供键盘等价操作 -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="fs-resize" onmousedown={startResize}></div>
</aside>

<style>
  .fs-panel {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg-panel);
    border-top: 1px solid var(--border);
    position: relative;
    overflow: hidden;
  }
  .fs-header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.25rem 0.5rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .fold {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.75rem;
    padding: 0.15rem 0.3rem;
    border-radius: var(--radius-sm);
  }
  .fold:hover {
    color: var(--fg);
    background: var(--hover);
  }
  .fs-title {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    font-weight: 600;
    font-size: 0.82rem;
    color: var(--fg);
    flex-shrink: 0;
  }
  .fs-title svg,
  .fs-title-collapsed svg {
    width: 15px;
    height: 15px;
    color: var(--accent);
  }
  .fs-title-collapsed {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.78rem;
    color: var(--fg-muted);
  }
  .fs-cwd {
    font-size: 0.72rem;
    color: var(--fg-muted);
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    flex: 1;
    min-width: 40px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    background: var(--input-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0.14rem 0.5rem;
  }
  .fs-nav {
    display: flex;
    align-items: center;
    gap: 0.15rem;
    flex-shrink: 0;
  }
  .nav-sep {
    width: 1px;
    height: 14px;
    background: var(--border);
    margin: 0 0.15rem;
  }
  .icon-btn {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    cursor: pointer;
    font-size: 0.9rem;
    width: 24px;
    height: 24px;
    border-radius: var(--radius-sm);
    padding: 0;
    line-height: 1;
    transition: background 0.12s ease, color 0.12s ease;
  }
  .icon-btn:hover:not(:disabled) {
    background: var(--hover);
    color: var(--accent);
  }
  .icon-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .icon-btn.busy {
    color: var(--warning);
  }
  .fs-hint {
    padding: 1rem;
    text-align: center;
    color: var(--fg-muted);
    font-size: 0.8rem;
  }
  .fs-error {
    padding: 0.4rem 0.7rem;
    color: var(--danger);
    font-size: 0.8rem;
    border-bottom: 1px solid var(--border);
  }
  .fs-list {
    flex: 1;
    overflow-y: auto;
    font-size: 0.8rem;
  }
  .fs-cols {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.22rem 0.6rem;
    font-size: 0.68rem;
    color: var(--fg-muted);
    letter-spacing: 0.04em;
    border-bottom: 1px solid var(--border);
    position: sticky;
    top: 0;
    background: var(--bg-panel);
    z-index: 1;
    user-select: none;
  }
  .fs-row {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.28rem 0.6rem;
    cursor: default;
    white-space: nowrap;
    border-radius: var(--radius-sm);
    margin: 0 0.15rem;
    transition: background 0.1s ease;
  }
  .fs-row:hover {
    background: var(--hover);
  }
  .fs-row.selected {
    background: var(--accent-dim, var(--active-bg));
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .fs-row:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .fs-icon {
    flex-shrink: 0;
    width: 15px;
    height: 15px;
    display: inline-flex;
  }
  .fs-icon svg {
    width: 100%;
    height: 100%;
  }
  .fs-row .fs-icon {
    color: var(--fg-muted);
  }
  .fs-row:hover .fs-icon {
    color: var(--fg);
  }
  .fs-icon.dir {
    color: var(--accent);
  }
  .fs-icon.dir:hover {
    color: var(--accent-hover);
  }
  .fs-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--fg);
  }
  .fs-size {
    width: 60px;
    text-align: right;
    color: var(--fg-muted);
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.74rem;
  }
  .fs-perms {
    width: 90px;
    color: var(--fg-muted);
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.74rem;
  }
  .fs-mtime {
    width: 150px;
    color: var(--fg-muted);
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.74rem;
  }
  .name-dialog {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.4rem 0.7rem;
    border-bottom: 1px solid var(--border);
    background: var(--warn-bg);
    font-size: 0.82rem;
  }
  .name-dialog code {
    color: var(--warning);
  }
  .name-dialog input {
    flex: 1;
    min-width: 0;
    background: var(--input-bg);
    border: 1px solid var(--border);
    color: var(--fg);
    border-radius: var(--radius-sm);
    padding: 0.3rem 0.5rem;
    font-size: 0.82rem;
  }
  .name-actions {
    display: flex;
    gap: 0.4rem;
    flex-shrink: 0;
  }
  .name-actions button {
    padding: 0.3rem 0.8rem;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--modal-bg);
    color: var(--fg);
    cursor: pointer;
    font-size: 0.8rem;
  }
  .name-actions .primary {
    background: var(--accent);
    color: #fff;
    border-color: var(--accent);
  }
  .name-actions .danger {
    background: var(--danger);
    color: #fff;
    border-color: var(--danger);
  }
  .pv-veil {
    position: fixed;
    inset: 0;
    z-index: 160;
    background: rgba(0, 0, 0, 0.45);
    backdrop-filter: blur(2px);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .pv-modal {
    width: 720px;
    max-width: 82vw;
    max-height: 82vh;
    background: var(--modal-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .pv-head {
    display: flex;
    align-items: center;
    gap: 0.55rem;
    padding: 0.55rem 0.8rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .pv-glyph {
    width: 18px;
    height: 18px;
    color: var(--accent);
    flex-shrink: 0;
  }
  .pv-titles {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .pv-title {
    font-weight: 600;
    font-size: 0.88rem;
    color: var(--fg);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pv-sub {
    font-size: 0.7rem;
    color: var(--fg-muted);
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pv-close {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-size: 1.1rem;
    cursor: pointer;
    padding: 0 0.3rem;
    border-radius: var(--radius-sm);
    flex-shrink: 0;
  }
  .pv-close:hover {
    color: var(--fg);
    background: var(--hover);
  }
  .pv-foot {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem 0.8rem;
    border-top: 1px solid var(--border);
    flex-shrink: 0;
  }
  .pv-note {
    flex: 1;
    color: var(--warning);
    font-size: 0.75rem;
  }
  .pv-saved {
    flex: 1;
    font-size: 0.78rem;
    color: var(--ok);
  }
  .pv-btn {
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--fg);
    border-radius: var(--radius-sm);
    padding: 0.28rem 0.95rem;
    font-size: 0.8rem;
    cursor: pointer;
  }
  .pv-btn:hover:not(:disabled) {
    background: var(--hover);
  }
  .pv-btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
    font-weight: 600;
  }
  .pv-btn.primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .pv-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .pv-body {
    flex: 1;
    overflow: auto;
    padding: 0.8rem;
    margin: 0;
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.8rem;
    color: var(--fg);
    white-space: pre-wrap;
    word-break: break-all;
  }
  textarea.pv-body {
    border: none;
    border-radius: 0;
    resize: none;
    background: var(--modal-bg);
    outline: none;
  }
  .pv-hint {
    text-align: center;
    color: var(--fg-muted);
  }
  .fs-resize {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 4px;
    cursor: ns-resize;
    z-index: 6;
  }
  .fs-resize:hover {
    background: var(--accent);
    opacity: 0.5;
  }
  .ctx-menu {
    position: fixed;
    z-index: 150;
    background: var(--modal-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    display: flex;
    flex-direction: column;
    padding: 0.25rem;
    min-width: 110px;
  }
  .ctx-menu button {
    border: none;
    background: transparent;
    text-align: left;
    padding: 0.4rem 0.7rem;
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-size: 0.85rem;
    color: var(--fg);
  }
  .ctx-menu button:hover {
    background: var(--hover);
  }
  .ctx-menu button.danger {
    color: var(--danger);
  }
  .ctx-veil {
    position: fixed;
    inset: 0;
    z-index: 149;
  }
</style>