# 训练数据集（P70 → SFT 导出）

把操作记录器（`logs/terminal-*.jsonl`，P70）攒下的轨迹转成可直接训练的 SFT 样本。
**本仓库只提供导出器与格式定义，不在本机训练**（用户决定）；训练在具备 GPU 的机器上进行。

## 用法

```bash
node scripts/dataset/export.mjs        # 输出 dataset/sft-samples.jsonl + dataset/stats.json
```

`dataset/` 已 gitignore（内容由 logs 派生，含内网主机信息）。

## 样本格式（OpenAI messages 单轮）

```json
{
  "messages": [
    { "role": "system", "content": "<内置 Agent 提示词，导出时自动从 src-tauri/src/agent_prompts.rs 提取>" },
    { "role": "user", "content": "[任务] 查看磁盘使用情况…\n[目录] /root\n[上一步命令] df -h\n[上一步输出]\n…" },
    { "role": "assistant", "content": "df -h | sort -k 5 -nr | head -n 6" }
  ],
  "meta": { "ts": "…", "session": "…", "day": "20260906", "source": "skill", "step": 1, "exit_code": 0 }
}
```

设计原则：**与部署形态一致**——Agent 循环每轮给模型的是「任务 + 目录 + 上一步输出」，要求回一条单行命令。
`[上一步输出]` 截断 400 字（真实回喂更大，训练样本取短防稀释）。

## 质量规则（v1，刻意保守）

| 规则 | 理由 |
|---|---|
| 只收 `exit_code==0` 的步骤 | 失败步的单轮样本会教模型输出无效命令 |
| 剔除协议标记/空命令/超长 | `DONE`、`GOAL_*` 等不是 shell 命令 |
| `(task, command)` 全局去重 | 完全相同的样本对 SFT 无增益（bench 跨天重跑会产生大量重复） |
| qa 记录不导 | 历史回答由旧运维提示词产生（`DONE`/`echo 2`），是负样本 |
| input 记录不导 | 人类键入缺少任务上下文 |

失败→调整→成功的**恢复型多轮轨迹**是 v2 方向（对 Agent 更有价值但构造更复杂）。

## 量级现状（2026-10-11）

86 条样本（skill 金标 15 + LLM 成功步 71），70 个任务中 30 个产出样本。
**够 LoRA 冒烟与管线验证，不够稳定提升**——量级上到 300+ 的途径就是继续用 App 跑真实任务。

## 训练机上的推荐路线（不在本机执行）

1. **QLoRA 小模型冒烟**：Qwen2.5-Coder-1.5B/3B-Instruct + LLaMA-Factory 或 axolotl，
   LoRA rank 8–16、3 epoch 起，几小时内可跑完；样本量 <300 时重点看是否过拟合。
2. **合并导出 GGUF**（Q4_K_M 足够）。
3. **导入 Ollama**：`ollama create helm-agent -f Modelfile`（Modelfile 里 system 用与训练一致的内置 Agent 提示词，
   部署时清空 `config.system_prompt_agent` 保持一致）。
4. **评测对比**：`node scripts/bench/run.mjs`（v2，事件流判定）对同一任务集跑分，
   与 glm4 基线（v2 口径 7/8，见 `AGENTS.md` §5 P106）对比成功率/步数/时长。
5. 数据继续积累后重导出、重训、重评——这就是闭环。

## 与仓库其他部分的关系

- 记录器：`src-tauri/src/recorder.rs`（P70，字段定义与过滤规则的源头）
- 评测：`scripts/bench/run.mjs`（v2，事件流判定 + 自动批准）
- 提示词单一来源：`src-tauri/src/agent_prompts.rs::default_system_prompt_agent`（P134-C 自 agent.rs 拆出；导出器运行时自动提取，改提示词无需同步本脚本）
