<script lang="ts">
  import type { SessionInfo, SessionStatus } from "../../lib/api";

  interface Props {
    sessions: SessionInfo[];
    statuses: Record<string, SessionStatus>;
    onOpenSettings: () => void;
    onOpenHistory: () => void;
  }

  let { sessions, statuses, onOpenSettings, onOpenHistory } = $props<Props>();

  const connected = $derived(
    sessions.filter((s) => statuses[s.name] === "Connected").length,
  );
</script>

<footer class="statusbar">
  <span class="item">会话 {connected}/{sessions.length}</span>
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
  .item {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
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
</style>
