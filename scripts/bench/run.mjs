// P84 跑分器 v2（P106 重写）：事件流判定 + 危险确认自动批准 + 可选文件校验
//
// v1 的两个缺陷（实测）：
//   1. 判定读 DOM 摘要——P88 已知"绕过 UI 提交"偏差会把 Agent 完成误标成「已回答」，
//      技能完成的摘要也对不上 success 关键词，结果只能人工从 JSONL 判读；
//   2. 无人值守跑分时删除类任务卡危险确认 120s 超时（T8 恒 fail）。
//
// v2 行为：
//   - 挂原生 'ai' 事件监听，按事件流判定：done(无中止关键词) + 校验/命令证据 → success
//   - planning(needConfirm) 与 pendingCommand 自动批准（批准计划=整份授权，含危险命令）；
//     防后端 flush_stale 吞掉竞态批准：8s 无进展自动补批（上限 12 次）
//   - tasks.json 可带 verify（file-exists / file-absent），用 SFTP fs_read_file 后置校验
//   - 零命令却报「任务完成」判 no-op（弱模型散文+DONE 不算成功，P104-3 实测）
//
// 用法：node scripts/bench/run.mjs [会话名]（缺省取 config.yaml 里第一个 linux 会话）
// 前置：vite + helm.exe 带 CDP 9229 已启动；目标会话已在 UI 里连接（绿点）且为活动标签
// 输出：控制台表格 + scripts/bench/results-<时间戳>.json
import fs from "node:fs";
import path from "node:path";

const TASKS_DIR = path.dirname(new URL(import.meta.url).pathname.replace(/^\/(\w:)/, "$1"));
const tasks = JSON.parse(fs.readFileSync(path.join(TASKS_DIR, "tasks.json"), "utf8")).tasks;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const SESSION = process.argv[2] || "";
const TASK_TIMEOUT_MS = 8 * 60 * 1000;

// ---------- CDP 基础 ----------
const list = await fetch("http://127.0.0.1:9229/json").then((r) => r.json());
const page = list.find((p) => p.type === "page" && p.webSocketDebuggerUrl);
if (!page) { console.error("FAIL: 无 CDP 页面"); process.exit(1); }
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
let msgId = 0;
const pending = new Map();
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) {
    const p = pending.get(m.id);
    pending.delete(m.id);
    m.error ? p.rej(new Error(JSON.stringify(m.error))) : p.res(m.result);
  }
};
const send = (method, params = {}) =>
  new Promise((res, rej) => { const i = ++msgId; pending.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method, params })); });
