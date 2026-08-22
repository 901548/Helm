<script lang="ts">
  import type { SessionInfo, SessionKind, SessionStatus } from "../../lib/api";

  function kindLabel(k: SessionKind | undefined): string {
    return k === "windows" ? "Win" : k === "rdp" ? "RDP" : "";
  }

  interface Props {
    sessions: SessionInfo[];
    statuses: Record<string, SessionStatus>;
    tabs: string[];
    onSelect: (name: string) => void;
    onConnect: (name: string) => void;
    onDisconnect: (name: string) => void;
    onRdp: (name: string) => void;
    onNew: () => void;
    onEdit: (info: SessionInfo) => void;
    onDelete: (name: string) => void;
  }

  let { sessions, statuses, tabs, onSelect, onConnect, onDisconnect, onRdp, onNew, onEdit, onDelete } =
    $props<Props>();

  let ctxSession = $state<string | null>(null);
  let ctxPos = $state<{ x: number; y: number } | null>(null);
  let collapsed = $state(false);

  function openContext(e: MouseEvent, name: string) {
    e.preventDefault();
    ctxSession = name;
    ctxPos = { x: e.clientX, y: e.clientY };
  }

  function ctx(action: "connect" | "disconnect" | "edit" | "delete") {
    if (!ctxSession) return;
    const name = ctxSession;
    ctxSession = null;
    ctxPos = null;
    if (action === "connect") {
      const s = sessions.find((x) => x.name === name);
      if (s?.kind === "rdp") onRdp(name);
      else onConnect(name);
    } else if (action === "disconnect") onDisconnect(name);
    else if (action === "edit") {
      const info = sessions.find((s) => s.name === name);
      if (info) onEdit(info);
    } else if (action === "delete") onDelete(name);
  }

  function statusDot(st: SessionStatus | undefined): string {
    return st === "Connected" ? "ok" : st === "Connecting" ? "connecting" : "off";
  }

  function statusText(st: SessionStatus | undefined): string {
    return st === "Connected" ? "已连接" : st === "Connecting" ? "连接中" : "未连接";
  }
</script>

