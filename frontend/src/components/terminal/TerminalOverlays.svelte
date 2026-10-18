<script lang="ts">
  // P126：终端区浮层集合（自 TerminalTabs 抽出）
  //
  // 这五块都是**纯展示 + 回调**，不碰 xterm 实例，抽出来让 TerminalTabs 只留
  // 「终端实例池 + 键鼠/协议接线」这一真正耦合的核心。
  // 样式随之搬入（scoped 样式不能跨组件继承），故调用方不再需要这些类。
  import type { ZTransfer } from "../../lib/types";

  interface Props {
    /** 搜索条 */
    showSearch: boolean;
    searchQuery: string;
    searchInput?: HTMLInputElement | null;
    onSearchKeydown: (e: KeyboardEvent) => void;
    onSearch: (dir: "next" | "prev") => void;
    onCloseSearch: () => void;

    /** 终端区右键菜单 */
    ctxPos: { x: number; y: number } | null;
    onCtxAction: (action: "copy" | "paste" | "search" | "clear") => void;
    onCloseCtx: () => void;

    /** 断线重连横幅 */
    showReconnect: boolean;
    onReconnect: () => void;

    /** ZMODEM 传输浮层（仅活动会话；null 表示无） */
    ztransfer: ZTransfer | null;
    onZmodemPick: (files: FileList | null) => void;
    onZmodemAbort: () => void;

    /** 空状态（无标签时） */
    showEmpty: boolean;
    sessionsCollapsed: boolean;
  }
  let {
    showSearch,
    searchQuery = $bindable(),
    searchInput = $bindable(null),
    onSearchKeydown,
    onSearch,
    onCloseSearch,
    ctxPos,
    onCtxAction,
    onCloseCtx,
    showReconnect,
    onReconnect,
    ztransfer,
    onZmodemPick,
    onZmodemAbort,
    showEmpty,
    sessionsCollapsed,
  }: Props = $props();
</script>

