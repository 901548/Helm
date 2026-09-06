// P84 Agent 评测跑分器：CDP 驱动 Helm 真机执行任务集，采集成功率/步数/时长
//
// 用法：node scripts/bench/run.mjs [会话名] （默认 centos-132）
// 前置：vite + helm.exe 带 CDP 9229 已启动；目标会话已在配置中；Ollama 已运行
// 输出：控制台表格 + scripts/bench/results-<时间戳>.json
import fs from "node:fs";
import path from "node:path";

const SESSION = process.argv[2] || "centos-132";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const TASKS_DIR = path.dirname(new URL(import.meta.url).pathname.replace(/^\/(\w:)/, "$1"));
const tasks = JSON.parse(fs.readFileSync(path.join(TASKS_DIR, "tasks.json"), "utf8")).tasks;

// ---------- CDP 基础 ----------
const list = await fetch("http://127.0.0.1:9229/json").then((r) => r.json());
const page = list.find((p) => p.type === "page");
if (!page) { console.error("FAIL: 无 CDP 页面"); process.exit(1); }
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
let id = 0;
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
  new Promise((res, rej) => { const i = ++id; pending.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method, params })); });
const ev = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (r.exceptionDetails) throw new Error("eval failed: " + JSON.stringify(r.exceptionDetails).slice(0, 200));
  return r.result.value;
};
const typeCmd = async (s) => {
  await ev(`window.__TAURI_INTERNALS__.invoke('send_active_input', { data: Array.from(new TextEncoder().encode(${JSON.stringify(s)})) })`);
  await ev(`window.__TAURI_INTERNALS__.invoke('send_active_input', { data: [13] })`);
};

// 中止关键词：Done 消息含任一即视为失败
const ABORT_KEYWORDS = ["中止", "已停止", "超时", "已放弃", "提前声明", "未给出可执行命令", "未调用工具"];

const readState = async () =>
  ev(`(() => ({
    busy: document.querySelector('.ai-input')?.disabled === true,
    summary: document.querySelector('.ai-summary')?.textContent.trim() ?? "",
    cards: [...document.querySelectorAll('.ai-stream .card')].length,
  }))()`);

const results = [];
console.log(`== Agent 评测开始：会话 ${SESSION}，${tasks.length} 个任务 ==\n`);

for (const t of tasks) {
  // 每任务前清场：Ctrl+C + 回 home + 清 AI 历史
  await typeCmd("\u0003");
  await sleep(400);
  await typeCmd("cd /root");
  await sleep(600);
  try { await ev(`window.__TAURI_INTERNALS__.invoke('ai_clear_history', { name: ${JSON.stringify(SESSION)} })`); } catch {}
  await ev(`window.__TAURI_INTERNALS__.invoke('ai_set_mode', { name: ${JSON.stringify(SESSION)}, mode: 'agent' })`);
  await sleep(400);

  const t0 = Date.now();
  try {
    await ev(`window.__TAURI_INTERNALS__.invoke('ai_submit', { name: ${JSON.stringify(SESSION)}, input: ${JSON.stringify(t.task)}, pwd: '/root', container: null })`);
  } catch (e) {
    results.push({ id: t.id, category: t.category, status: "submit-error", durationSec: 0, steps: 0, summary: String(e).slice(0, 80) });
    continue;
  }

  // 轮询完成（单任务上限 8 分钟）
  let busy = true;
  let summary = "";
  for (let i = 0; i < 160 && busy; i++) {
    await sleep(3000);
    const st = await readState();
    busy = st.busy;
    summary = st.summary;
  }
  const durationSec = Math.round((Date.now() - t0) / 1000);

  // 步数：从记录器数本任务窗口内的 ai_step（按时间过滤由汇总阶段做，这里先读总数）
  const abortHit = ABORT_KEYWORDS.find((k) => summary.includes(k));
  const status = busy ? "timeout" : abortHit ? "fail" : summary.includes("任务完成") ? "success" : abortHit ? "fail" : "done?";
  results.push({ id: t.id, category: t.category, status, durationSec, summary: summary.slice(0, 100) });
  console.log(`${t.id.padEnd(16)} ${status.padEnd(8)} ${durationSec}s | ${summary.slice(0, 60)}`);
  await sleep(2500);
}

// 步数统计：从记录器 JSONL 按运行窗口切分（ai_task 之间的 ai_step）
const recDir = path.resolve(TASKS_DIR, "../../logs");
let stepCounts = {};
try {
  const files = fs.readdirSync(recDir).filter((f) => f.endsWith(".jsonl")).sort();
  const rec = [];
  for (const f of files) {
    for (const l of fs.readFileSync(path.join(recDir, f), "utf8").split("\n")) {
      if (!l.trim()) continue;
      try { rec.push(JSON.parse(l)); } catch {}
    }
  }
  // 找到每个 ai_task 之后、下一个 ai_task 之前的 ai_step 数
  let cur = null;
  for (const r of rec) {
    if (r.type === "ai_task") { cur = r.ts; stepCounts[cur] = 0; }
    else if (r.type === "ai_step" && cur) stepCounts[cur]++;
  }
  // 最近 N 个 ai_task 对应本次评测的任务（按顺序取末尾）
  const keys = Object.keys(stepCounts);
  const tail = keys.slice(-results.length);
  results.forEach((r, i) => { r.steps = stepCounts[tail[i]] ?? "?"; });
} catch (e) {
  console.log("(记录器步数统计失败:", String(e).slice(0, 60), ")");
}

// 汇总
const okCount = results.filter((r) => r.status === "success").length;
console.log("\n== 评测汇总 ==");
console.log(`成功 ${okCount}/${results.length}`);
for (const r of results) {
  console.log(`  ${r.id.padEnd(16)} ${String(r.status).padEnd(8)} steps=${r.steps} ${r.durationSec}s`);
}
const stamp = new Date().toISOString().replace(/[:T]/g, "-").slice(0, 16);
const outFile = path.join(TASKS_DIR, `results-${stamp}.json`);
fs.writeFileSync(outFile, JSON.stringify({ session: SESSION, model: "ollama/glm4:latest", fc: true, results }, null, 2));
console.log("结果已写入:", outFile);
ws.close();
process.exit(0);
