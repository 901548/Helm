<script lang="ts">
  import type { SessionInfo, SessionStatus } from "../../lib/api";

  interface Props {
    sessions: SessionInfo[];
    statuses: Record<string, SessionStatus>;
    /** P119：会话栏折叠态（该指示器同时是折叠开关） */
    sessionsCollapsed?: boolean;
    onToggleSessions: () => void;
    onOpenSettings: () => void;
    onOpenHistory: () => void;
  }

  let { sessions, statuses, sessionsCollapsed = false, onToggleSessions, onOpenSettings, onOpenHistory } =
    $props<Props>();

  const connected = $derived(
    sessions.filter((s) => statuses[s.name] === "Connected").length,
  );
</script>

<footer class="statusbar">
  <!-- P119：会话计数即折叠开关（用户定案）——信息与操作合一，位置常驻且与历史/设置同排 -->
  <button
    class="menu sessions-toggle"
    onclick={onToggleSessions}
    title={sessionsCollapsed ? "展开会话栏" : "折叠会话栏"}
    aria-expanded={!sessionsCollapsed}
  >
    <span>会话 {connected}/{sessions.length}</span>
    <svg
      class="chev"
      class:flip={!sessionsCollapsed}
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    ><path d="M9 6l6 6-6 6" /></svg>
  </button>
  <span class="spacer"></span>
  <button class="menu" onclick={onOpenHistory} title="操作历史（训练数据）">
    <span style="font-size:13px">📜</span>
    <span>历史</span>
  </button>
    <button class="menu" onclick={onOpenSettings}>
    <svg
      width="13"
      height="13"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    ><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" /></svg>
    <span>设置</span>
  </button>
</footer>

<style>
  .statusbar {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    background: var(--statusbar-bg);
    color: var(--statusbar-fg);
    border-top: 1px solid var(--statusbar-border);
    padding: 0.3rem 0.8rem;
    font-size: 0.8rem;
    flex-shrink: 0;
    user-select: none;
  }
  .spacer {
    flex: 1;
  }
  .menu {
    border: none;
    background: transparent;
    color: var(--statusbar-fg);
    cursor: pointer;
    font-size: 0.8rem;
    padding: 0.15rem 0.5rem;
    border-radius: 4px;
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
  }
  .menu:hover {
    background: var(--hover);
    color: var(--fg);
  }
  /* P119：会话开关——比普通菜单项更实一点（它是布局控制，不只是导航） */
  .sessions-toggle {
    color: var(--statusbar-fg);
    font-weight: 600;
  }
  .sessions-toggle .chev {
    display: block;
    color: var(--accent);
    transition: transform 0.15s ease;
  }
  .sessions-toggle .chev.flip {
    transform: rotate(180deg);
  }
</style>
