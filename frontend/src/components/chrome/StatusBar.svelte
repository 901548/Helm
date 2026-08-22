<script lang="ts">
  import type { SessionInfo, SessionStatus } from "../../lib/api";

  interface Props {
    sessions: SessionInfo[];
    statuses: Record<string, SessionStatus>;
    onOpenSettings: () => void;
  }

  let { sessions, statuses, onOpenSettings } = $props<Props>();

  const connected = $derived(
    sessions.filter((s) => statuses[s.name] === "Connected").length,
  );
</script>

<footer class="statusbar">
  <span class="item">会话 {connected}/{sessions.length}</span>
  <span class="spacer"></span>
  <button class="menu" onclick={onOpenSettings}>⚙ 设置</button>
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
  }
  .menu:hover {
    background: var(--hover);
    color: var(--fg);
  }
</style>
