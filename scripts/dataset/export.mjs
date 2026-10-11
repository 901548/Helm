// P70→训练闭环第一步：把操作记录器的 JSONL 轨迹导出为可训练的 SFT 数据集。
//
// 输入：logs/terminal-*.jsonl（P70 记录器，append-only 按天滚动）
// 输出：dataset/sft-samples.jsonl（OpenAI messages 单轮样本）+ dataset/stats.json + 控制台审计
//
// 样本构造（与部署形态一致）：
//   system   = 内置 Agent 提示词（运行时从 src-tauri/src/agent.rs 提取，防两处漂移）
//   user     = [任务] + [目录] +（非首步时）[上一步命令的输出]——还原 agent_step 的 prev_output 上下文
//   assistant= 该步命令（单行，exit_code==0 才收）
//
// 质量规则（v1，刻意保守）：
//   - 只收 exit_code==0 的步骤：失败步的单轮样本会教模型输出无效命令；
//     "失败→调整→成功"的恢复轨迹是多轮形态，留作 v2（见 README 路线）
//   - 剔除协议标记与空命令（DONE/GOAL_*/REFLEXION/EXPECT 开头等——不是 shell 命令）
//   - (task, command) 去重：同一任务里模型反复重试同一条成功命令只留首条
//   - qa 不导出：历史回答被旧运维提示词污染（"DONE"/"echo 2" 是负样本），见审计
//   - input 记录不导出：人类键入缺少任务上下文
//
// 用法：node scripts/dataset/export.mjs [--out dataset]
import fs from "node:fs";
import path from "node:path";

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname.replace(/^\/(\w:)/, "$1")), "../..");
const LOGS = path.join(ROOT, "logs");
const OUT = path.join(ROOT, process.argv.includes("--out") ? process.argv[process.argv.indexOf("--out") + 1] : "dataset");
const PREV_OUTPUT_CAP = 400; // user 上下文里上一步输出的截断长度（agent 实际回喂更大，训练样本取短防稀释）
const COMMAND_CAP = 2000; // 防御：异常长的"命令"不可能是正经单行命令

