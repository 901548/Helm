<script lang="ts">
  import { onMount } from "svelte";
  import SessionPanel from "./components/sessions/SessionPanel.svelte";
  import SessionForm from "./components/sessions/SessionForm.svelte";
  import TerminalTabs from "./components/terminal/TerminalTabs.svelte";
  import FileBrowser from "./components/files/FileBrowser.svelte";
  import SettingsModal from "./components/chrome/SettingsModal.svelte";
  import StatusBar from "./components/chrome/StatusBar.svelte";
  import * as api from "./lib/api";
  import type { AiCard, AiLogEntry, AiConfig, SessionInfo, SessionKind, SessionStatus, Theme, UiConfig } from "./lib/api";

  let sessions = $state<SessionInfo[]>([]);
  let statuses = $state<Record<string, SessionStatus>>({});
  let tabs = $state<string[]>([]);
  let activeTab = $state<string | null>(null);
  // 多会话：按会话名记录忙碌态，面板/提交只取当前活动会话的那一份
  let aiBusy = $state<Record<string, boolean>>({});
  // §8.7.1 唯一状态机：按会话记录当前执行态（Idle/Parsing/Planning/AwaitingConfirm/Executing/ReadingBack）
  let aiState = $state<Record<string, string>>({});
  let aiMode = $state<"qa" | "agent">("qa");
  let aiConfig = $state<AiConfig | null>(null);
  let uiConfig = $state<UiConfig | null>(null);
  // P39 AI 常驻命令条 + 活动流状态
  let aiCards = $state<AiCard[]>([]);
  let aiTaskText = $state("");
  let aiSummary = $state<{ text: string; ok: boolean } | null>(null);
  let aiStreamOpen = $state(false);
  let aiFocusSeq = $state(0);
  let logOpen = $state(false);
  let aiLog = $state<AiLogEntry[]>([]);
  let aiEcho = $state<{ seq: number; name: string; text: string }[]>([]);
  // 推理型模型思考过程：实时累积展示，不混入最终答案卡片
  let aiThinking = $state("");
  let pwds = $state<Record<string, string>>({});
  let echoSeq = 0;
  let logId = 0;
  let cardId = 0;

  let showSessionForm = $state(false);
  let editingSession = $state<SessionInfo | null>(null);
  let showSettings = $state(false);

  let sessionsPanelPct = $state(22);
  let dockSessions = $state(true);
  let theme = $state<Theme>("dark");
  let systemDark = $state(false);
  let cdReq = $state<{ name: string; target: string; seq: number } | null>(null);
  let cdSeq = 0;

  const resolvedTheme = $derived<"light" | "dark">(
    theme === "system" ? (systemDark ? "dark" : "light") : theme,
  );

  // P32:会话 → 平台类型映射（供联动/监控/文件面板按平台门控）
  const kinds = $derived<Record<string, SessionKind>>(
    Object.fromEntries(sessions.map((s) => [s.name, s.kind ?? "linux"])),
  );

  $effect(() => {
    document.documentElement.dataset.theme = resolvedTheme;
    document.documentElement.style.colorScheme = resolvedTheme;
  });

  async function refreshStatuses() {
    await Promise.all(
      sessions.map(async (s) => {
        try {
          statuses[s.name] = await api.sessionStatus(s.name);
        } catch {
          statuses[s.name] = "Disconnected";
        }
      }),
    );
  }

  async function refreshAiMode(name?: string) {
    try {
      aiMode = await api.aiMode(name ?? activeTab ?? "");
    } catch {
      /* ignore */
    }
  }

  async function refreshUiConfig() {
    try {
      uiConfig = await api.getUiConfig();
      sessionsPanelPct = uiConfig.sessions_panel_pct;
      dockSessions = uiConfig.dock_sessions;
      theme = uiConfig.theme;
    } catch {
      /* ignore */
    }
  }

  onMount(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    systemDark = mq.matches;
    const onSystemChange = (e: MediaQueryListEvent) => (systemDark = e.matches);
    mq.addEventListener("change", onSystemChange);
    api.listSessions().then((list) => {
      sessions = list;
      refreshStatuses();
    });
    refreshAiMode();
    refreshUiConfig();
    api.getAiConfig().then((c) => (aiConfig = c));

    const unsubConn = api.onConnection((p) => {
      if (p.status === "connected") {
        statuses[p.name] = "Connected";
        if (!tabs.includes(p.name)) {
          tabs = [...tabs, p.name];
        }
        activeTab = p.name;
        api.setActive(p.name);
      } else {
        statuses[p.name] = "Disconnected";
        if (p.status === "failed") {
          // 连接失败:建标签并把错误回显到终端,否则只有灰点、失败原因不可见
          if (!tabs.includes(p.name)) {
            tabs = [...tabs, p.name];
          }
          if (activeTab !== p.name) {
            activeTab = p.name;
            api.setActive(p.name);
          }
          pushEcho(p.name, `\r\n\x1b[1;31m[连接失败]\x1b[0m ${p.error || "未知错误"}\r\n`);
        }
        if (p.status === "disconnected") {
          // 连接意外断开：保留标签以便重新连接，但活动状态让给已连接会话
          if (activeTab === p.name) {
            activeTab = pickNextActive(p.name);
            if (activeTab) api.setActive(activeTab);
            else api.clearActive();
          }
        }
      }
    });
    const unsubAi = api.onAi((p) => {
      // 多会话下，AI 活动面板只反映当前活动会话；终端镜像则写回到各自会话
      const isActive = p.name === activeTab;
      switch (p.kind) {
        case "busy":
          // 多会话：忙碌按会话命名，互不覆盖；面板只读当前会话那一份
          aiBusy[p.name] = p.busy;
          break;
        case "streaming": {
          // QA 流式追加进活动流卡片;Agent 模式的模型原始输出不展示(等解析后的命令卡片)
          if (!p.text || aiMode !== "qa" || !isActive) break;
          const last = aiCards[aiCards.length - 1];
          if (last && last.kind === "qa" && !last.done) {
            aiCards = [...aiCards.slice(0, -1), { ...last, text: last.text + p.text }];
          } else {
            aiCards = [...aiCards, { id: ++cardId, kind: "qa", text: p.text, done: false }];
          }
          break;
        }
        case "reasoning": {
          // 推理型模型思考过程：仅活动会话时实时展示（QA 卡未完成时可见）
          if (!isActive) break;
          aiThinking = (aiThinking + (p.text || "")).slice(-4000);
          break;
        }
        case "commandStep": {
          // 仅活动会话更新 AI 活动面板；命令仍镜像到其所属会话的终端（含后台并行）
          if (isActive) {
            const idx = [...aiCards]
              .reverse()
              .findIndex((c) => c.kind === "step" && c.command === p.command && ["running", "confirm", "skipped"].includes(c.status));
            if (idx >= 0) {
              const i = aiCards.length - 1 - idx;
              const c = aiCards[i] as (typeof aiCards)[number] & { kind: "step" };
              aiCards = [
                ...aiCards.slice(0, i),
                { ...c, status: p.success ? "ok" : "fail", message: p.message, output: p.output || undefined },
                ...aiCards.slice(i + 1),
              ];
            } else {
              aiCards = [
                ...aiCards,
                { id: ++cardId, kind: "step", command: p.command, status: p.success ? "ok" : "fail", message: p.message, output: p.output || undefined },
              ];
            }
            aiStreamOpen = true;
            addLog({
              id: ++logId,
              name: p.name,
              type: "cmd",
              command: p.command,
              success: p.success,
              message: p.message,
              output: p.output,
            });
          }
          // 镜像到各自会话的终端：让用户直观看到每条命令及其状态（agent 仍走独立后台通道真实执行）
          if (p.name && p.command) {
            pushEcho(
              p.name,
              p.success
                ? `\r\n\x1b[36m[AI] $ \x1b[0m${p.command}\x1b[90m ✓\x1b[0m\r\n`
                : `\r\n\x1b[36m[AI] $ \x1b[0m${p.command}\x1b[31m ✗${p.message ? ` ${p.message}` : ""}\x1b[0m\r\n`,
            );
            if (p.output) {
              const limited =
                p.output.length > 800 ? `${p.output.slice(0, 800)}\n…[已截断]` : p.output;
              pushEcho(p.name, `\x1b[90m${limited}\x1b[0m\r\n`);
            }
          }
          break;
        }
        case "pendingCommand":
          // 待确认命令镜像到所属会话终端；确认按钮只在活动会话面板显示
          if (p.name && p.command) {
            pushEcho(p.name, `\r\n\x1b[33m[AI] ⏸ 待确认: \x1b[0m${p.command}\r\n`);
          }
          if (isActive) {
            aiCards = [
              ...aiCards,
              { id: ++cardId, kind: "step", command: p.command, status: "confirm", level: p.level, reason: p.reason },
            ];
            aiStreamOpen = true;
          }
          break;
        case "planning":
          // §8.7.2 整份计划卡：一次展示该步全部命令，整份确认/编辑/放弃
          if (isActive && p.commands?.length) {
            aiCards = [
              ...aiCards,
              { id: ++cardId, kind: "plan", commands: p.commands, status: "plan" },
            ];
            aiStreamOpen = true;
          }
          break;
        case "state":
          // §8.7.1 唯一状态机广播：目前仅用于驱动 busy UI，按会话记录
          aiState[p.name] = p.state;
          break;
        case "done":
          if (isActive) {
            aiSummary = { text: p.message, ok: true };
            finishQaCard();
            addLog({
              id: ++logId,
              name: p.name,
              type: "info",
              command: "",
              success: true,
              message: "✓ " + p.message,
              output: "",
            });
          }
          if (aiMode === "agent" && p.name) {
            pushEcho(p.name, `\r\n\x1b[32m[AI] ✓ ${p.message}\x1b[0m\r\n`);
          }
          break;
        case "error":
          if (isActive) {
            aiSummary = { text: p.message, ok: false };
            finishQaCard();
            addLog({
              id: ++logId,
              name: p.name,
              type: "info",
              command: "",
              success: false,
              message: "✗ " + p.message,
              output: "",
            });
          }
          if (aiMode === "agent" && p.name) {
            pushEcho(p.name, `\r\n\x1b[31m[AI] ✗ ${p.message}\x1b[0m\r\n`);
          }
          break;
      }
    });
    return () => {
      mq.removeEventListener("change", onSystemChange);
      unsubConn.then((fn) => fn());
      unsubAi.then((fn) => fn());
    };
  });

  async function selectSession(name: string) {
    // P32：rdp 会话不是 SSH，不建终端标签（P33 接入 mstsc 控屏）
    if (kinds[name] === "rdp") return;
    activeTab = name;
    api.setActive(name);
    // 每会话模式隔离：切到该会话时加载它自己的 AI 模式
    refreshAiMode(name);
    if (!tabs.includes(name)) {
      // 未打开终端标签时，先连接
      connectSession(name);
    }
  }

  async function connectSession(name: string) {
    if (kinds[name] === "rdp") return;
    if (statuses[name] === "Connected") return;
    statuses[name] = "Connecting";
    try {
      await api.connectSession(name, 80, 24);
    } catch (e) {
      statuses[name] = "Disconnected";
      console.error(e);
    }
  }

  /// 关闭/断开标签后挑下一个活动标签:优先已连接的会话,
  /// 避免把键盘输入路由到未连接会话;无候选则清空活动状态。
  function pickNextActive(exclude: string): string | null {
    return tabs.find((t) => t !== exclude && statuses[t] === "Connected") ?? null;
  }

  async function disconnectSession(name: string) {
    try {
      await api.disconnectSession(name);
    } catch {
      /* ignore */
    }
    statuses[name] = "Disconnected";
    tabs = tabs.filter((t) => t !== name);
    if (activeTab === name) {
      activeTab = pickNextActive(name);
      if (activeTab) api.setActive(activeTab);
      else api.clearActive();
    }
  }

  // P33：远程桌面控屏——生成 .rdp 并拉起系统 mstsc
  async function rdpConnect(name: string) {
    try {
      await api.rdpConnect(name);
    } catch (e) {
      console.error(e);
    }
  }

  function openNewSession() {
    editingSession = null;
    showSessionForm = true;
  }

  function openEditSession(info: SessionInfo) {
    editingSession = info;
    showSessionForm = true;
  }

  async function submitSession(info: SessionInfo, oldName: string | null) {
    try {
      if (oldName) {
        await api.updateSession(oldName, info);
        tabs = tabs.map((t) => (t === oldName ? info.name : t));
        const st = statuses[oldName];
        delete statuses[oldName];
        statuses[info.name] = st;
        // 改名迁移权威 PWD,否则 Agent 初始目录退化为 "/"
        if (pwds[oldName] !== undefined) {
          pwds[info.name] = pwds[oldName];
          delete pwds[oldName];
        }
        if (activeTab === oldName) {
          activeTab = info.name;
          api.setActive(info.name);
        }
      } else {
        await api.addSession(info);
      }
      sessions = await api.listSessions();
      showSessionForm = false;
      return null;
    } catch (e) {
      return String(e);
    }
  }

  async function deleteSession(name: string) {
    try {
      await api.disconnectSession(name);
      await api.deleteSession(name);
    } catch {
      /* ignore */
    }
    sessions = sessions.filter((s) => s.name !== name);
    delete statuses[name];
    delete pwds[name];
    tabs = tabs.filter((t) => t !== name);
    if (activeTab === name) {
      activeTab = pickNextActive(name);
      if (activeTab) api.setActive(activeTab);
      else api.clearActive();
    }
  }

  function onTabClose(name: string) {
    disconnectSession(name);
  }

  function handleCd(name: string, target: string) {
    cdReq = { name, target, seq: ++cdSeq };
  }

  function handlePwd(name: string, pwd: string) {
    pwds[name] = pwd;
  }

  function pushEcho(name: string, text: string) {
    // 队列而非单槽:同一批次内多个事件都保留,靠递增 seq 由消费端按序写入,
    // 防止密集 streaming 事件只留最后一个导致丢字。超限裁剪旧条目(视为已消费)。
    aiEcho = [...aiEcho.slice(-199), { seq: ++echoSeq, name, text }];
  }

  function addLog(entry: AiLogEntry) {
    aiLog = [...aiLog.slice(-199), entry];
  }

  /// QA 流式卡片收尾(done/error 时)
  function finishQaCard() {
    const last = aiCards[aiCards.length - 1];
    if (last && last.kind === "qa" && !last.done) {
      aiCards = [...aiCards.slice(0, -1), { ...last, done: true }];
    }
  }

  /// AI 命令条提交:重置活动流 + 调后端;Agent 任务在终端留一行锚点
  /// `container`：Docker 会话运行时目标容器（§8.7.4）
  async function submitFromDock(text: string, container?: string | null) {
    if (!text.trim() || aiBusy[activeTab ?? ""]) return;
    aiCards = aiMode === "qa" ? [{ id: ++cardId, kind: "qa", text: "", done: false }] : [];
    aiTaskText = text;
    aiSummary = null;
    aiStreamOpen = true;
    aiThinking = "";
    const name = activeTab ?? "";
    if (aiMode === "agent" && name) {
      pushEcho(name, `\r\n\x1b[90m[AI] 任务: ${text}\x1b[0m\r\n`);
    }
    const pwd = pwds[name] ?? "";
    try {
      await api.aiSubmit(name, text, pwd || undefined, container || null);
    } catch (e) {
      aiSummary = { text: String(e), ok: false };
      finishQaCard();
      addLog({
        id: ++logId,
        name,
        type: "info",
        command: "",
        success: false,
        message: "✗ " + e,
        output: "",
      });
    }
  }

  function handleModeChange(m: "qa" | "agent") {
    aiMode = m;
    api.aiSetMode(activeTab ?? "", m).catch(() => {});
  }

  /// Alt+I:聚焦命令条输入框
  function focusAi() {
    aiFocusSeq++;
  }

  function toggleLog() {
    logOpen = !logOpen;
  }

  function toggleStream() {
    if (aiCards.length || aiSummary || aiTaskText) aiStreamOpen = !aiStreamOpen;
  }

  async function decide(approve: boolean) {
    // 乐观更新确认卡片;拒绝时后端会回发 CommandStep(已跳过)统一收口
    // 优先找逐条确认卡(step/confirm),否则找整份计划卡(plan/plan)
    let i = -1;
    const ci = [...aiCards].reverse().findIndex((c) => c.kind === "step" && c.status === "confirm");
    if (ci >= 0) {
      i = aiCards.length - 1 - ci;
      const c = aiCards[i] as (typeof aiCards)[number] & { kind: "step" };
      aiCards = [...aiCards.slice(0, i), { ...c, status: approve ? "running" : "skipped" }, ...aiCards.slice(i + 1)];
    } else {
      const pi = [...aiCards].reverse().findIndex((c) => c.kind === "plan" && c.status === "plan");
      if (pi >= 0) {
        i = aiCards.length - 1 - pi;
        const c = aiCards[i] as (typeof aiCards)[number] & { kind: "plan" };
        aiCards = [...aiCards.slice(0, i), { ...c, status: approve ? "running" : "skipped" }, ...aiCards.slice(i + 1)];
      }
    }
    try {
      await api.aiControl(activeTab ?? "", approve ? "approve" : "reject");
    } catch {
      /* ignore */
    }
  }

  /// §8.7.3 整份计划修改：用编辑后的命令列表覆盖并执行
  async function editPlan(commands: string[]) {
    const pi = [...aiCards].reverse().findIndex((c) => c.kind === "plan" && c.status === "plan");
    if (pi >= 0) {
      const i = aiCards.length - 1 - pi;
      const c = aiCards[i] as (typeof aiCards)[number] & { kind: "plan" };
      aiCards = [...aiCards.slice(0, i), { ...c, commands: c.commands, status: "running" }, ...aiCards.slice(i + 1)];
    }
    try {
      await api.aiControl(activeTab ?? "", "edit", commands);
    } catch {
      /* ignore */
    }
  }

  async function stopAi() {
    try {
      await api.aiStop(activeTab ?? "");
    } catch {
      /* ignore */
    }
  }

  async function clearAi() {
    try {
      await api.aiClearHistory(activeTab ?? "");
    } catch {
      /* ignore */
    }
    aiLog = [];
    aiCards = [];
    aiSummary = null;
    aiTaskText = "";
    aiStreamOpen = false;
    aiThinking = "";
  }

  async function saveSettings(ai: AiConfig, ui: UiConfig) {
    try {
      if (ai.model) {
        await api.updateAiConfig(ai);
        aiConfig = ai;
        refreshAiMode(activeTab ?? "");
      }
      await api.updateUiConfig(ui);
      uiConfig = ui;
      sessionsPanelPct = ui.sessions_panel_pct;
      dockSessions = ui.dock_sessions;
      theme = ui.theme;
      showSettings = false;
      return null;
    } catch (e) {
      return String(e);
    }
  }