const ev = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (r.exceptionDetails) throw new Error("eval failed: " + JSON.stringify(r.exceptionDetails).slice(0, 200));
  return r.result.value;
};
const invoke = (cmd, args) => ev(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args ?? {})})`);
const typeRaw = async (s) => {
  // 纯跑分器动作（清场命令）不进训练日志：record:false（与 P100-6 面板同步同理由）
  await invoke("send_input", { name: SESSION, data: Array.from(new TextEncoder().encode(s)), record: false });
  await invoke("send_input", { name: SESSION, data: [13], record: false });
};

// ---------- 事件流 ----------
await send("Runtime.enable");
await ev(`(() => { window.__benchEvents = []; return true; })()`);
const cbId = await ev(`window.__TAURI_INTERNALS__.transformCallback((e) => { if (e?.payload) window.__benchEvents.push(e.payload); }, false)`);
await invoke("plugin:event|listen", { event: "ai", target: { kind: "Any" }, handler: cbId });
const readEvents = async () => ev(`window.__benchEvents`) || [];

// ---------- 会话解析 ----------
let session = SESSION;
if (!session) {
  const sessions = await invoke("list_sessions");
  const linux = (sessions ?? []).find((s) => (s.kind ?? "linux") === "linux");
  session = linux?.name ?? sessions?.[0]?.name;
}
if (!session) { console.error("FAIL: 未找到可用会话（config.yaml 里没有会话，或请传会话名参数）"); process.exit(1); }

// 模型名只用于报告：从 config.yaml 抠（无 yaml 依赖）
const cfgPath = path.resolve(TASKS_DIR, "../../config.yaml");
const cfgText = fs.existsSync(cfgPath) ? fs.readFileSync(cfgPath, "utf8") : "";
const model = (cfgText.match(/^(\s*)model:\s*(\S+)$/m) ?? [])[2] ?? "?";

console.log(`== Agent 评测 v2：会话 ${session}，${tasks.length} 个任务，model=${model} ==\n`);

// ---------- 校验器 ----------
async function runVerify(v) {
  if (!v) return { ok: true, note: "" };
  try {
    if (v.type === "file-exists") {
      const r = await invoke("fs_read_file", { name: session, path: v.path, limit: 64 });
      return { ok: r?.ok === true, note: r?.ok ? `存在: ${v.path}` : `不存在: ${v.path} (${String(r?.message).slice(0, 60)})` };
    }
    if (v.type === "file-absent") {
      const r = await invoke("fs_read_file", { name: session, path: v.path, limit: 64 });
      const absent = r?.ok !== true;
      return { ok: absent, note: absent ? `已删除: ${v.path}` : `仍存在: ${v.path}` };
    }
    return { ok: false, note: `未知校验类型: ${v.type}` };
  } catch (e) {
    return { ok: false, note: String(e).slice(0, 80) };
  }
}

// ---------- 主循环 ----------
const results = [];
for (const t of tasks) {
  // 每任务前清场：Ctrl+C + 回 home + 清 AI 历史 + 切 Agent 模式
  try {
    await typeRaw("\u0003");
    await sleep(400);
    await typeRaw("cd /root");
    await sleep(600);
    await invoke("ai_clear_history", { name: session });
    await invoke("ai_set_mode", { name: session, mode: "agent" });
  } catch (e) {
    results.push({ id: t.id, category: t.category, status: "submit-error", durationSec: 0, steps: 0, failedSteps: 0, summary: String(e).slice(0, 100) });
    console.log(`${t.id.padEnd(16)} submit-error | ${String(e).slice(0, 60)}`);
    continue;
  }

  // 事件游标：每轮 poll 只消费新事件（旧实现固定 slice(ev0)，每轮重读全部窗口 → 步数虚高）
  let evCursor = (await readEvents()).length;
  const t0 = Date.now();
  try {
    await invoke("ai_submit", { name: session, input: t.task, pwd: "/root", container: null, termContext: null });
  } catch (e) {
    results.push({ id: t.id, category: t.category, status: "submit-error", durationSec: 0, steps: 0, failedSteps: 0, summary: String(e).slice(0, 100) });
    console.log(`${t.id.padEnd(16)} submit-error | ${String(e).slice(0, 60)}`);
    continue;
  }

  // 事件流轮询：自动批准 + 等待 done/error
  const approved = new Set();
  let lastConfirmAt = 0;
  let haveConfirm = false;
  let outcome = null; // { kind: "done"|"error", message }
  let steps = 0;
  let failedSteps = 0;
  while (Date.now() - t0 < TASK_TIMEOUT_MS) {
    await sleep(500);
    const all = await readEvents();
    const win = all.slice(evCursor);
    evCursor = all.length;
    for (const e of win) {
      if (e.name !== session) continue;
      if (e.kind === "commandStep") {
        steps++;
        if (e.success === false) failedSteps++;
      } else if (e.kind === "planning" && e.needConfirm) {
        const key = "plan:" + (e.commands ?? []).map((c) => c.command).join("&&");
        if (!approved.has(key)) {
          approved.add(key);
          haveConfirm = true;
          lastConfirmAt = Date.now();
          invoke("ai_control", { name: session, action: "approve" }).catch(() => {});
        }
      } else if (e.kind === "pendingCommand") {
        const key = "cmd:" + e.command;
        if (!approved.has(key)) {
          approved.add(key);
          haveConfirm = true;
          lastConfirmAt = Date.now();
          invoke("ai_control", { name: session, action: "approve" }).catch(() => {});
        }
      } else if (e.kind === "done" || e.kind === "error") {
        outcome = { kind: e.kind, message: String(e.message ?? "") };
      }
    }
    // 看门狗：确认事件已发出但 8s 仍无命令落地 → 可能被后端 flush_stale 吞掉，补批
    if (haveConfirm && Date.now() - lastConfirmAt > 8000 && approved.size <= 12) {
      lastConfirmAt = Date.now();
      invoke("ai_control", { name: session, action: "approve" }).catch(() => {});
    }
    if (outcome) break;
  }

  const durationSec = Math.round((Date.now() - t0) / 1000);
  const ABORT = ["中止", "已停止", "超时", "已放弃", "提前声明", "未给出可执行命令", "未调用工具"];
  let status;
  let verifyNote = "";
  if (!outcome) {
    status = "timeout";
  } else if (outcome.kind === "error") {
    status = "fail";
  } else if (ABORT.some((k) => outcome.message.includes(k))) {
    status = "fail";
  } else if (steps === 0) {
    status = "no-op"; // 「任务完成」但一条命令没跑（散文+DONE），不算成功
  } else {
    const v = await runVerify(t.verify);
    verifyNote = v.note;
    status = v.ok ? "success" : "verify-fail";
  }
  results.push({
    id: t.id, category: t.category, status, durationSec, steps, failedSteps,
    verify: verifyNote, summary: outcome ? outcome.message.slice(0, 120) : "(超时未结束)",
  });
  console.log(`${t.id.padEnd(16)} ${status.padEnd(12)} ${String(durationSec).padStart(4)}s steps=${String(steps).padStart(3)} fail=${failedSteps}${verifyNote ? " | " + verifyNote : ""}`);
  await sleep(2500);
}

// ---------- 汇总 ----------
const okCount = results.filter((r) => r.status === "success").length;
console.log("\n== 评测汇总 ==");
console.log(`成功 ${okCount}/${results.length}`);
for (const r of results) {
  console.log(`  ${r.id.padEnd(16)} ${r.status.padEnd(12)} steps=${String(r.steps).padStart(3)} fail=${r.failedSteps} ${r.durationSec}s`);
}
const stamp = new Date().toISOString().replace(/[:T]/g, "-").slice(0, 16);
const outFile = path.join(TASKS_DIR, `results-${stamp}.json`);
fs.writeFileSync(outFile, JSON.stringify({ session, model, runner: "v2", results }, null, 2));
console.log("结果已写入:", outFile);
process.exit(0);