<aside class="panel" class:collapsed>
  <header class="panel-header">
    {#if collapsed}
      <button
        class="expand-btn"
        title="展开会话栏"
        onclick={() => (collapsed = false)}
      >▶</button>
    {:else}
      <span class="title">会话</span>
      <div class="head-actions">
        <button
          class="icon-btn"
          title="折叠"
          onclick={() => (collapsed = true)}
        >◂</button>
        <button class="icon-btn accent" title="新建会话" onclick={onNew}>＋</button>
      </div>
    {/if}
  </header>

  {#if !collapsed}
    <div class="session-list">
      {#each sessions as s (s.name)}
        {@const st = statuses[s.name]}
        {@const isActive = tabs.includes(s.name)}
        <div
          class="session-item"
          class:active={isActive}
          role="button"
          tabindex="0"
          onclick={() => onSelect(s.name)}
          ondblclick={() =>
            s.kind === "rdp" ? onRdp(s.name) : (st === "Connected" ? onDisconnect(s.name) : onConnect(s.name))
          }
          oncontextmenu={(e) => openContext(e, s.name)}
          onkeydown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              onSelect(s.name);
            }
          }}
        >
          <span class="dot {statusDot(st)}" title={statusText(st)}></span>
          <div class="s-body">
            <div class="s-row">
              <span class="name">{s.name}</span>
              {#if kindLabel(s.kind)}
                <span class="badge {s.kind}" title={s.kind === "windows" ? "Windows（SSH）" : "远程桌面（RDP）"}>
                  {kindLabel(s.kind)}
                </span>
              {/if}
            </div>
            <span class="meta">{s.user}@{s.host}</span>
          </div>
        </div>
      {/each}
    </div>

    {#if sessions.length === 0}
      <p class="empty">暂无会话，点击 ＋ 新建</p>
    {/if}
  {/if}

  {#if ctxPos}
    <div class="ctx-menu" style:left={ctxPos.x + "px"} style:top={ctxPos.y + "px"}>
      <button onclick={() => ctx("connect")}>连接</button>
      <button onclick={() => ctx("disconnect")}>断开</button>
      <button onclick={() => ctx("edit")}>编辑</button>
      <button class="danger" onclick={() => ctx("delete")}>删除</button>
    </div>
    <!-- 遮罩：仅用于点击/右键空白处关闭菜单，无独立语义 -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="ctx-veil" onclick={() => (ctxSession = null, ctxPos = null)} oncontextmenu={(e) => { e.preventDefault(); ctxSession = null; ctxPos = null; }}></div>
  {/if}
</aside>

<style>
  .panel {
    width: var(--sp, 22%);
    min-width: 0;
    background: var(--bg-panel);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
    transition: width 0.15s ease;
  }
  .panel.collapsed {
    width: 44px;
  }
  .panel.collapsed .panel-header {
    justify-content: center;
    padding: 0.35rem 0;
    flex: 1;
    border-bottom: none;
  }
  .expand-btn {
    border: none;
    background: transparent;
    color: var(--accent);
    cursor: pointer;
    font-size: 1.4rem;
    line-height: 1;
    width: 100%;
    height: 100%;
    padding: 0;
    border-radius: var(--radius-sm);
  }
  .expand-btn:hover {
    background: var(--hover);
    color: var(--accent-hover);
  }

  .panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.4rem;
    padding: 0.55rem 0.7rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .title {
    font-weight: 600;
    font-size: 0.9rem;
    color: var(--fg);
  }

  .head-actions {
    display: flex;
    gap: 0.15rem;
  }

  .icon-btn {
    border: none;
    background: transparent;
    cursor: pointer;
    font-size: 1rem;
    width: 26px;
    height: 26px;
    border-radius: var(--radius-sm);
    line-height: 1;
    color: var(--fg-muted);
    padding: 0;
  }
  .icon-btn:hover {
    background: var(--hover);
    color: var(--fg);
  }
  .icon-btn.accent {
    color: var(--accent);
  }

  .session-list {
    list-style: none;
    margin: 0;
    padding: 0.4rem;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .session-item {
    display: flex;
    align-items: flex-start;
    gap: 0.55rem;
    padding: 0.45rem 0.6rem;
    border-radius: var(--radius-sm);
    cursor: pointer;
    user-select: none;
    transition: background 0.12s ease;
  }
  .session-item:hover {
    background: var(--hover);
  }
  .session-item.active {
    background: var(--active-bg);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .session-item:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }

  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    flex-shrink: 0;
    margin-top: 5px;
  }
  .dot.ok {
    background: var(--ok);
    box-shadow: 0 0 5px color-mix(in srgb, var(--ok) 65%, transparent);
  }
  .dot.connecting {
    background: var(--warning);
    animation: pulse 1s infinite;
  }
  .dot.off {
    background: #c4c9d0;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }

  .s-body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .s-row {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    min-width: 0;
  }

  .badge {
    font-size: 0.62rem;
    line-height: 1;
    padding: 0.18rem 0.3rem;
    border-radius: var(--radius-sm);
    background: var(--panel-glow, rgba(128, 128, 160, 0.18));
    color: var(--fg-muted);
    flex-shrink: 0;
  }
  .badge.windows {
    color: #4aa3ff;
  }
  .badge.rdp {
    color: #c792ea;
  }

  .name {
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex-shrink: 1;
  }

  .meta {
    color: var(--fg-muted);
    font-size: 0.7rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .empty {
    color: var(--fg-muted);
    text-align: center;
    font-size: 0.85rem;
    padding: 1rem;
  }

  .ctx-menu {
    position: fixed;
    z-index: 100;
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
    padding: 0.45rem 0.7rem;
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-size: 0.9rem;
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
    z-index: 99;
  }
</style>