</script>

<svelte:head>
  <title>Helm</title>
</svelte:head>

<main class="app" style:--sp={`${sessionsPanelPct}%`}>
  <div class="main-row">
    {#if dockSessions}
      <SessionPanel
        {sessions}
        {statuses}
        {tabs}
        onSelect={selectSession}
        onConnect={connectSession}
        onDisconnect={disconnectSession}
        onRdp={rdpConnect}
        onNew={openNewSession}
        onEdit={openEditSession}
        onDelete={deleteSession}
      />
    {/if}

    <section class="main-col">
      <section class="terminal-pane">
        <TerminalTabs
          {tabs}
          {activeTab}
          theme={resolvedTheme}
          {kinds}
          onSelect={selectSession}
          onClose={onTabClose}
          onAdd={openNewSession}
          onCd={handleCd}
          onPwd={handlePwd}
          {aiMode}
          aiBusy={aiBusy[activeTab ?? ""] ?? false}
          {aiCards}
          {aiTaskText}
          {aiSummary}
          {aiStreamOpen}
          {aiFocusSeq}
          {aiLog}
          {logOpen}
          {aiEcho}
          {aiThinking}
          onModeChange={handleModeChange}
          onApprove={() => decide(true)}
          onReject={() => decide(false)}
          onEditPlan={editPlan}
          onStop={stopAi}
          onClear={clearAi}
          onOpenTask={focusAi}
          onToggleLog={toggleLog}
          onToggleStream={toggleStream}
          onDockSubmit={submitFromDock}
        />
      </section>

      <FileBrowser {activeTab} {cdReq} {kinds} />
    </section>
  </div>

  <StatusBar
    {sessions}
    {statuses}
    onOpenSettings={() => (showSettings = true)}
  />
</main>

{#if showSessionForm}
  <SessionForm
    {editingSession}
    onCancel={() => (showSessionForm = false)}
    onSubmit={submitSession}
  />
{/if}

{#if showSettings && uiConfig}
  <SettingsModal
    {aiConfig}
    {uiConfig}
    onCancel={() => (showSettings = false)}
    onSave={saveSettings}
  />
{/if}

<style>
  :global(:root) {
    --bg: #0f1116;
    --bg-panel: #171b22;
    --fg: #e8eaed;
    --fg-muted: #9aa3ad;
    --border: #262b33;
    --accent: #4c8dff;
    --accent-hover: #6ba1ff;
    --danger: #ef6b6b;
    --warning: #e6a23c;
    --ok: #3fb96b;
    --radius: 8px;
    --radius-sm: 6px;
    --radius-lg: 10px;
    --shadow: 0 2px 10px rgba(0, 0, 0, 0.4);
    --hover: #22272e;
    --active-bg: #1d2733;
    --input-bg: #1f242b;
    --modal-bg: #1b1f26;
    --track-bg: #20242b;
    --bubble-bg: #222830;
    --warn-bg: #2a2417;
    --term-bg: #0a0d11;
    --term-fg: #d6d9de;
    --tabbar-bg: #171b22;
    --tab-border: #262b33;
    --tab-active-bg: #0a0d11;
    --tabbar-fg: #9aa3ad;
    --statusbar-bg: #1a1d24;
    --statusbar-fg: #c8cdd4;
    --statusbar-border: #262b33;
    --accent-dim: rgba(76, 141, 255, 0.14);
  }

  :global(:root[data-theme="light"]) {
    --bg: #f6f8fa;
    --bg-panel: #ffffff;
    --fg: #1a1a2e;
    --fg-muted: #6b7280;
    --border: #e2e6ea;
    --accent: #2f6fed;
    --accent-hover: #1f5fd0;
    --danger: #d64545;
    --warning: #e6a23c;
    --ok: #2ea44f;
    --shadow: 0 2px 8px rgba(20, 30, 50, 0.08);
    --hover: #f0f3f7;
    --active-bg: #e3ecfb;
    --input-bg: #ffffff;
    --modal-bg: #ffffff;
    --track-bg: #eef2f7;
    --bubble-bg: #f0f3f7;
    --warn-bg: #fff8ec;
    --term-bg: #ffffff;
    --term-fg: #1a1a2e;
    --tabbar-bg: #f0f3f7;
    --tab-border: #e2e6ea;
    --tab-active-bg: #ffffff;
    --tabbar-fg: #6b7280;
    --statusbar-bg: #ffffff;
    --statusbar-fg: #6b7280;
    --statusbar-border: #e2e6ea;
    --accent-dim: rgba(47, 111, 237, 0.1);
  }

  /* 主题化细滚动条(WebView2 = Chromium,webkit 前缀生效) */
  :global(::-webkit-scrollbar) {
    width: 10px;
    height: 10px;
  }
  :global(::-webkit-scrollbar-track) {
    background: transparent;
  }
  :global(::-webkit-scrollbar-thumb) {
    background: var(--border);
    border-radius: 5px;
    border: 2px solid transparent;
    background-clip: padding-box;
  }
  :global(::-webkit-scrollbar-thumb:hover) {
    background: var(--fg-muted);
    border: 2px solid transparent;
    background-clip: padding-box;
  }
  :global(::-webkit-scrollbar-corner) {
    background: transparent;
  }

  :global(body) {
    margin: 0;
    font-family: "Segoe UI", system-ui, sans-serif;
    background: var(--bg);
    color: var(--fg);
    overflow: hidden;
    -webkit-font-smoothing: antialiased;
  }

  :global(*) {
    box-sizing: border-box;
  }

  .app {
    height: 100vh;
    display: flex;
    flex-direction: column;
  }

  .main-row {
    flex: 1;
    display: flex;
    flex-direction: row;
    min-height: 0;
    min-width: 0;
  }
  .main-row > :global(aside.panel) {
    flex: 0 0 auto;
  }
  .main-col {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--term-bg);
  }
  .terminal-pane {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--term-bg);
    flex: 1;
    /* 收拢终端内容，避免 AI 输入条/终端溢出画到下方文件面板上（Bug1） */
    overflow: hidden;
  }
</style>
