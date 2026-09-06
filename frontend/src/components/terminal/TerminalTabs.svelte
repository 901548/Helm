<script lang="ts">
  import { onMount, tick } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { SearchAddon } from "@xterm/addon-search";
  import { WebglAddon } from "@xterm/addon-webgl";
  import { WebLinksAddon } from "@xterm/addon-web-links";
  // P73 ZMODEM（rz/sz）：浏览器 bundle 挂 window.Zmodem（Sentry + Browser.send_files）
  import "zmodem.js/dist/zmodem.js";
  const ZModem = () => (window as any).Zmodem;
  import type { ITheme } from "@xterm/xterm";
  import "@xterm/xterm/css/xterm.css";
  import * as api from "../../lib/api";
  import type { AiCard, AiLogEntry, AiState, SessionKind, SessionStatus } from "../../lib/api";
  import { normalizePwd, parseOscPwd, stripAnsi as stripAnsiLib } from "../../lib/osc";
  import SysMonitor from "./SysMonitor.svelte";
  import AiCopilot from "./AiCopilot.svelte";

  interface Props {
    tabs: string[];
    activeTab: string | null;
    theme: "light" | "dark";
    kinds?: Record<string, SessionKind>;
    status?: SessionStatus;
    onReconnect: () => void;
    onSelect: (name: string) => void;
    onClose: (name: string) => void;
    onAdd: () => void;
    onCd?: (name: string, target: string) => void;
    onPwd?: (name: string, pwd: string) => void;
    aiMode: "qa" | "agent";
    aiBusy: boolean;
    aiState?: AiState;
    aiCards: AiCard[];
    aiTaskText: string;
    aiSummary: { text: string; ok: boolean } | null;
    aiStreamOpen: boolean;
    aiFocusSeq: number;
    aiLog: AiLogEntry[];
    logOpen: boolean;
    aiEcho: { seq: number; name: string; text: string }[];
    aiThinking?: string;
    termFontSize?: number;
    termScrollback?: number;
    onModeChange: (m: "qa" | "agent") => void;    onApprove: () => void;
    onReject: () => void;
    onEditPlan: (commands: string[]) => void;
    onStop: () => void;
    onClear: () => void;
    onOpenTask: () => void;
    onToggleLog: () => void;
    onToggleStream: () => void;
    onDockSubmit: (text: string, container?: string | null, termContext?: string | null) => void;
  }

  let {
    tabs,
    activeTab,
    theme,
    kinds = {},
    status = "Disconnected",
    onReconnect,
    onSelect,
    onClose,
    onAdd,
    onCd,
    onPwd,
    aiMode,
    aiBusy,
    aiState = "idle",
    aiCards,
    aiTaskText,
    aiSummary,
    aiStreamOpen,
    aiFocusSeq,
    aiLog,
    logOpen,
    aiEcho,
    aiThinking = "",
    termFontSize = 14,
    termScrollback = 5000,
    onModeChange,
    onApprove,
    onReject,
    onEditPlan,
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
    {
      term: Terminal;
      fit: FitAddon;
      search: SearchAddon;
      open: boolean;
      webgl?: WebglAddon;
      zsentry?: any;
      zsession?: any;
    }
  >();

  function createTerminal(name: string): Terminal {
    const term = new Terminal({
      // P75：JetBrains Mono Medium——真实中等字重，WebGL 渲染下饱满不发糊
      // （Cascadia 400 经 WebGL 偏细，合成 600 发糊；CJK 回落系统雅黑）
      fontFamily: '"JetBrains Mono", "Cascadia Mono", monospace',
      fontSize: termFontSize,
      fontWeight: 500,
      lineHeight: 1.2,
      cursorBlink: true,
      convertEol: true,
      scrollback: termScrollback,
      theme: XTERM_THEMES[theme],
    });
    const fit = new FitAddon();
    const search = new SearchAddon();
    term.loadAddon(fit);
    term.loadAddon(search);
    // P68 输出中的 URL 可点击：经后端 open_external 用系统浏览器打开（只放行 http/https），
    // 不用默认 window.open——WebView2 里那会导航走应用页面本身
    term.loadAddon(new WebLinksAddon((e, uri) => {
      e.preventDefault();
      api.openExternal(uri).catch(() => {});
    }));
    // P73 ZMODEM 哨兵：入站字节全部经 consume（普通输出透传上屏，ZMODEM 帧拦截进协议栈）；
    // 协议应答经 sender 回写远端（record:false，训练日志只记人类键入）
    const zsentry = new (ZModem().Sentry)({
      to_terminal: (octets: Uint8Array) => term.write(octets),
      sender: (octets: Uint8Array | number[]) => {
        const raw = octets instanceof Uint8Array ? octets : Uint8Array.from(octets);
        api.sendInputRaw(name, raw);
      },
      on_detect: (d: any) => onZmodemDetect(name, d),
      on_retract: () => clearZmodem(name),
    });
    term.onData((data) => {
      // P73：ZMODEM 会话进行中吞掉键入（协议字节专用通道，防破坏帧序）
      if (terminals.get(name)?.zsession) return;
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
    terminals.set(name, { term, fit, search, open: false, zsentry });
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
        // P68 WebGL 硬件渲染（大输出滚动性能）；上下文创建失败（上下文数超限/驱动问题/
        // 容器隐藏）时静默回退 DOM 渲染器，不影响功能
        try {
          const webgl = new WebglAddon();
          e.term.loadAddon(webgl);
          e.webgl = webgl;
        } catch {
          /* DOM renderer fallback */
        }
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
      .then((text) => {
        // P68 运维安全：多行粘贴经 confirm 二次把关（防误执行粘贴块中的破坏性命令）
        const lines = text.split(/\r\n|\r|\n/).filter((l) => l.trim().length > 0).length;
        if (lines > 1 && !window.confirm(`粘贴内容包含 ${lines} 行命令，将逐行发送到终端执行。确定继续？`)) {
          return;
        }
        api.sendActiveInput(new TextEncoder().encode(text));
      })
      .catch(() => {});
  }

  // ===== P73 ZMODEM（rz/sz）=====
  // 传输浮层状态（响应式渲染）；字节累积与协议对象存 terminals 条目（非响应式）
  type ZTransfer = { dir: "up" | "down"; name: string; size: number; done: number; note: string };
  let ztransfers = $state<Record<string, ZTransfer | null>>({});
  function setZTransfer(name: string, t: ZTransfer | null) {
    ztransfers = { ...ztransfers, [name]: t };
  }

  function clearZmodem(name: string) {
    const e = terminals.get(name);
    if (e) e.zsession = undefined;
    setZTransfer(name, null);
  }

  function b64Of(bytes: Uint8Array): string {
    let bin = "";
    const CH = 0x8000;
    for (let i = 0; i < bytes.length; i += CH) {
      bin += String.fromCharCode(...bytes.subarray(i, i + CH));
    }
    return btoa(bin);
  }

  function onZmodemDetect(name: string, d: any) {
    const role = d.get_session_role(); // 'receive'=远端 sz；'send'=远端 rz
    const zsession = d.confirm();
    const e = terminals.get(name);
    if (!e) return;
    e.zsession = zsession;

    if (role === "receive") {
      // sz：远端发文件过来 → 收齐经后端落 ~/Downloads/helm-zmodem/
      zsession.on("offer", (xfer: any) => {
        const det = xfer.get_details();
        const fname = det.name || "file";
        const chunks: Uint8Array[] = [];
        let done = 0;
        setZTransfer(name, { dir: "down", name: fname, size: det.size ?? 0, done: 0, note: "" });
        xfer.on("input", (p: Uint8Array) => {
          chunks.push(p);
          done += p.length;
          setZTransfer(name, { dir: "down", name: fname, size: det.size ?? 0, done, note: "" });
        });
        xfer.accept().then(() => {
          const total = chunks.reduce((n, c) => n + c.length, 0);
          const all = new Uint8Array(total);
          let off = 0;
          for (const c of chunks) {
            all.set(c, off);
            off += c.length;
          }
          api
            .zmodemSave(fname, b64Of(all))
            .then((path) => setZTransfer(name, { dir: "down", name: fname, size: total, done: total, note: `已保存：${path}` }))
            .catch((err) => setZTransfer(name, { dir: "down", name: fname, size: total, done: total, note: `保存失败：${String(err)}` }));
        });
      });
      zsession.on("session_end", () => {
        // 保留 note（保存路径提示）几秒后清
        const t = ztransfers[name];
        if (t) setTimeout(() => clearZmodem(name), 4000);
        else clearZmodem(name);
      });
      // P73 关键：Receive 会话必须 start() 才会发 ZRINIT，否则服务器永远等不到握手
      zsession.start();
    } else {
      // rz：远端等我们发文件 → 浮层内嵌文件选择器
      setZTransfer(name, { dir: "up", name: "", size: 0, done: 0, note: "" });
      zsession.on("session_end", () => clearZmodem(name));
    }
  }

  function zmodemPick(name: string, files: FileList | null) {
    const e = terminals.get(name);
    const zsession = e?.zsession;
    if (!zsession) return;
    if (!files || files.length === 0) {
      zsession.abort?.();
      clearZmodem(name);
      return;
    }
    const file = files[0];
    setZTransfer(name, { dir: "up", name: file.name, size: file.size, done: 0, note: "" });
    ZModem().Browser.send_files(zsession, [file], {
      on_progress: (_obj: any, _xfer: any, chunk: Uint8Array) => {
        const t = ztransfers[name];
        if (t) setZTransfer(name, { ...t, done: Math.min(t.size, t.done + chunk.length) });
      },
    })
      .then(() => {
        setZTransfer(name, { dir: "up", name: file.name, size: file.size, done: file.size, note: "发送完成" });
        setTimeout(() => clearZmodem(name), 2500);
      })
      .catch((err: unknown) => {
        setZTransfer(name, { dir: "up", name: file.name, size: file.size, done: 0, note: `发送失败：${String(err)}` });
      });
  }

  // P86 终端上下文感知：提取活动会话屏幕最后 40 行（纯文本，截 4000 字）
  function extractTermContext(): string | null {
    const e = activeTab ? terminals.get(activeTab) : undefined;
    if (!e) return null;
    const buf = e.term.buffer.active;
    const total = buf.length;
    const lines: string[] = [];
    for (let i = Math.max(0, total - 42); i < total; i++) {
      const line = buf.getLine(i);
      if (line) lines.push(line.translateToString(true));
    }
    while (lines.length && !lines[lines.length - 1].trim()) lines.pop();
    if (!lines.length) return null;
    return lines.join("\n").slice(-4000);
  }
  function dockSubmit(text: string, container?: string | null) {
    onDockSubmit(text, container, extractTermContext());
  }

  function zmodemAbort(name: string) {
    const e = terminals.get(name);
    e?.zsession?.abort?.();
    clearZmodem(name);
  }

  // P68 终端区右键菜单（复制/粘贴/搜索/清屏）
  let tctx = $state<{ x: number; y: number } | null>(null);  function openTermCtx(e: MouseEvent) {
    // 仅终端区域（.xterm 内）拦截；tabbar/搜索框等走默认行为
    if (!(e.target as HTMLElement | null)?.closest?.(".xterm")) return;
    e.preventDefault();
    tctx = { x: e.clientX, y: e.clientY };
  }
  function termCtx(action: "copy" | "paste" | "search" | "clear") {
    tctx = null;
    const term = activeTab ? terminals.get(activeTab)?.term : undefined;
    if (action === "copy") copySelection();
    else if (action === "paste") paste();
    else if (action === "search") {
      showSearch = true;
      setTimeout(() => searchBar?.focus(), 0);
    } else if (action === "clear") term?.clear();
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
    } else if (ctrl && (key === "=" || key === "+")) {
      // P68 字体缩放：放大（终端聚焦或应用区域都响应，缩放是全局观感）
      if (fromAppField) return;
      e.preventDefault();
      zoomOffset = Math.min(14, zoomOffset + 1);
    } else if (ctrl && key === "-") {
      if (fromAppField) return;
      e.preventDefault();
      zoomOffset = Math.max(-6, zoomOffset - 1);
    } else if (ctrl && key === "0") {
      if (fromAppField) return;
      e.preventDefault();
      zoomOffset = 0;
    } else if (ctrl && key === "tab" && tabs.length > 1) {
      // P68 标签键盘导航：Ctrl+Tab 下一个 / Ctrl+Shift+Tab 上一个（循环）
      e.preventDefault();
      const idx = activeTab ? tabs.indexOf(activeTab) : -1;
      const dir = e.shiftKey ? -1 : 1;
      const next = tabs[(((idx + dir) % tabs.length) + tabs.length) % tabs.length];
      if (next && next !== activeTab) onSelect(next);
    } else if (ctrl && key >= "1" && key <= "9" && !e.shiftKey && !e.altKey) {
      // P68 Ctrl+1..9 直达第 N 个标签
      const t = tabs[Number(key) - 1];
      if (t) {
        e.preventDefault();
        if (t !== activeTab) onSelect(t);
      }
    } else if (e.key === "F11") {
      // P74 全屏切换（全局响应，不区分输入焦点）
      e.preventDefault();
      api.toggleFullscreen().catch(() => {});
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
    // P73 调试：dev 模式暴露终端表（哨兵/传输状态核查用），生产构建无此全局
    if (import.meta.env.DEV) {
      (window as any).__helmTerms = terminals;
    }
    const unsubP = api.onTerminalOutput(({ name, data }) => {
      const e = terminals.get(name);
      if (!e) return;
      const bytes = new Uint8Array(data);
      // P73：输出统一过 ZMODEM 哨兵——普通字节透传上屏，ZMODEM 帧被拦截进协议栈
      if (e.zsentry) {
        e.zsentry.consume(bytes);
      } else {
        e.term.write(bytes);
      }
      feedEcho(name, bytes);
    });
    const onResize = () => fitActive();
    window.addEventListener("resize", onResize);
    // P68 Ctrl+滚轮字体缩放（passive:false 才能 preventDefault 挡住 WebView 页面缩放）
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      zoomOffset = Math.min(14, Math.max(-6, zoomOffset + (e.deltaY < 0 ? 1 : -1)));
    };
    termArea?.addEventListener("wheel", onWheel, { passive: false });
    const ro = new ResizeObserver(onResize);
    if (termArea) ro.observe(termArea);
    window.addEventListener("keydown", handleKey, true);

    return () => {
      unsubP.then((fn) => fn());
      window.removeEventListener("resize", onResize);
      termArea?.removeEventListener("wheel", onWheel);
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
      // P74：WebGL 画布清屏色不随运行时主题刷新（切主题后仍是旧深色）。
      // dispose 与重挂必须隔帧——同步重挂会撞上渲染器拆除中间态直接崩溃
      if (e.webgl) {
        const old = e.webgl;
        e.webgl = undefined;
        try {
          old.dispose();
        } catch {
          /* ignore */
        }
        requestAnimationFrame(() => {
          try {
            const w = new WebglAddon();
            e.term.loadAddon(w);
            e.webgl = w;
          } catch {
            /* DOM 渲染回退 */
          }
        });
      }
    }
  });

  // P68 临时字体缩放（Ctrl+滚轮 / Ctrl+=/-/0）：叠加在设置基准之上，不持久化
  let zoomOffset = $state(0);

  // P67 终端设置即时生效：字体变化后须 refit，缓冲行数 xterm 支持运行时调整
  $effect(() => {
    const size = Math.min(28, Math.max(8, termFontSize + zoomOffset));
    const back = termScrollback;
    for (const e of terminals.values()) {
      e.term.options.fontSize = size;
      e.term.options.scrollback = back;
    }
    tick().then(() => fitActive());
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
  <button class="tab-btn" title="搜索 (Ctrl+F)" onclick={() => (showSearch = true)}>
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="11" cy="11" r="7" /><path d="m21 21-4.3-4.3" /></svg>
  </button>
  <button class="tab-add" title="新建会话" onclick={onAdd}>
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14" /></svg>
  </button>
</div>

<SysMonitor {activeTab} />

<!-- 右键菜单捕获层:容器级 contextmenu 拦截终端区右键,无独立语义,与 ctx-veil 同类 -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="term-area" bind:this={termArea} oncontextmenu={openTermCtx}>
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

  {#if tctx}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="term-ctx-veil" onclick={() => (tctx = null)} oncontextmenu={(e) => { e.preventDefault(); tctx = null; }}></div>
    <div class="term-ctx" style:left={tctx.x + "px"} style:top={tctx.y + "px"}>
      <button onclick={() => termCtx("copy")}>复制</button>
      <button onclick={() => termCtx("paste")}>粘贴</button>
      <button onclick={() => termCtx("search")}>搜索 (Ctrl+F)</button>
      <button onclick={() => termCtx("clear")}>清屏</button>
    </div>
  {/if}

  {#if tabs.length > 0 && status === "Disconnected"}
    <!-- P72 断线重连横幅:活动会话断开时浮于终端顶部,一键重连 -->
    <div class="reconnect-bar">
      <span class="reconnect-text">连接已断开</span>
      <button onclick={onReconnect} title="重新连接当前会话">重新连接</button>
    </div>
  {/if}

  {#if activeTab && ztransfers[activeTab]}
    <!-- P73 ZMODEM 传输浮层 -->
    {@const zt = ztransfers[activeTab]}
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
          <input type="file" onchange={(e) => zmodemPick(activeTab, e.currentTarget.files)} />
        </label>
      {/if}
      <button class="zm-cancel" onclick={() => zmodemAbort(activeTab)} title="中止传输">取消</button>
    </div>
  {/if}

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
  {aiState}
  kind={kinds[activeTab ?? ""] ?? "linux"}
  name={activeTab ?? ""}
  {status}
  cards={aiCards}
  taskText={aiTaskText}
  summary={aiSummary}
  streamOpen={aiStreamOpen}
  focusSeq={aiFocusSeq}
  {logOpen}
  thinking={aiThinking}
  onSubmit={dockSubmit}
  onStop={onStop}
  onApprove={onApprove}
  onReject={onReject}
  onEditPlan={onEditPlan}
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
    cursor: pointer;
    padding: 0 0.55rem;
    height: 100%;
    border-radius: var(--radius-sm);
    display: inline-flex;
    align-items: center;
    justify-content: center;
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
    cursor: pointer;
    padding: 0 0.8rem;
    height: 100%;
    border-radius: var(--radius-sm);
    display: inline-flex;
    align-items: center;
    justify-content: center;
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
  /* P68 终端区右键菜单（样式对齐 SessionPanel ctx-menu） */
  .term-ctx {
    position: fixed;
    z-index: 100;
    background: var(--modal-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    display: flex;
    flex-direction: column;
    padding: 0.25rem;
    min-width: 130px;
  }
  .term-ctx button {
    border: none;
    background: transparent;
    text-align: left;
    padding: 0.45rem 0.7rem;
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-size: 0.9rem;
    color: var(--fg);
  }
  .term-ctx button:hover {
    background: var(--hover);
  }
  .term-ctx-veil {
    position: fixed;
    inset: 0;
    z-index: 99;
  }
  /* P72 断线重连横幅:终端区顶部居中浮层 */
  .reconnect-bar {
    position: absolute;
    top: 0.5rem;
    left: 50%;
    transform: translateX(-50%);
    z-index: 6;
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.3rem 0.4rem 0.3rem 0.8rem;
    background: var(--modal-bg);
    border: 1px solid var(--warning);
    border-radius: 999px;
    box-shadow: var(--shadow);
  }
  .reconnect-text {
    font-size: 0.78rem;
    color: var(--warning);
    white-space: nowrap;
  }
  .reconnect-bar button {
    padding: 0.22rem 0.8rem;
    font-size: 0.78rem;
    font-weight: 600;
    background: var(--accent);
    color: #fff;
    border: none;
    border-radius: 999px;
    cursor: pointer;
  }
  .reconnect-bar button:hover {
    background: var(--accent-hover);
  }
  /* P73 ZMODEM 传输浮层 */
  .zm-bar {
    position: absolute;
    top: 0.5rem;
    right: 0.6rem;
    z-index: 7;
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.4rem 0.6rem;
    background: var(--modal-bg);
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    max-width: 440px;
  }
  .zm-icon {
    color: var(--accent);
    font-size: 1rem;
  }
  .zm-info {
    min-width: 0;
    flex: 1;
  }
  .zm-title {
    font-size: 0.78rem;
    color: var(--fg);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .zm-progress {
    margin-top: 0.25rem;
    height: 4px;
    border-radius: 2px;
    background: var(--track-bg);
    overflow: hidden;
  }
  .zm-progress-fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.2s ease;
  }
  .zm-nums {
    margin-top: 0.15rem;
    font-size: 0.68rem;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .zm-note {
    margin-top: 0.15rem;
    font-size: 0.7rem;
    color: var(--ok);
    word-break: break-all;
  }
  .zm-pick {
    flex-shrink: 0;
    padding: 0.25rem 0.8rem;
    font-size: 0.78rem;
    font-weight: 600;
    background: var(--accent);
    color: #fff;
    border-radius: 999px;
    cursor: pointer;
  }
  .zm-pick input {
    display: none;
  }
  .zm-cancel {
    flex-shrink: 0;
    padding: 0.25rem 0.7rem;
    font-size: 0.75rem;
    background: transparent;
    color: var(--fg-muted);
    border: 1px solid var(--border);
    border-radius: 999px;
    cursor: pointer;
  }
  .zm-cancel:hover {
    color: var(--fg);
    background: var(--hover);
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
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.72rem;
  }
  :global(.xterm) {
    height: 100%;
  }
</style>
