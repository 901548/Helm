<script lang="ts">
  import { onMount } from "svelte";
  import SessionPanel from "./components/sessions/SessionPanel.svelte";
  import SessionForm from "./components/sessions/SessionForm.svelte";
  import TerminalTabs from "./components/terminal/TerminalTabs.svelte";
  import FileBrowser from "./components/files/FileBrowser.svelte";
  import SettingsModal from "./components/chrome/SettingsModal.svelte";
  import HistoryModal from "./components/chrome/HistoryModal.svelte";
  import StatusBar from "./components/chrome/StatusBar.svelte";
  import * as api from "./lib/api";
  import type { AiCard, AiLogEntry, AiConfig, AiState, SessionInfo, SessionKind, SessionStatus, Theme, UiConfig } from "./lib/api";

  let sessions = $state<SessionInfo[]>([]);
  let statuses = $state<Record<string, SessionStatus>>({});
  let tabs = $state<string[]>([]);
  let activeTab = $state<string | null>(null);
  // 多会话：按会话名记录忙碌态，面板/提交只取当前活动会话的那一份
  let aiBusy = $state<Record<string, boolean>>({});
  // §8.7.1 唯一状态机前端侧：ai_job 广播的 State 事件按会话落盘，dock 只读活动会话那份
  let aiState = $state<Record<string, AiState>>({});
  let aiMode = $state<"qa" | "agent">("qa");
  let aiConfig = $state<AiConfig | null>(null);
  let uiConfig = $state<UiConfig | null>(null);
  // P39 AI 常驻命令条 + 活动流状态
  // P89：对话卡片按会话分桶（后端 AiSlot.conv 为权威副本，切换标签时拉取）
  let aiCards = $state<Record<string, AiCard[]>>({});
  let aiTaskText = $state<Record<string, string>>({});
  let aiSummary = $state<Record<string, { text: string; ok: boolean } | null>>({});
  let aiStreamOpen = $state(false);
  let aiFocusSeq = $state(0);
  let logOpen = $state(false);
  let aiLog = $state<AiLogEntry[]>([]);
  let aiEcho = $state<{ seq: number; name: string; text: string }[]>([]);
  // 推理型模型思考过程：实时累积展示，不混入最终答案卡片
  let aiThinking = $state<Record<string, string>>({});
  let pwds = $state<Record<string, string>>({});
  let echoSeq = 0;
  let logId = 0;
  let cardId = 0;
  // P93：conv 拉取的 last-write-wins 序列号——切标签/提交新任务都会作废在途的旧快照，
  // 防旧 aiConvRead 结果晚到覆盖掉刚落地的新卡片
  let convSeq = 0;

  let showSessionForm = $state(false);
  let editingSession = $state<SessionInfo | null>(null);
  let showSettings = $state(false);
  let showHistory = $state(false);

  let sessionsPanelPct = $state(22);
  let dockSessions = $state(true);
  let theme = $state<Theme>("dark");
  let systemDark = $state(false);
  let cdReq = $state<{ name: string; target: string; seq: number } | null>(null);
  let cdSeq = 0;

  const resolvedTheme = $derived<"light" | "dark">(
    theme === "system" ? (systemDark ? "dark" : "light") : theme,
  );

  // P88：终端回显开关（关 = 终端只留 shell 输出，AI 过程看卡片）
  const aiEchoOn = $derived(aiConfig?.ai_echo_terminal ?? false);

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
    api
      .listSessions()
      .then((list) => {
        sessions = list;
        refreshStatuses();
      })
      .catch((e) => console.error("加载会话列表失败:", e));
    refreshAiMode();
    refreshUiConfig();
    api.getAiConfig().then((c) => (aiConfig = c)).catch((e) => console.error("加载 AI 配置失败:", e));

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
          pushEcho(p.name, `\r\n\x1b[1;31m[连接失败]\x1b[0m ${p.error || "未知错误"}\r\n`, true);
        }
        if (p.status === "disconnected") {
          // 连接意外断开：保留标签以便重新连接；有其他已连接会话时才让出活动位，
          // 否则保留断线标签为当前视图（P72 重连横幅依赖 activeTab 有效）
          if (activeTab === p.name) {
            const next = pickNextActive(p.name);
            if (next) {
              activeTab = next;
              api.setActive(next);
            }
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
        case "state":
          // §8.7.1 显式状态迁移，dock 状态 chip 唯一判态来源
          aiState[p.name] = p.state;
          break;
        case "streaming": {
          // QA 流式追加进活动流卡片;Agent 模式的模型原始输出不展示(等解析后的命令卡片)
          if (!p.text || aiMode !== "qa" || !isActive) break;
          updateCards(p.name, (cards) => {
            const last = cards[cards.length - 1];
            if (last && last.kind === "qa" && !last.done) {
              return [...cards.slice(0, -1), { ...last, text: last.text + p.text }];
            }
            return [...cards, { id: ++cardId, kind: "qa", text: p.text, done: false }];
          });
          break;
        }
        case "reasoning": {
          // 推理型模型思考过程：仅活动会话时实时展示（QA 卡未完成时可见）
          if (!isActive) break;
          aiThinking = { ...aiThinking, [p.name]: ((aiThinking[p.name] ?? "") + (p.text || "")).slice(-4000) };
          break;
        }
        case "commandStep": {
          // 仅活动会话更新 AI 活动面板；命令仍镜像到其所属会话的终端（含后台并行）
          {
            updateCards(p.name, (cards) => {
              const idx = [...cards]
                .reverse()
                .findIndex((c) => c.kind === "step" && c.command === p.command && ["running", "confirm", "skipped"].includes(c.status));
              if (idx >= 0) {
                const i = cards.length - 1 - idx;
                const c = cards[i] as (typeof cards)[number] & { kind: "step" };
                return [
                  ...cards.slice(0, i),
                  { ...c, status: p.success ? "ok" : "fail", message: p.message, output: p.output || undefined },
                  ...cards.slice(i + 1),
                ];
              }
              return [...cards, { id: ++cardId, kind: "step", command: p.command, status: p.success ? "ok" : "fail", message: p.message, output: p.output || undefined }];
            });
            if (isActive) {
              aiStreamOpen = true;
            }
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
          updateCards(p.name, (cards) => [
            ...cards,
            { id: ++cardId, kind: "step", command: p.command, status: "confirm", level: p.level, reason: p.reason },
          ]);
          if (isActive) {
            aiStreamOpen = true;
          }
          break;
        case "planning":
          // §8.7.2 整份计划卡：一次展示该步全部命令，整份确认/编辑/放弃
          if (p.commands?.length) {
            updateCards(p.name, (cards) => [
              ...cards,
              { id: ++cardId, kind: "plan", commands: p.commands, status: "plan", needConfirm: p.needConfirm ?? true },
            ]);
            if (isActive) {
              aiStreamOpen = true;
            }
          }
          break;
        case "goalStarted":
          // P57 L2 子目标开始：镜像到所属会话终端（轻量提示，不新增卡片类型）
          if (p.name) {
            pushEcho(p.name, `\r\n\x1b[36m┌─ [AI] 子目标 ${p.goalIndex + 1}: ${p.title}\x1b[0m\r\n`);
          }
          break;
        case "goalDone":
          // P57 L2 子目标结局：成功/失败（失败将停留供重规划）
          if (p.name) {
            const ok = p.status === "ok";
            pushEcho(
              p.name,
              `\r\n${ok ? "\x1b[32m" : "\x1b[31m"}└─ [AI] 子目标 ${p.goalIndex + 1} ${ok ? "✓ 完成" : "✗ 失败（将重规划当前子目标）"}\x1b[0m\r\n`,
            );
          }
          break;
        case "done":
          if (isActive) {
            // P88-A：QA 模式下完整回答已在卡片内流式展示，摘要只放短句（消除问/答挤一行）
            const isQa = aiMode === "qa";
            aiSummary = { ...aiSummary, [p.name]: { text: isQa ? "已回答" : p.message, ok: true } };
            finishQaCard(p.name);
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
            aiSummary = { ...aiSummary, [p.name]: { text: p.message, ok: false } };
            finishQaCard(p.name);
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
      // 未打开终端标签时：后端已连接的直接开标签复用（P74：reload 后 statuses 已知真实状态），
      // 否则发起连接（Connected 事件回来再建标签）
      if (statuses[name] === "Connected") {
        tabs = [...tabs, name];
      } else {
        connectSession(name);
      }
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
        // P89/P92：AI 对话桶 + busy/state 跟随改名（改名时 AI 任务进行中则同步迁移，
        // 否则新名命令条看不到 busy/停止按钮，旧名成为孤儿键）
        if (aiCards[oldName] !== undefined) {
          aiCards[info.name] = aiCards[oldName];
          delete aiCards[oldName];
          aiTaskText[info.name] = aiTaskText[oldName];
          delete aiTaskText[oldName];
          aiSummary[info.name] = aiSummary[oldName];
          delete aiSummary[oldName];
          aiThinking[info.name] = aiThinking[oldName];
          delete aiThinking[oldName];
        }
        if (aiBusy[oldName] !== undefined) {
          aiBusy[info.name] = aiBusy[oldName];
          delete aiBusy[oldName];
        }
        if (aiState[oldName] !== undefined) {
          aiState[info.name] = aiState[oldName];
          delete aiState[oldName];
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

  /// 忘记会话主机的 TOFU 指纹（服务器重装后重置信任），结果经确认框反馈
  async function forgetHostKey(name: string) {
    const info = sessions.find((s) => s.name === name);
    if (!info || info.kind === "rdp") return;
    if (!window.confirm(`忘记【${name}】(${info.host}:${info.port ?? 22}) 已记录的主机密钥？\n服务器重装后密钥变更连不上时使用；下次连接将重新信任新密钥。`)) return;
    try {
      const removed = await api.forgetHostKey(info.host, info.port ?? 22);
      window.alert(removed ? "已忘记主机密钥，下次连接将重新记录。" : "该主机没有已记录的密钥。");
    } catch (e) {
      window.alert(`操作失败: ${String(e)}`);
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
    // P89：AI 对话桶随会话删除清理
    delete aiCards[name];
    delete aiTaskText[name];
    delete aiSummary[name];
    delete aiThinking[name];
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

  // P89：按会话更新卡片桶（事件路由以 p.name 为键，后台会话也积累）
  function updateCards(name: string, fn: (cards: AiCard[]) => AiCard[]) {
    aiCards = { ...aiCards, [name]: fn(aiCards[name] ?? []) };
  }

  // P89：切换标签时从后端拉取该会话的权威对话记录（后台会话的积累也在）
  function convToCard(e: Record<string, unknown>, id: number): AiCard {
    const kind = String(e.kind ?? "");
    if (kind === "qa") {
      return { id, kind: "qa", text: String(e.a ?? ""), done: true };
    }
    if (kind === "plan") {
      return {
        id,
        kind: "plan",
        commands: (e.commands as any) ?? [],
        status: "plan",
        needConfirm: Boolean(e.need_confirm),
      };
    }
    return {
      id,
      kind: "step",
      command: String(e.command ?? ""),
      status: e.success ? "ok" : "fail",
      message: String(e.message ?? ""),
      output: String(e.output ?? "") || undefined,
    };
  }
  $effect(() => {
    const n = activeTab;
    if (!n) return;
    const seq = ++convSeq;
    api
      .aiConvRead(n)
      .then((conv) => {
        if (seq !== convSeq || activeTab !== n) return; // 已切走或已有更新拉取，丢弃
        // Task 条目回填任务文本（非卡片）；Qa/Plan/Step 映射为卡片
        const taskEntry = conv.find((e) => String(e.kind ?? "") === "task");
        const taskText = taskEntry ? String((taskEntry as Record<string, unknown>).task ?? "") : "";
        const cards = conv
          .filter((e) => String(e.kind ?? "") !== "task")
          .map((e) => convToCard(e, ++cardId));
        aiTaskText = { ...aiTaskText, [n]: taskText };
        aiCards = { ...aiCards, [n]: cards };
      })
      .catch(() => {});
  });

  function pushEcho(name: string, text: string, force = false) {
    // P88：aiEchoOn 关闭时静默（连接失败等 force 调用除外）
    if (!force && !aiEchoOn) return;
    // 队列而非单槽:同一批次内多个事件都保留,靠递增 seq 由消费端按序写入,
    // 防止密集 streaming 事件只留最后一个导致丢字。超限裁剪旧条目(视为已消费)。
    aiEcho = [...aiEcho.slice(-199), { seq: ++echoSeq, name, text }];
  }

  function addLog(entry: AiLogEntry) {
    aiLog = [...aiLog.slice(-199), entry];
  }

  /// QA 流式卡片收尾(done/error 时)
  function finishQaCard(name: string) {
    updateCards(name, (cards) => {
      const last = cards[cards.length - 1];
      if (last && last.kind === "qa" && !last.done) {
        return [...cards.slice(0, -1), { ...last, done: true }];
      }
      return cards;
    });
  }

  /// AI 命令条提交:重置活动流 + 调后端;Agent 任务在终端留一行锚点
  /// `container`：Docker 会话运行时目标容器（§8.7.4）
  async function submitFromDock(text: string, container?: string | null, termContext?: string | null) {
    if (!text.trim() || aiBusy[activeTab ?? ""]) return;
    const name = activeTab ?? "";
    // 提交新任务：作废在途的旧 conv 拉取，避免其晚到覆盖刚建的任务头/卡片
    convSeq++;
    updateCards(name, () => (aiMode === "qa" ? [{ id: ++cardId, kind: "qa", text: "", done: false }] : []));
    aiTaskText = { ...aiTaskText, [name]: text };
    aiSummary = { ...aiSummary, [name]: null };
    aiStreamOpen = true;
    aiThinking = { ...aiThinking, [name]: "" };
    if (aiMode === "agent" && name) {
      pushEcho(name, `\r\n\x1b[90m[AI] 任务: ${text}\x1b[0m\r\n`);
    }
    const pwd = pwds[name] ?? "";
    try {
      await api.aiSubmit(name, text, pwd || undefined, container || null, termContext || null);
    } catch (e) {
      aiSummary = { ...aiSummary, [name]: { text: String(e), ok: false } };
      finishQaCard(name);
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
    const n = activeTab ?? "";
    if ((aiCards[n]?.length ?? 0) || aiSummary[n] || aiTaskText[n]) aiStreamOpen = !aiStreamOpen;
  }

  async function decide(approve: boolean) {
    // 乐观更新确认卡片;拒绝时后端会回发 CommandStep(已跳过)统一收口
    // 优先找逐条确认卡(step/confirm),否则找整份计划卡(plan/plan)
    let i = -1;
    const cards = aiCards[activeTab ?? ""] ?? [];
    const ci = [...cards].reverse().findIndex((c) => c.kind === "step" && c.status === "confirm");
    if (ci >= 0) {
      i = cards.length - 1 - ci;
      const c = cards[i] as (typeof cards)[number] & { kind: "step" };
      updateCards(activeTab ?? "", (cs) => [...cs.slice(0, i), { ...c, status: approve ? "running" : "skipped" }, ...cs.slice(i + 1)]);
    } else {
      const pi = [...cards].reverse().findIndex((c) => c.kind === "plan" && c.status === "plan");
      if (pi >= 0) {
        i = cards.length - 1 - pi;
        const c = cards[i] as (typeof cards)[number] & { kind: "plan" };
        updateCards(activeTab ?? "", (cs) => [...cs.slice(0, i), { ...c, status: approve ? "running" : "skipped" }, ...cs.slice(i + 1)]);
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
    const cards = aiCards[activeTab ?? ""] ?? [];
    const pi = [...cards].reverse().findIndex((c) => c.kind === "plan" && c.status === "plan");
    if (pi >= 0) {
      const i = cards.length - 1 - pi;
      const c = cards[i] as (typeof cards)[number] & { kind: "plan" };
      updateCards(activeTab ?? "", (cs) => [...cs.slice(0, i), { ...c, status: "running" }, ...cs.slice(i + 1)]);
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
    updateCards(activeTab ?? "", () => []);
    try {
      await api.aiConvClear(activeTab ?? "");
    } catch {
      /* ignore */
    }
    aiSummary = { ...aiSummary, [activeTab ?? ""]: null };
    aiTaskText = { ...aiTaskText, [activeTab ?? ""]: "" };
    aiStreamOpen = false;
    aiThinking = { ...aiThinking, [activeTab ?? ""]: "" };
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
        onForgetKey={forgetHostKey}
      />
    {/if}

    <section class="main-col">
      <section class="terminal-pane">
        <TerminalTabs
          {tabs}
          {activeTab}
          theme={resolvedTheme}
          {kinds}
          status={statuses[activeTab ?? ""] ?? "Disconnected"}
          onReconnect={() => { if (activeTab) connectSession(activeTab); }}
          onSelect={selectSession}
          onClose={onTabClose}
          onAdd={openNewSession}
          onCd={handleCd}
          onPwd={handlePwd}
          {aiMode}
          aiBusy={aiBusy[activeTab ?? ""] ?? false}
          aiState={aiState[activeTab ?? ""] ?? "idle"}
          cards={aiCards[activeTab ?? ""] ?? []}
          taskText={aiTaskText[activeTab ?? ""] ?? ""}
          summary={aiSummary[activeTab ?? ""] ?? null}
          aiStreamOpen={aiStreamOpen}
          aiFocusSeq={aiFocusSeq}
          aiLog={aiLog}
          logOpen={logOpen}
          aiEcho={aiEcho}
          thinking={aiThinking[activeTab ?? ""] ?? ""}
          termFontSize={uiConfig?.term_font_size ?? 14}
          termScrollback={uiConfig?.term_scrollback ?? 5000}
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
          onOpenHistory={() => (showHistory = true)}
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
    {#if showHistory}
      <HistoryModal onClose={() => (showHistory = false)} />
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
    --panel-glow: rgba(128, 128, 160, 0.18);
    --badge-win: #4aa3ff;
    --badge-rdp: #c792ea;
    --badge-docker: #3db2ff;
    --dot-off: #c4c9d0;
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
    --panel-glow: rgba(80, 90, 120, 0.12);
    --badge-win: #1f6fd6;
    --badge-rdp: #8250df;
    --badge-docker: #0b7bd6;
    --dot-off: #9aa3ad;
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
    font-family: "Segoe UI", "Microsoft YaHei UI", system-ui, sans-serif;
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