// ---------- 内置 Agent 提示词：从 agent.rs 提取（与部署一致，防两处漂移） ----------
function extractAgentPrompt() {
  const src = fs.readFileSync(path.join(ROOT, "src-tauri/src/agent.rs"), "utf8");
  const m = src.match(/pub fn default_system_prompt_agent\(\) -> String \{\s*concat!\(([\s\S]*?)\)\s*\.to_string\(\)/);
  if (!m) throw new Error("无法从 agent.rs 提取 default_system_prompt_agent——函数签名变了，请同步本脚本");
  const parts = [...m[1].matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((x) =>
    x[1].replace(/\\n/g, "\n").replace(/\\"/g, '"').replace(/\\\\/g, "\\"),
  );
  if (parts.length === 0) throw new Error("agent.rs 提示词提取为空");
  return parts.join("");
}

// ---------- 读取记录 ----------
function loadRecords() {
  const files = fs.existsSync(LOGS)
    ? fs.readdirSync(LOGS).filter((f) => f.startsWith("terminal-") && f.endsWith(".jsonl")).sort()
    : [];
  const records = [];
  for (const f of files) {
    const day = f.replace("terminal-", "").replace(".jsonl", "");
    const lines = fs.readFileSync(path.join(LOGS, f), "utf8").split("\n");
    for (const l of lines) {
      const s = l.trim();
      if (!s) continue;
      try {
        const r = JSON.parse(s);
        r._day = day;
        records.push(r);
      } catch {
        stats.parseErrors++;
      }
    }
  }
  return records;
}

// ---------- 协议标记/无效命令判定 ----------
const PROTOCOL_RE = /^(DONE|GOAL(\b|[_:\s])|GOAL_OK|GOAL_FAIL|GOAL_DONE|REFLEXION|EXPECT|finish\b|goal_ok|goal_done|goal_fail|reflect\b|set_goal\b|run_command\b)/i;
function isUsableCommand(cmd) {
  const c = (cmd ?? "").trim();
  if (!c || c.length > COMMAND_CAP) return false;
  if (PROTOCOL_RE.test(c)) return false; // 协议行/工具名泄漏残留，不是 shell 命令
  if (/[\r\n]/.test(c.trim())) return false; // 多行输入不是单命令样本（粘贴块等）
  return true;
}

// ---------- 主流程 ----------
const stats = {
  files: 0, parseErrors: 0,
  records: 0, byType: {},
  tasks: 0, steps: 0,
  samples: 0, bySource: { skill: 0, llm: 0 },
  dropped: { exitNonZero: 0, protocolOrInvalid: 0, duplicate: 0, orphanStep: 0 },
  qaSkipped: 0, inputSkipped: 0,
  tasksWithSamples: 0, tasksZeroSuccess: 0,
};

fs.mkdirSync(OUT, { recursive: true });
const SYSTEM = extractAgentPrompt();
const records = loadRecords();
stats.records = records.length;

// 按 ai_task 分窗：窗 = 一条 ai_task 起到下一条 ai_task 前的所有 ai_step
const outPath = path.join(OUT, "sft-samples.jsonl");
const out = fs.createWriteStream(outPath);
let prevTask = null;
let windowSteps = []; // 当前任务窗内的步骤（含失败，供 prev_output 上下文）
let windowTask = null;
const seen = new Set(); // (task, command) 去重
let samplesInTask = 0;
const perTask = [];

const flushTask = () => {
  if (prevTask !== null) {
    perTask.push({ task: prevTask.task?.slice(0, 40), steps: windowSteps.length, samples: samplesInTask });
    if (samplesInTask > 0) stats.tasksWithSamples++;
    else if (windowSteps.length > 0) stats.tasksZeroSuccess++;
  }
  samplesInTask = 0;
};

for (const r of records) {
  stats.byType[r.type] = (stats.byType[r.type] ?? 0) + 1;
  if (r.type === "input") { stats.inputSkipped++; continue; }
  if (r.type === "qa") { stats.qaSkipped++; continue; }
  if (r.type === "ai_task") {
    flushTask();
    stats.tasks++;
    prevTask = r;
    windowTask = r;
    windowSteps = [];
    continue;
  }
  if (r.type !== "ai_step") continue;
  stats.steps++;
  const step = r;
  windowSteps.push(step);

  const ok = step.exit_code === 0;
  if (!ok) { stats.dropped.exitNonZero++; continue; }
  if (!windowTask) { stats.dropped.orphanStep++; continue; } // 无任务头的孤立步骤（跨天切窗等）
  if (!isUsableCommand(step.command)) { stats.dropped.protocolOrInvalid++; continue; }
  const key = `${windowTask.task}\u0000${step.command.trim()}`;
  if (seen.has(key)) { stats.dropped.duplicate++; continue; }
  seen.add(key);

  // user 上下文：任务 + 目录 + 上一步输出（还原 agent_step 的 prev_output 形态）
  const prev = windowSteps[windowSteps.length - 2]; // 刚 push 的当前步的前一步
  let user = `[任务] ${windowTask.task}\n[目录] ${step.pwd ?? windowTask.pwd ?? "/"}`;
  if (prev) {
    const prevOut = String(prev.output ?? "").trim();
    if (prevOut) user += `\n[上一步命令] ${prev.command}\n[上一步输出]\n${prevOut.slice(0, PREV_OUTPUT_CAP)}`;
  }

  const sample = {
    messages: [
      { role: "system", content: SYSTEM },
      { role: "user", content: user },
      { role: "assistant", content: step.command.trim() },
    ],
    meta: {
      ts: step.ts, session: step.session, day: step._day,
      source: step.source === "skill" ? "skill" : "llm",
      step: step.step ?? null, exit_code: step.exit_code,
    },
  };
  out.write(JSON.stringify(sample) + "\n");
  stats.samples++;
  stats.bySource[sample.meta.source]++;
  samplesInTask++;
}
flushTask();
out.end();

// stats.json
const audit = {
  generatedAt: new Date().toISOString(),
  ...stats,
  perTask,
  notes: [
    "qa 记录未导出：历史回答由旧运维提示词产生（DONE / echo 2 等命令形态），属负样本",
    "input 记录未导出：人类键入缺少任务上下文",
    "失败步未导出为单轮样本：恢复型多轮轨迹留作 v2（见 scripts/dataset/README.md）",
    "样本 system 统一使用内置 Agent 提示词（运行时从 agent.rs 提取）——部署训练后的模型建议清空 config.system_prompt_agent 保持一致",
  ],
};
fs.writeFileSync(path.join(OUT, "stats.json"), JSON.stringify(audit, null, 2));

// ---------- 控制台审计 ----------
console.log("== P70 轨迹 → SFT 数据集审计 ==");
console.log(`记录 ${stats.records} 条（input ${stats.inputSkipped} / qa ${stats.qaSkipped} / ai_task ${stats.tasks} / ai_step ${stats.steps}）`);
console.log(`样本 ${stats.samples} 条（skill 金标 ${stats.bySource.skill} + LLM 成功步 ${stats.bySource.llm}）`);
console.log(`过滤：失败步 ${stats.dropped.exitNonZero} / 协议或非法 ${stats.dropped.protocolOrInvalid} / 重复 ${stats.dropped.duplicate} / 孤立步 ${stats.dropped.orphanStep}`);
console.log(`任务 ${stats.tasks} 个：${stats.tasksWithSamples} 个产出样本，${stats.tasksZeroSuccess} 个零成功步`);
console.log(`量级评估：${stats.samples < 300 ? "偏薄（<300 条）——够 LoRA 冒烟与管线验证，稳定提升需继续用 App 积累数据" : "可用于小规模 LoRA 训练"}`);
console.log(`输出：${outPath}\n      ${path.join(OUT, "stats.json")}`);