{#if showSearch}
  <div class="search-bar">
    <span class="search-glyph" aria-hidden="true">
      <svg viewBox="0 0 16 16" fill="none"><circle cx="7" cy="7" r="4.5" stroke="currentColor" stroke-width="1.6"/><path d="m10.5 10.5 3 3" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>
    </span>
    <input
      bind:this={searchInput}
      bind:value={searchQuery}
      aria-label="搜索终端内容"
      placeholder="搜索终端内容 (Esc 关闭)"
      onkeydown={onSearchKeydown}
    />
    <button class="s-btn" title="上一个 (↑)" onclick={() => onSearch("prev")}>↑</button>
    <button class="s-btn" title="下一个 (↓)" onclick={() => onSearch("next")}>↓</button>
    <button class="s-btn close" title="关闭" onclick={onCloseSearch}>×</button>
  </div>
{/if}

{#if ctxPos}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="term-ctx-veil" onclick={onCloseCtx} oncontextmenu={(e) => { e.preventDefault(); onCloseCtx(); }}></div>
  <div class="term-ctx" style:left={ctxPos.x + "px"} style:top={ctxPos.y + "px"}>
    <button onclick={() => onCtxAction("copy")}>复制</button>
    <button onclick={() => onCtxAction("paste")}>粘贴</button>
    <button onclick={() => onCtxAction("search")}>搜索 (Ctrl+F)</button>
    <button onclick={() => onCtxAction("clear")}>清屏</button>
  </div>
{/if}

{#if showReconnect}
  <!-- P72 断线重连横幅:活动会话断开时浮于终端顶部,一键重连 -->
  <div class="reconnect-bar">
    <span class="reconnect-text">连接已断开</span>
    <button onclick={onReconnect} title="重新连接当前会话">重新连接</button>
  </div>
{/if}

{#if ztransfer}
  <!-- P73 ZMODEM 传输浮层 -->
  {@const zt = ztransfer}
  <div class="zm-bar">
    <span class="zm-icon" aria-hidden="true">⇅</span>
    <div class="zm-info">
      <div class="zm-title">
        {zt.dir === "down" ? "下载" : zt.name ? "上传" : "服务器请求发送文件（rz）"}：{zt.name || "请选择本地文件"}
      </div>
      {#if zt.size > 0}
        <div class="zm-progress">
          <div class="zm-progress-fill" style:width={`${zt.size ? Math.min(100, (zt.done / zt.size) * 100) : 0}%`}></div>
        </div>
        <div class="zm-nums">{zt.done} / {zt.size} 字节</div>
      {/if}
      {#if zt.note}<div class="zm-note">{zt.note}</div>{/if}
    </div>
    {#if zt.dir === "up" && !zt.name}
      <label class="zm-pick">
        选择文件
        <input type="file" onchange={(e) => onZmodemPick(e.currentTarget.files)} />
      </label>
    {/if}
    <button class="zm-cancel" onclick={onZmodemAbort} title="中止传输">取消</button>
  </div>
{/if}

{#if showEmpty}
  <!-- 空状态提示:无会话标签时占据终端区,纯展示不拦事件 -->
  <div class="term-empty">
    <svg class="term-empty-glyph" viewBox="0 0 48 48" fill="none" aria-hidden="true">
      <circle cx="24" cy="24" r="13" stroke="currentColor" stroke-width="3.5"/>
      <circle cx="24" cy="24" r="4" fill="currentColor"/>
      {#each [0, 45, 90, 135, 180, 225, 270, 315] as a}
        <line
          x1={24 + 8 * Math.cos((a * Math.PI) / 180)}
          y1={24 + 8 * Math.sin((a * Math.PI) / 180)}
          x2={24 + 19 * Math.cos((a * Math.PI) / 180)}
          y2={24 + 19 * Math.sin((a * Math.PI) / 180)}
          stroke="currentColor"
          stroke-width="3.5"
          stroke-linecap="round"
        />
      {/each}
    </svg>
    <p class="term-empty-main">{sessionsCollapsed ? "点左下角「会话」展开会话列表" : "在左侧选择会话并连接"}</p>
    <p class="term-empty-sub">
      {sessionsCollapsed ? "展开后可选会话或点 ＋ 新建 · Alt+I 聚焦 AI 输入" : "Alt+I 聚焦 AI 输入 · Alt+L 查看 AI 日志"}
    </p>
  </div>
{/if}

<style>
  /* ---------- 搜索条 ---------- */
  .search-bar {
    position: absolute;
    top: 8px;
    right: 12px;
    display: flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.3rem 0.4rem;
    background: var(--modal-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    box-shadow: var(--shadow);
    z-index: 6;
  }
  .search-glyph {
    display: flex;
    color: var(--fg-muted);
  }
  .search-glyph svg {
    width: 14px;
    height: 14px;
  }
  .search-bar input {
    width: 200px;
    padding: 0.25rem 0.4rem;
    font-size: 0.78rem;
    background: var(--input-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg);
  }
  .s-btn {
    padding: 0.15rem 0.4rem;
    font-size: 0.75rem;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    cursor: pointer;
  }
  .s-btn:hover {
    background: var(--hover);
    border-color: var(--accent);
  }
  .s-btn.close {
    color: var(--fg-muted);
  }
  .s-btn.close:hover {
    color: var(--danger);
  }

  /* ---------- 终端区右键菜单 ---------- */
  .term-ctx-veil {
    position: absolute;
    inset: 0;
    z-index: 7;
  }
  .term-ctx {
    position: absolute;
    z-index: 8;
    min-width: 150px;
    padding: 0.25rem;
    background: var(--modal-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    box-shadow: var(--shadow);
    display: flex;
    flex-direction: column;
  }
  .term-ctx button {
    padding: 0.35rem 0.6rem;
    text-align: left;
    font-size: 0.8rem;
    background: transparent;
    border: none;
    border-radius: var(--radius-sm);
    color: var(--fg);
    cursor: pointer;
  }
  .term-ctx button:hover {
    background: var(--hover);
    color: var(--accent);
  }

  /* ---------- 断线重连横幅 ---------- */
  .reconnect-bar {
    position: absolute;
    top: 10px;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 0.7rem;
    padding: 0.4rem 0.8rem;
    background: var(--warn-bg);
    border: 1px solid var(--warning);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    z-index: 5;
  }
  .reconnect-text {
    font-size: 0.8rem;
    color: var(--fg);
  }
  .reconnect-bar button {
    padding: 0.2rem 0.6rem;
    font-size: 0.78rem;
    font-weight: 600;
    background: var(--accent);
    color: #fff;
    border: none;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  /* ---------- ZMODEM 浮层 ---------- */
  .zm-bar {
    position: absolute;
    bottom: 12px;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 0.7rem;
    min-width: 320px;
    max-width: 80%;
    padding: 0.5rem 0.8rem;
    background: var(--modal-bg);
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    z-index: 6;
  }
  .zm-icon {
    font-size: 1.1rem;
    color: var(--accent);
  }
  .zm-info {
    flex: 1;
    min-width: 0;
  }
  .zm-title {
    font-size: 0.78rem;
    color: var(--fg);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .zm-progress {
    height: 4px;
    margin-top: 0.3rem;
    background: var(--track-bg);
    border-radius: 2px;
    overflow: hidden;
  }
  .zm-progress-fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.15s linear;
  }
  .zm-nums,
  .zm-note {
    margin-top: 0.15rem;
    font-size: 0.7rem;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .zm-pick {
    padding: 0.25rem 0.6rem;
    font-size: 0.76rem;
    background: var(--accent);
    color: #fff;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .zm-pick input {
    display: none;
  }
  .zm-cancel {
    padding: 0.25rem 0.6rem;
    font-size: 0.76rem;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg-muted);
    cursor: pointer;
  }
  .zm-cancel:hover {
    border-color: var(--danger);
    color: var(--danger);
  }

  /* ---------- 空状态 ---------- */
  .term-empty {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.35rem;
    color: var(--fg-muted);
    pointer-events: none;
  }
  .term-empty-glyph {
    width: 48px;
    height: 48px;
    opacity: 0.35;
    margin-bottom: 0.4rem;
  }
  .term-empty-main {
    margin: 0;
    font-size: 0.88rem;
    color: var(--fg);
  }
  .term-empty-sub {
    margin: 0;
    font-size: 0.76rem;
  }
</style>
