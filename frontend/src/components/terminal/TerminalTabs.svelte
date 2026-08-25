<script lang="ts">
  import { onMount, tick } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { SearchAddon } from "@xterm/addon-search";
  import type { ITheme } from "@xterm/xterm";
  import "@xterm/xterm/css/xterm.css";
  import * as api from "../../lib/api";
  import type { AiCard, AiLogEntry, SessionKind } from "../../lib/api";
  import { normalizePwd, parseOscPwd, stripAnsi as stripAnsiLib } from "../../lib/osc";
  import SysMonitor from "./SysMonitor.svelte";
  import AiCopilot from "./AiCopilot.svelte";

  interface Props {
    tabs: string[];
    activeTab: string | null;
    theme: "light" | "dark";
    kinds?: Record<string, SessionKind>;
    onSelect: (name: string) => void;
    onClose: (name: string) => void;
    onAdd: () => void;
    onCd?: (name: string, target: string) => void;
    onPwd?: (name: string, pwd: string) => void;
    aiMode: "qa" | "agent";
    aiBusy: boolean;
    aiCards: AiCard[];
    aiTaskText: string;
    aiSummary: { text: string; ok: boolean } | null;
    aiStreamOpen: boolean;
    aiFocusSeq: number;
    aiLog: AiLogEntry[];
    logOpen: boolean;
    aiEcho: { seq: number; name: string; text: string }[];
    aiThinking?: string;
    onModeChange: (m: "qa" | "agent") => void;
    onApprove: () => void;
    onReject: () => void;
    onStop: () => void;
    onClear: () => void;
    onOpenTask: () => void;
    onToggleLog: () => void;
    onToggleStream: () => void;
    onDockSubmit: (text: string) => void;
  }

  let {
    tabs,
    activeTab,
    theme,
    kinds = {},
    onSelect,
    onClose,
    onAdd,
    onCd,
    onPwd,
    aiMode,
    aiBusy,
    aiCards,
    aiTaskText,
    aiSummary,
    aiStreamOpen,
    aiFocusSeq,
    aiLog,
    logOpen,
    aiEcho,
    aiThinking = "",
    onModeChange,
    onApprove,
    onReject,
    onStop,
    onClear,
    onOpenTask,
    onToggleLog,
    onToggleStream,
    onDockSubmit,
  } = $props<Props>();

  const echoBuffs = new Map<string, string>();
  const decoders = new Map<string, TextDecoder>();
  const lastCdFire = new Map<string, { target: string; t: number }>();
  const pendingCd = new Map<string, { target: string; t: number }>();
  // P24:OSC7 权威 PWD 标记(`ESC]7;helm:PWD ESC\`)状态
  const markerBufs = new Map<string, string>();
  const hasMarker = new Map<string, boolean>();
  const lastPwd = new Map<string, string>();

  const stripAnsi = stripAnsiLib;

  function fireCd(name: string, target: string) {
    const now = Date.now();
    const p = lastCdFire.get(name);
    if (p && p.target === target && now - p.t < 800) return;
    lastCdFire.set(name, { target, t: now });
    onCd?.(name, target);
  }

  /// 从原始流中截取 OSC7 `ESC]7;helm:PWD BEL` 标记(P24 权威 PWD)。
  /// 解析逻辑在 lib/osc(纯函数,带测试);此处仅维护每会话的跨包片段缓冲。
  function extractOscPwd(name: string, raw: string): string | null {
    const { pwd, rest } = parseOscPwd(markerBufs.get(name) ?? "", raw);
    markerBufs.set(name, rest);
    return pwd;
  }

  /// P24:按权威 OSC7 PWD 同步文件面板。bash 每次重画提示符都发标记,
  /// 故 PWD 未变(普通命令)时去重跳过,仅真实目录变化才触发。
  function syncOscPwd(name: string, pwd: string) {
    hasMarker.set(name, true);
    const norm = normalizePwd(pwd);
    if (lastPwd.get(name) === norm) return;
    lastPwd.set(name, norm);
    fireCd(name, norm);
    // 权威 PWD 上报 App（Agent 任务初始目录用）
    onPwd?.(name, norm);
  }

  function feedEcho(name: string, data: Uint8Array) {
    // P32:Windows 会话无 OSC7/bash 回显语义,整体跳过 cd 跟踪(文件面板联动不适用)
    if (kinds[name] === "windows") return;
    let dec = decoders.get(name);
    if (!dec) {
      dec = new TextDecoder();
      decoders.set(name, dec);
    }
    const rawText = dec.decode(data, { stream: true });
    // P24:OSC 标记为权威路径;解析到后跳过回显解析(仅非 bash 会话回退用)
    const oscPwd = extractOscPwd(name, rawText);
    if (oscPwd) syncOscPwd(name, oscPwd);
    if (hasMarker.get(name)) return;
    const text = stripAnsi(rawText);
    let tail = (echoBuffs.get(name) ?? "") + text;
    if (tail.length > 16384) tail = tail.slice(-16384);
    const lines = tail.split(/\r\n|\n|\r/);
    let keep = "";
    if (!/[\r\n]$/.test(tail)) {
      keep = lines.pop() ?? "";
    }
    for (const rawLine of [...lines, keep]) {
      const line = rawLine.trim();
      if (!line) continue;
      const now = Date.now();
      const pend = pendingCd.get(name);
      // 回显行匹配 `]# cd <target>` / `$ cd <target>` 时登记候选(读迭渐进段后以最终组成为准)
      const echoM = line.match(/\]\s*[#$]\s*cd(?:[ \t]+([^\s;&|]+))?[ \t]*$/);
      if (echoM) {
        const tgt = echoM[1] ?? "~";
        pendingCd.set(name, { target: tgt === "-" ? "" : tgt.replace(/^['"]|['"]$/g, ""), t: now });
        continue;
      }
      if (!pend) continue;
      // 失败:报错行(如 `-bash: cd: /op: No such file…`)→ 作废
      if (/: cd(:| )|\bNo such file\b|\bnot found\b/.test(line)) {
        pendingCd.delete(name);
        continue;
      }
      // 成功:紧随其后的新提示符(`…]# ` / `…]$ `)确认 cd 已生效
      const confirmOk = /\]\s*[#$]\s*$/.test(line) || (/^\S+@\S+[: ].*[#$]\s*$/.test(line) && now - pend.t < 3000);
      if (confirmOk) {
        if (pend.target) fireCd(name, pend.target);
        pendingCd.delete(name);
        continue;
      }
      // 其他输出 → 非简单 cd,作废,避免误触发
      pendingCd.delete(name);
    }
    echoBuffs.set(name, keep);
  }

  const XTERM_THEMES: Record<"light" | "dark", ITheme> = {
    light: {
      background: "#ffffff",
      foreground: "#1a1a2e",
      cursor: "#2f6fed",
      cursorAccent: "#ffffff",
      selectionBackground: "#2f6fed40",
      selectionForeground: "#1a1a2e",
      black: "#24292e",
      red: "#cf222e",
      green: "#116329",
      yellow: "#4d2d00",
      blue: "#0969da",
      magenta: "#8250df",
      cyan: "#1b7c83",
      white: "#6e7781",
      brightBlack: "#57606a",
      brightRed: "#82071e",
      brightGreen: "#116329",
      brightYellow: "#4d2d00",
      brightBlue: "#0969da",
      brightMagenta: "#8250df",
      brightCyan: "#1b7c83",
      brightWhite: "#ffffff",
    },
    dark: {
      background: "#0a0d11",
      foreground: "#d6d9de",
      cursor: "#4c8dff",
      cursorAccent: "#0a0d11",
      selectionBackground: "#4c8dff66",
      black: "#484f58",
      red: "#ff7b72",
      green: "#3fb950",
      yellow: "#d29922",
      blue: "#58a6ff",
      magenta: "#bc8cff",
      cyan: "#39c5cf",
      white: "#b1bac4",
      brightBlack: "#6e7681",
      brightRed: "#ffa198",
      brightGreen: "#56d364",
      brightYellow: "#e3b341",
      brightBlue: "#79c0ff",
      brightMagenta: "#d2a8ff",
      brightCyan: "#56d4dd",
      brightWhite: "#f0f6fc",
    },
  };

  let termArea: HTMLDivElement;
  let searchBar = $state<HTMLInputElement | null>(null);
  let showSearch = $state(false);
  let searchQuery = $state("");
  let logBody = $state<HTMLDivElement | undefined>(undefined);

  const terminals = new Map<
    string,
    { term: Terminal; fit: FitAddon; search: SearchAddon; open: boolean }
  >();

  function createTerminal(name: string): Terminal {
    const term = new Terminal({
      fontFamily: '"Cascadia Mono", monospace',
      fontSize: 14,
      lineHeight: 1.2,
      cursorBlink: true,
      convertEol: true,
      scrollback: 5000,
      theme: XTERM_THEMES[theme],
    });
    const fit = new FitAddon();
    const search = new SearchAddon();
    term.loadAddon(fit);
    term.loadAddon(search);
    term.onData((data) => {
      api.sendActiveInput(new TextEncoder().encode(data));
    });
    term.onSelectionChange(() => {
      if (term.hasSelection()) {
        const sel = term.getSelection();
        try {
          navigator.clipboard.writeText(sel);
        } catch {
          /* ignore */
        }
      }
    });
    terminals.set(name, { term, fit, search, open: false });
    return term;
  }

  function syncContainers() {
    if (!termArea) return;
    for (const [name, e] of terminals) {
      if (!tabs.includes(name)) {
        e.term.dispose();
        terminals.delete(name);
        echoBuffs.delete(name);
        decoders.delete(name);
        lastCdFire.delete(name);
        pendingCd.delete(name);
        markerBufs.delete(name);
        hasMarker.delete(name);
        lastPwd.delete(name);
      }
    }
    const containers = termArea.querySelectorAll<HTMLDivElement>("[data-term]");
    for (const el of containers) {
      const name = el.dataset.term!;
      let e = terminals.get(name);
      if (!e) {
        createTerminal(name);
        e = terminals.get(name)!;
      }
      if (!e.open) {
        e.term.open(el);
        e.open = true;
        try {
          e.fit.fit();
        } catch {
          /* ignore */
        }
      }
    }
    fitActive();
  }

  function fitActive() {
    const e = activeTab ? terminals.get(activeTab) : undefined;
    if (e && e.open) {
      try {
        e.fit.fit();
        api.resizeSessions(e.term.cols, e.term.rows);
      } catch {
        /* ignore */
      }
    }
  }

  function focusActive() {
    const e = activeTab ? terminals.get(activeTab) : undefined;
    if (e && e.open) e.term.focus();
  }

  function copySelection() {
    const e = activeTab ? terminals.get(activeTab) : undefined;
    if (!e) return;
    if (e.term.hasSelection()) {
      navigator.clipboard.writeText(e.term.getSelection());
    }
  }

  function paste() {
    navigator.clipboard
      .readText()
      .then((text) => api.sendActiveInput(new TextEncoder().encode(text)))
      .catch(() => {});
  }

  function doSearch(direction: "next" | "prev") {
    const e = activeTab ? terminals.get(activeTab) : undefined;
    if (!e || !searchQuery) return;
    if (direction === "prev") e.search.findPrevious(searchQuery);
    else e.search.findNext(searchQuery, { incremental: false });
  }

  function closeSearch() {
    showSearch = false;
    searchQuery = "";
    focusActive();
  }

  function onSearchKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      doSearch(e.shiftKey ? "prev" : "next");
    } else if (e.key === "Escape") {
      closeSearch();
    }
  }

  function handleKey(e: KeyboardEvent) {
    if (showSearch) return;
    const ctrl = e.ctrlKey || e.metaKey;
    const key = e.key.toLowerCase();
    const t = e.target as HTMLElement | null;
    // xterm 的隐藏 textarea(.xterm 容器内)是"终端输入"而非应用输入框,
    // 必须排除,否则终端聚焦时 Ctrl+F/Ctrl+C(选区复制)/Ctrl+V 全军覆没。
    const inTerminal = !!t?.closest?.(".xterm");
    const fromAppField =
      (t?.tagName === "INPUT" || t?.tagName === "TEXTAREA") && !inTerminal;
    if (ctrl && key === "f") {
      if (fromAppField) return;
      e.preventDefault();
      showSearch = true;
      setTimeout(() => searchBar?.focus(), 0);
    } else if (ctrl && key === "c") {
      // 有选区才拦截做复制；无选区时放行，让 Ctrl+C 作为 SIGINT 发给远端
      if (fromAppField) return;
      const term = activeTab ? terminals.get(activeTab)?.term : undefined;
      if (term && term.hasSelection()) {
        e.preventDefault();
        e.stopPropagation();
        copySelection();
      }
    } else if (ctrl && key === "v") {
      if (fromAppField) return;
      e.preventDefault();
      e.stopPropagation();
      paste();
    } else if (e.altKey && key === "i" && !fromAppField) {
      // 聚焦 AI 命令条输入框（⌥I）;输入框内不劫持
      e.preventDefault();
      onOpenTask();
    } else if (e.altKey && key === "l" && !fromAppField) {
      // AI 日志（⌥L）
      e.preventDefault();
      onToggleLog();
    }
  }

  /// AI 回显：把 App 推送的文本按 seq 顺序写入活动终端(队列消费,不丢批次中间条目)
  let lastEchoSeq = 0;
  $effect(() => {
    for (const e of aiEcho) {
      if (e.seq <= lastEchoSeq) continue;
      lastEchoSeq = e.seq;
      if (!activeTab || e.name !== activeTab) continue;
      const term = terminals.get(activeTab)?.term;
      if (term) term.write(new TextEncoder().encode(e.text));
    }
  });

  /// AI 日志：打开时滚到底（新条目到达也滚动）
  $effect(() => {
    aiLog;
    logOpen;
    if (logOpen) {
      requestAnimationFrame(() => {
        if (logBody) logBody.scrollTop = logBody.scrollHeight;
      });
    }
  });

  onMount(() => {
    const unsubP = api.onTerminalOutput(({ name, data }) => {
      const e = terminals.get(name);
      if (!e) return;
      const bytes = new Uint8Array(data);
      e.term.write(bytes);
      feedEcho(name, bytes);
    });
    const onResize = () => fitActive();
    window.addEventListener("resize", onResize);
    const ro = new ResizeObserver(onResize);
    if (termArea) ro.observe(termArea);
    window.addEventListener("keydown", handleKey, true);

    return () => {
      unsubP.then((fn) => fn());
      window.removeEventListener("resize", onResize);
      ro.disconnect();
      window.removeEventListener("keydown", handleKey, true);
      for (const e of terminals.values()) e.term.dispose();
      terminals.clear();
    };
  });

  $effect(() => {
    tabs;
    activeTab;
    tick().then(() => {
      syncContainers();
      focusActive();
    });
  });

  $effect(() => {
    const t = theme;
    for (const e of terminals.values()) {
      e.term.options.theme = XTERM_THEMES[t];
    }
  });
</script>

<div class="tabbar">
  <div class="tabs">
    {#each tabs as t (t)}
      <div
        class:active={t === activeTab}
        class="tab"
        role="button"
        tabindex="0"
        onclick={() => onSelect(t)}
        onauxclick={(e) => {
          if (e.button === 1) onClose(t);
        }}
        onkeydown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onSelect(t);
          }
        }}
      >
        <span class="tab-name">{t}</span>
        <button
          class="tab-close"
          title="关闭"
          onclick={(e) => {
            e.stopPropagation();
            onClose(t);
          }}
        >×</button>
      </div>
    {/each}
  </div>
  <button class="tab-btn" title="搜索 (Ctrl+F)" onclick={() => (showSearch = true)}>🔍</button>
  <button class="tab-add" title="新建会话" onclick={onAdd}>＋</button>
</div>

<SysMonitor {activeTab} />

<div class="term-area" bind:this={termArea}>
  {#if showSearch}
    <div class="search-bar">
      <span class="search-glyph" aria-hidden="true">
        <svg viewBox="0 0 16 16" fill="none"><circle cx="7" cy="7" r="4.5" stroke="currentColor" stroke-width="1.6"/><path d="m10.5 10.5 3 3" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>
      </span>
      <input
        bind:this={searchBar}
        bind:value={searchQuery}
        aria-label="搜索终端内容"
        placeholder="搜索终端内容 (Esc 关闭)"
        onkeydown={onSearchKeydown}
      />
      <button class="s-btn" title="上一个 (↑)" onclick={() => doSearch("prev")}>↑</button>
      <button class="s-btn" title="下一个 (↓)" onclick={() => doSearch("next")}>↓</button>
      <button class="s-btn close" title="关闭" onclick={closeSearch}>×</button>
    </div>
  {/if}
  {#each tabs as t (t)}
    <div class="term-container" class:active={t === activeTab} data-term={t}></div>
  {/each}

  {#if tabs.length === 0}
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
      <p class="term-empty-main">在左侧选择会话并连接</p>
      <p class="term-empty-sub">Alt+I 聚焦 AI 输入 · Alt+L 查看 AI 日志</p>
    </div>
  {/if}

  {#if logOpen}
    <div class="ai-log-wrap">
      <div class="ai-log-head">
        <span>AI 日志</span>
        <button class="s-btn" onclick={onToggleLog}>×</button>
      </div>
      <div class="ai-log-body" bind:this={logBody}>
        {#if aiLog.length === 0}
          <p class="ai-log-empty">暂无日志</p>
        {/if}
        {#each aiLog as entry (entry.id)}
          <div class="ai-log-entry" class:err={entry.type === "cmd" && !entry.success}>
            <span class="ai-log-name">{entry.name}</span>
            {#if entry.type === "cmd"}
              <code class="ai-log-cmd">{entry.command}</code>
              {#if entry.message}
                <span class="ai-log-msg">{entry.message}</span>
              {/if}
              {#if entry.output}
                <pre class="ai-log-out">{entry.output}</pre>
              {/if}
            {:else}
              <span class="ai-log-msg">{entry.message}</span>
            {/if}
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>

<AiCopilot
  mode={aiMode}
  busy={aiBusy}
  cards={aiCards}
  taskText={aiTaskText}
  summary={aiSummary}
  streamOpen={aiStreamOpen}
  focusSeq={aiFocusSeq}
  {logOpen}
  thinking={aiThinking}
  onSubmit={onDockSubmit}
  onStop={onStop}
  onApprove={onApprove}
  onReject={onReject}
  onModeChange={onModeChange}
  onToggleStream={onToggleStream}
  onToggleLog={onToggleLog}
  onClear={onClear}
/>

<style>
  .tabbar {
    display: flex;
    align-items: center;
    background: var(--tabbar-bg);
    border-bottom: 1px solid var(--tab-border);
    min-height: 36px;
    flex-shrink: 0;
  }
  .tabs {
    display: flex;
    overflow-x: auto;
    flex: 1;
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.4rem 0.85rem;
    color: var(--tabbar-fg);
    cursor: pointer;
    user-select: none;
    white-space: nowrap;
    border-right: 1px solid var(--tab-border);
    font-size: 0.85rem;
    position: relative;
    transition: background 0.12s ease, color 0.12s ease;
  }
  .tab:hover:not(.active) {
    background: var(--hover);
    color: var(--fg);
  }
  .tab.active {
    background: var(--tab-active-bg);
    color: var(--fg);
  }
  .tab.active::before {
    content: "";
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 2px;
    background: var(--accent);
  }
  .tab:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .tab-name {
    max-width: 140px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tab-close {
    border: none;
    background: transparent;
    color: inherit;
    cursor: pointer;
    font-size: 0.9rem;
    line-height: 1;
    padding: 0 0.15rem;
    border-radius: 4px;
  }  .tab-close:hover {
    background: var(--hover);
    color: var(--danger);
  }
  .tab-btn {
    border: none;
    background: transparent;
    color: var(--tabbar-fg);
    font-size: 0.9rem;
    cursor: pointer;
    padding: 0 0.5rem;
    height: 100%;
    border-radius: var(--radius-sm);
    transition: color 0.12s ease, background 0.12s ease;
  }
  .tab-btn:hover {
    color: var(--accent);
    background: var(--hover);
  }
  .tab-add {
    border: none;
    background: transparent;
    color: var(--tabbar-fg);
    font-size: 1.05rem;
    cursor: pointer;
    padding: 0 0.8rem;
    height: 100%;
    border-radius: var(--radius-sm);
    transition: color 0.12s ease, background 0.12s ease;
  }
  .tab-add:hover {
    color: var(--accent);
    background: var(--hover);
  }
  .term-empty {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    color: var(--fg-muted);
    pointer-events: none;
    user-select: none;
  }
  .term-empty-glyph {
    width: 52px;
    height: 52px;
    opacity: 0.45;
    margin-bottom: 0.4rem;
  }
  .term-empty-main {
    margin: 0;
    font-size: 0.95rem;
  }
  .term-empty-sub {
    margin: 0;
    font-size: 0.78rem;
    opacity: 0.7;
  }
  .search-bar {
    position: absolute;
    top: 10px;
    right: 12px;
    z-index: 8;
    display: flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0.3rem 0.4rem;
    background: var(--modal-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    width: 340px;
    max-width: calc(100% - 24px);
  }
  .search-glyph {
    width: 14px;
    height: 14px;
    color: var(--fg-muted);
    flex-shrink: 0;
    display: inline-flex;
    margin-left: 0.3rem;
  }
  .search-glyph svg {
    width: 100%;
    height: 100%;
  }
  .search-bar input {
    flex: 1;
    min-width: 0;
    background: transparent;
    border: none;
    color: var(--fg);
    padding: 0.25rem 0.3rem;
    font-size: 0.85rem;
  }
  .search-bar input:focus {
    outline: none;
  }
  .s-btn {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    border-radius: var(--radius-sm);
    padding: 0.15rem 0.5rem;
    cursor: pointer;
    font-size: 0.85rem;
    flex-shrink: 0;
  }
  .s-btn:hover {
    color: var(--fg);
    background: var(--hover);
  }
  .s-btn.close:hover {
    color: var(--danger);
  }
  .term-area {
    flex: 1;
    min-height: 0;
    position: relative;
  }
  .term-container {
    position: absolute;
    inset: 0;
    display: none;
    padding: 0.35rem;
  }
  .term-container.active {
    display: block;
  }
  .ai-log-wrap {
    position: absolute;
    inset: 0;
    z-index: 5;
    background: var(--bg-panel);
    display: flex;
    flex-direction: column;
  }
  .ai-log-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.45rem 0.7rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--fg);
  }
  .ai-log-head .s-btn {
    font-weight: 400;
  }
  .ai-log-body {
    flex: 1;
    overflow-y: auto;
    padding: 0.5rem 0.7rem;
    font-size: 0.8rem;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .ai-log-empty {
    color: var(--fg-muted);
    text-align: center;
    margin-top: 2rem;
  }
  .ai-log-entry {
    border: 1px solid var(--border);
    border-left: 2px solid var(--ok);
    border-radius: var(--radius-sm);
    background: var(--bubble-bg);
    padding: 0.35rem 0.55rem;
  }
  .ai-log-entry.err {
    border-left-color: var(--danger);
  }
  .ai-log-name {
    color: var(--accent);
    font-size: 0.7rem;
    margin-right: 0.4rem;
  }
  .ai-log-cmd {
    display: inline-block;
    background: var(--input-bg);
    border-radius: var(--radius-sm);
    padding: 0.1rem 0.4rem;
    color: var(--fg);
    margin-bottom: 0.2rem;
    word-break: break-all;
  }
  .ai-log-msg {
    color: var(--fg-muted);
    font-size: 0.75rem;
    margin-left: 0.3rem;
  }
  .ai-log-out {
    background: var(--term-bg);
    color: var(--term-fg);
    border-radius: var(--radius-sm);
    padding: 0.3rem 0.4rem;
    margin: 0.25rem 0 0;
    white-space: pre-wrap;
    word-break: break-all;
    max-height: 220px;
    overflow: auto;
    font-family: "Cascadia Mono", monospace;
    font-size: 0.72rem;
  }
  :global(.xterm) {
    height: 100%;
  }
</style>
