// ============================================================================
// ai_job.rs - AI 任务编排域（自 core.rs 拆出,P42）
//   - run_ai_job 状态机:QA 流式 / Agent 逐步执行(危险命令确认 + cwd 权威跟踪)
//   - task_exec:在锁定会话上执行单条 Agent 命令并回传事件
//   - TaskCtx:任务上下文(锁定会话 + 初始权威 PWD)
// core.rs 只留命令薄封装;新增任务模式/执行策略改本文件。
// ============================================================================

use std::time::Duration;
use std::sync::Arc;

use tauri::{AppHandle, Emitter};

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::Mutex;

use crate::agent::{
    extract_goals, extract_reflexion, goal_marker, parse_commands, truncate_text, Agent,
    AgentMode, AiStreamEvent, FcAction, FcCall, FcTurn,
};
use crate::core::{AiControl, AiPayload, AiRunState, PlanCommand};
use crate::safety::{check_danger, DangerLevel};
use crate::ssh::SshManager;
use crate::task_exec::run_task_exec;

/// P57 L2 · 子目标状态（内存级，仅存于当前 job）
#[derive(Clone, Copy, PartialEq, Debug)]
enum GoalStatus {
    Pending,
    Active,
    Ok,
    Failed,
}

/// P57 L3 · 确认等待超时(秒)：用户长时间不点 批准/拒绝/放弃 时任务自动中止。
/// 不会"静默继续执行"未获批准的危险命令，避免确认点永久阻塞导致 busy 卡死。
const CONFIRM_TIMEOUT_SECS: u64 = 120;

/// 仅等待"确认"途径：超过 CONFIRM_TIMEOUT_SECS 未收到控制消息则返回 true(应中止任务)。
/// 区分通道关闭(Ok(None)/Cancel)与真正超时(Err)：调用方按各自语义处理。
async fn recv_confirm(ctl: &mut UnboundedReceiver<AiControl>) -> Result<Option<AiControl>, ()> {
    match tokio::time::timeout(Duration::from_secs(CONFIRM_TIMEOUT_SECS), ctl.recv()).await {
        Ok(v) => Ok(v),
        Err(_elapsed) => Err(()), // 确认超时
    }
}

/// 首个未完成的子目标（首个非 Ok）；用于决定当前执行/重规划目标。
fn next_active_goal(states: &[GoalStatus]) -> Option<usize> {
    states.iter().position(|s| !matches!(s, GoalStatus::Ok))
}

/// 对子目标 idx 应用一次执行结局（确定性：闭包判ok）。
/// - ok → 置 Ok，正常推进。
/// - 失败 → 累加 fail_retries；未达上限置 Failed（停留供 REFLEXION 重规划）；
///   超过上限则**放弃该子目标**（置 Ok 跳过）继续后续，避免一个子目标反复失败烧穿 max_steps
///     （模型漏发完成信号/无意义重复重试时的收敛兜底）。
/// 返回 (最终是否视为"完成可推进", 是否由重试上限放弃的)。
fn apply_goal_outcome(
    idx: usize,
    ok: bool,
    states: &mut [GoalStatus],
    fail_retries: &mut [u32],
    max_fail_retries: u32,
) -> (bool, bool) {
    if ok {
        states[idx] = GoalStatus::Ok;
        return (true, false);
    }
    fail_retries[idx] += 1;
    if fail_retries[idx] > max_fail_retries {
        states[idx] = GoalStatus::Ok;
        (false, true)
    } else {
        states[idx] = GoalStatus::Failed;
        (false, false)
    }
}

/// 把子目标列表渲染成喂给模型的目标进度（✔ 完成 / ✘ 失败 / … 进行中）
fn build_goal_summary(goals: &[String], states: &[GoalStatus]) -> String {
    if goals.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (i, g) in goals.iter().enumerate() {
        let sym = match states[i] {
            GoalStatus::Ok => "✔",
            GoalStatus::Failed => "✘",
            GoalStatus::Active | GoalStatus::Pending => "…",
        };
        out.push_str(&format!("  {} {}. {}\n", sym, i + 1, g));
    }
    out.pop(); // 去掉末尾换行
    out
}

/// AI 任务上下文（Agent 模式锁定会话 + 目录）
#[derive(Clone)]
pub struct TaskCtx {
    /// 锁定会话名
    pub name: String,
    pub host: String,
    pub user: String,
    /// 任务开始时的权威 PWD（前端 OSC7）
    pub pwd: String,
    /// Docker 会话目标容器名（kind=Docker 时 AI 命令注入该容器）
    pub container: Option<String>,
}

/// 将流式事件映射为前端口径的 AI 事件（正文 → Streaming，思考 → Reasoning），并附会话名
fn stream_to_payload(session: &str, evt: AiStreamEvent) -> AiPayload {
    match evt {
        AiStreamEvent::Content(t) => AiPayload::Streaming { name: session.to_string(), text: t },
        AiStreamEvent::Reasoning(t) => AiPayload::Reasoning { name: session.to_string(), text: t },
    }
}

/// 把步骤进度账本渲染成喂给模型的"工作记忆"文本：
/// 每条一行（✓ 成功 / ✗ 失败+退出码 + 输出要点），供模型不再重做已完成步骤、只重规划失败子集
fn build_progress_summary(ledger: &[(String, bool, String)]) -> String {
    if ledger.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (cmd, ok, note) in ledger {
        if *ok {
            out.push_str(&format!("[✓] {} → {}\n", cmd, if note.is_empty() { "ok" } else { note }));
        } else {
            out.push_str(&format!("[✗] {}（失败）→ {}\n", cmd, if note.is_empty() { "error" } else { note }));
        }
    }
    out.pop(); // 去掉末尾换行
    out
}

/// 后台 AI 任务状态机：按模式驱动 Agent 引擎，事件经 app emit 回传（均带会话名）
pub(crate) async fn run_ai_job(
    session: &str,
    agent: &Arc<Mutex<Agent>>,
    ssh: &Arc<SshManager>,
    app: &AppHandle,
    ctl_rx: &mut UnboundedReceiver<AiControl>,
    input: &str,
    mode: AgentMode,
    ctx: Option<TaskCtx>,
    recorder: &crate::recorder::Recorder,
) {
    match mode {
        AgentMode::QA => {
            let mut ag = agent.lock().await;
            let mut sink = Some(|evt: AiStreamEvent| {
                let _ = app.emit("ai", stream_to_payload(session, evt));
            });
            let sink: Option<&mut (dyn FnMut(AiStreamEvent) + Send)> = sink.as_mut().map(|f| f as _);
            let chat_fut = ag.chat(input, sink);
            tokio::pin!(chat_fut);
            tokio::select! {
                result = &mut chat_fut => match result {
                    Ok(reply) => {
                        // P70 训练数据：QA 问答对
                        recorder.log(serde_json::json!({
                            "type": "qa", "session": session,
                            "q": crate::agent::truncate_text(input, 2000),
                            "a": crate::agent::truncate_text(&reply, 4000),
                        }));
                        let _ = app.emit("ai", AiPayload::Done { name: session.to_string(), message: reply });
                    }
                    Err(e) => {
                        recorder.log(serde_json::json!({
                            "type": "qa", "session": session, "ok": false,
                            "q": crate::agent::truncate_text(input, 2000),
                            "a": crate::agent::truncate_text(&e.to_string(), 1000),
                        }));
                        let _ = app.emit("ai", AiPayload::Error { name: session.to_string(), message: e.to_string() });
                    }
                },
                // 收到 Cancel(ai_stop)时,drop chat_fut 会中断进行中的 HTTP 请求
                ctl = ctl_rx.recv() => {
                    if ctl == Some(AiControl::Cancel) {
                        let _ = app.emit("ai", AiPayload::Done { name: session.to_string(), message: "已停止".to_string() });
                    }
                }
            }
            drop(chat_fut);
        }
        AgentMode::Agent => {
            let ctx = match ctx {
                Some(ctx) => ctx,
                None => {
                    let _ = app.emit("ai", AiPayload::Done { name: session.to_string(), message: "任务已结束（会话未锁定）".to_string() });
                    return;
                }
            };
            let (max_steps, max_output_chars, confirm_all, timeout_secs) = {
                let ag = agent.lock().await;
                (
                    ag.max_steps(),
                    ag.max_output_chars(),
                    ag.agent_confirm(),
                    ag.command_timeout_secs(),
                )
            };
            let mut cwd = if ctx.pwd.is_empty() { "/".to_string() } else { ctx.pwd.clone() };
            // P70 训练数据：任务起点（环境 + 目标 + 初始目录）
            recorder.log(serde_json::json!({
                "type": "ai_task", "session": session, "host": ctx.host,
                "task": crate::agent::truncate_text(input, 2000), "pwd": cwd,
            }));
            // cwd 恒为绝对路径（无尾斜杠），作初始执行目录
            let mut last_output = String::new();
            let mut finished = false;
            // 步骤进度账本（工作记忆）：记录每条命令的成功/失败与要点回读，
            // 供 agent_step 注入，防长任务被 max_history 裁剪后模型遗忘目标/重做已完成步骤
            let mut ledger: Vec<(String, bool, String)> = Vec::new();
            const MAX_LEDGER: usize = 8;
            // 连续未给出可执行命令的次数：到达阈值即中止，防弱模型死循环（提示词回显等）
            let mut invalid_steps = 0;
            const MAX_INVALID_STEPS: u32 = 2;
            // P57 L2 子目标任务图（内存态）：goals 与 go_states 同步等长；
            // 按"首个非 Ok"推进；每步以退出码确定性判成/败，失败停留供重规划、成功自动前进；
            // 防"过早收敛"：尚有未完成子目标时拒绝裸 DONE。
            // go_fail_retries 记录每子目标失败重试次数，超限即放弃跳过（防烧穿 max_steps）。
            const MAX_GOAL_FAIL_RETRIES: u32 = 2;
            let mut goals: Vec<String> = Vec::new();
            let mut go_states: Vec<GoalStatus> = Vec::new();
            let mut go_fail_retries: Vec<u32> = Vec::new();
            let mut last_reflexion: Option<String> = None;
            let mut premature_done = 0u32;

            // §8.7.1 唯一状态机：显式迁态并广播，前端只订阅 State 判态
            let emit_state = |st: AiRunState| {
                let _ = app.emit("ai", AiPayload::State { name: session.to_string(), state: st });
            };
            // 计划级确认前清空推理/执行期间滞留的陈旧信号，防误消费；
            // 滞留中的 Cancel 必须兑现（返回 true），否则用户点停止会被当陈旧信号吞掉
            let flush_stale = |ctl_rx: &mut UnboundedReceiver<AiControl>| -> bool {
                let mut cancelled = false;
                while let Ok(msg) = ctl_rx.try_recv() {
                    if msg == AiControl::Cancel {
                        cancelled = true;
                    }
                }
                cancelled
            };

            // P85 内置技能：任务命中技能库 → 确定性执行剧本，完全绕过 LLM
            // （常见运维任务零幻觉秒级完成；技能轨迹带 source:"skill" 金标，可直接作训练数据）
            if let Some(sk) = crate::skills::match_skill(input) {
                let steps: Vec<&str> = sk.steps.to_vec();
                let plan_cmds: Vec<PlanCommand> = steps
                    .iter()
                    .map(|c| {
                        let (level, reason) = check_danger(c);
                        PlanCommand { command: c.to_string(), level, reason }
                    })
                    .collect();
                let any_danger = plan_cmds.iter().any(|c| c.level != DangerLevel::Safe);
                emit_state(AiRunState::Planning);
                let _ = app.emit(
                    "ai",
                    AiPayload::Planning {
                        name: session.to_string(),
                        commands: plan_cmds,
                        need_confirm: confirm_all || any_danger,
                    },
                );

                // 危险步骤照常走计划级确认（安全链与 LLM 路径一致）
                if confirm_all || any_danger {
                    emit_state(AiRunState::AwaitingConfirm);
                    if flush_stale(ctl_rx) {
                        emit_state(AiRunState::Idle);
                        return;
                    }
                    match recv_confirm(ctl_rx).await {
                        Ok(Some(AiControl::Approve)) => {}
                        Ok(Some(AiControl::Reject)) => {
                            let _ = app.emit(
                                "ai",
                                AiPayload::Done { name: session.to_string(), message: "已放弃技能执行".to_string() },
                            );
                            emit_state(AiRunState::Idle);
                            return;
                        }
                        Ok(_) => {
                            emit_state(AiRunState::Idle);
                            return;
                        }
                        Err(()) => {
                            let _ = app.emit(
                                "ai",
                                AiPayload::Done { name: session.to_string(), message: "等待确认超时,技能执行已中止".to_string() },
                            );
                            emit_state(AiRunState::Idle);
                            return;
                        }
                    }
                }

                emit_state(AiRunState::Executing);
                let mut ok_count = 0usize;
                let mut fail_count = 0usize;
                for (i, command) in steps.iter().enumerate() {
                    if ctl_rx.try_recv().ok() == Some(AiControl::Cancel) {
                        let _ = app.emit(
                            "ai",
                            AiPayload::Done { name: session.to_string(), message: "已停止".to_string() },
                        );
                        emit_state(AiRunState::Idle);
                        return;
                    }
                    let (output, code, new_pwd) =
                        task_exec(ssh, app, &ctx.name, command, &cwd, timeout_secs, ctx.container.as_deref()).await;
                    if !new_pwd.is_empty() {
                        cwd = new_pwd;
                    }
                    if code == 0 {
                        ok_count += 1;
                    } else {
                        fail_count += 1;
                    }
                    // 金标训练数据：source:"skill"（确定性执行轨迹）
                    recorder.log(serde_json::json!({
                        "type": "ai_step", "session": session, "host": ctx.host,
                        "source": "skill", "skill": sk.id,
                        "step": i + 1, "command": command,
                        "exit_code": code, "pwd": cwd,
                        "output": truncate_text(output.trim(), 2000),
                    }));
                    let _ = app.emit(
                        "ai",
                        AiPayload::CommandStep {
                            name: session.to_string(),
                            command: command.to_string(),
                            success: code == 0,
                            message: if code == 0 { String::new() } else { format!("退出码 {}", code) },
                            output: truncate_text(&output, 2000),
                        },
                    );
                }
                let fail_note = if fail_count > 0 {
                    format!("、{} 步失败", fail_count)
                } else {
                    String::new()
                };
                let _ = app.emit(
                    "ai",
                    AiPayload::Done {
                        name: session.to_string(),
                        message: format!("内置技能「{}」执行完成：{} 步成功{}", sk.title, ok_count, fail_note),
                    },
                );
                emit_state(AiRunState::Idle);
                return;
            }
            for _step in 0..max_steps {
                if ctl_rx.try_recv().ok() == Some(AiControl::Cancel) {
                    finished = true;
                    break;
                }
                let _ = app.emit("ai", AiPayload::StepBegin { name: session.to_string() });
                emit_state(AiRunState::Parsing);
                let ctx_desc = if ctx.user.is_empty() {
                    format!("会话 {}，当前目录 {}", ctx.name, cwd)
                } else {
                    format!("会话 {} ({}@{})，当前目录 {}", ctx.name, ctx.user, ctx.host, cwd)
                };
                let goal_summary = build_goal_summary(&goals, &go_states);
                let progress_summary = build_progress_summary(&ledger);
                // P57 L2 工作记忆：目标进度 + 上次失败反思 + 命令账本，一次注入，防长任务遗忘目标
                let mut work_mem = String::new();
                if !goal_summary.is_empty() {
                    work_mem.push_str("【子目标进度】\n");
                    work_mem.push_str(&goal_summary);
                    work_mem.push('\n');
                }
                if let Some(r) = &last_reflexion {
                    work_mem.push_str(&format!("【上次失败反思】{}\n", r));
                }
                if !progress_summary.is_empty() {
                    work_mem.push_str("【命令账本】\n");
                    work_mem.push_str(&progress_summary);
                }
                // 模型调用期间同步监听控制通道：用户"停止"时走 select 取消分支,
                // drop 掉未完成的 agent_step(即中断进行中的 HTTP 请求)并释放 agent 锁,
                // 使 ai_stop 的 reset_task/clear_history 立即拿到锁,停止不再等整个 step 超时。
                let mut next = None;
                // P83：FC 回合结果（fc_active 时走 agent_step_fc，否则走文本协议）
                let mut fc_turn: Option<Result<FcTurn, String>> = None;
                {
                    let mut ag = agent.lock().await;
                    if ag.fc_active() {
                        // FC 路径：非流式（工具调用参数需要完整 JSON），无 sink
                        let fut = ag.agent_step_fc(input, &last_output, &ctx_desc, &work_mem);
                        tokio::pin!(fut);
                        loop {
                            tokio::select! {
                                r = &mut fut => { fc_turn = Some(r.map_err(|e| e.to_string())); break; }
                                ctl = ctl_rx.recv() => {
                                    if ctl == Some(AiControl::Cancel) {
                                        let _ = app.emit("ai", AiPayload::Done { name: session.to_string(), message: "已停止".to_string() });
                                        finished = true;
                                        break;
                                    }
                                }
                            }
                        }
                    } else {
                        let mut sink = Some(|evt: AiStreamEvent| {
                            let _ = app.emit("ai", stream_to_payload(session, evt));
                        });
                        let sink: Option<&mut (dyn FnMut(AiStreamEvent) + Send)> =
                            sink.as_mut().map(|f| f as _);
                        let fut = ag.agent_step(input, &last_output, &ctx_desc, &work_mem, sink);
                        tokio::pin!(fut);
                        // 仅 Cancel 结束任务;陈旧的 Approve/Reject 忽略并继续等模型返回,
                        // 否则一个错发的确认信号会静默终止整个任务。
                        loop {
                            tokio::select! {
                                r = &mut fut => { next = Some(r); break; }
                                ctl = ctl_rx.recv() => {
                                    if ctl == Some(AiControl::Cancel) {
                                        let _ = app.emit("ai", AiPayload::Done { name: session.to_string(), message: "已停止".to_string() });
                                        finished = true;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                let _ = app.emit("ai", AiPayload::StepOutputEnd { name: session.to_string() });
                if finished {
                    break;
                }

                // ── P83 FC 分流：FC 回合转换为本步动作，文本回合走既有解析 ──
                enum TurnOutcome {
                    Text(String),
                    Calls(Vec<FcCall>),
                }
                let outcome: TurnOutcome = if let Some(r) = fc_turn {
                    match r {
                        Ok(FcTurn::DowngradedToText(text)) => {
                            // 服务端不支持 tools：粘性降级，本回合按文本协议处理
                            TurnOutcome::Text(text)
                        }
                        Ok(FcTurn::Invalid(content)) => {
                            // 模型收下 tools 却返回纯文本（不产生 tool_calls）：
                            // 粘性降级为文本协议，把这段文本原样交给既有解析路径（零损失回退）
                            {
                                let mut ag = agent.lock().await;
                                ag.fc_force_disable();
                            }
                            let _ = app.emit(
                                "ai",
                                AiPayload::CommandStep {
                                    name: session.to_string(),
                                    command: "FC 降级".to_string(),
                                    success: true,
                                    message: "模型不支持工具调用，已自动切换文本协议".to_string(),
                                    output: String::new(),
                                },
                            );
                            TurnOutcome::Text(content)
                        }
                        Ok(FcTurn::Done { finish_id, summary }) => {
                            // finish 也受子目标护栏约束（防弱模型过早收敛）；
                            // 未收口时回填工具结果让模型重试，而不是直接终结
                            let remaining = !goals.is_empty()
                                && go_states.iter().any(|s| !matches!(s, GoalStatus::Ok));
                            if remaining {
                                premature_done += 1;
                                {
                                    let mut ag = agent.lock().await;
                                    ag.push_tool_result(&finish_id, "任务尚未收口：仍有未完成的子目标，请继续调用工具推进");
                                }
                                if premature_done >= 2 {
                                    let _ = app.emit(
                                        "ai",
                                        AiPayload::Done {
                                            name: session.to_string(),
                                            message: "任务已结束（模型多次提前声明完成，部分子目标未完成）".to_string(),
                                        },
                                    );
                                    finished = true;
                                    break;
                                }
                                emit_state(AiRunState::ReadingBack);
                                continue;
                            }
                            let _ = app.emit("ai", AiPayload::Done { name: session.to_string(), message: summary });
                            finished = true;
                            break;
                        }
                        Ok(FcTurn::Calls(calls)) => TurnOutcome::Calls(calls),
                        Err(e) => {
                            let _ = app.emit("ai", AiPayload::Error { name: session.to_string(), message: e });
                            finished = true;
                            break;
                        }
                    }
                } else {
                    let next = next.expect("select 分支保证:未取消时必有结果");
                    let next = match next {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = app.emit("ai", AiPayload::Error { name: session.to_string(), message: e.to_string() });
                            finished = true;
                            break;
                        }
                    };
                    TurnOutcome::Text(next)
                };

                let step_active: Option<usize>;
                let mut commands: Vec<String> = Vec::new();
                // P83 FC：工具结果回填表 (id, 结果文本)；命令类结果在执行后填充
                let mut fc_results: Vec<(String, String)> = Vec::new();
                // P83：显式 goal_ok 标记（FC）——在命令执行前结算当前子目标
                let mut fc_goal_ok = false;
                let mut cmd_ids: Vec<String> = Vec::new();
                let mut fc_outputs: Vec<String> = Vec::new();
                let is_fc = matches!(outcome, TurnOutcome::Calls(_));

                match outcome {
                    TurnOutcome::Text(next) => {
                        // P57 L2：解析子目标/反思标记（不入命令队列）
                        for g in extract_goals(&next) {
                            if !goals.iter().any(|x| x == &g) {
                                goals.push(g);
                                go_states.push(GoalStatus::Pending);
                                go_fail_retries.push(0u32);
                            }
                        }
                        if let Some(r) = extract_reflexion(&next) {
                            last_reflexion = Some(r);
                        }
                        let active_idx = next_active_goal(&go_states);
                        if let Some(idx) = active_idx {
                            if go_states[idx] == GoalStatus::Pending {
                                go_states[idx] = GoalStatus::Active;
                                let _ = app.emit(
                                    "ai",
                                    AiPayload::GoalStarted {
                                        name: session.to_string(),
                                        goal_index: idx as u32,
                                        title: goals[idx].clone(),
                                    },
                                );
                            }
                        }
                        step_active = active_idx;
                        // P57 L2 防"过早收敛"：仍有未完成/失败子目标时,不接受裸 DONE,
                        // 提醒模型继续推进或重规划当前子目标（含有界计数器防止弱模型反复假完成死循环）。
                        if next.trim().eq_ignore_ascii_case("DONE") {
                            let remaining = !goals.is_empty()
                                && go_states.iter().any(|s| !matches!(s, GoalStatus::Ok));
                            if remaining {
                                premature_done += 1;
                                let remaining_count = goals.iter().zip(&go_states)
                                    .filter(|(_, s)| !matches!(s, GoalStatus::Ok))
                                    .count();
                                last_output = format!(
                                    "[提示] 子目标任务尚未收口（{} 个子目标中仍有 {} 个未完成），不能提前结束：请针对当前失败/未完成的子目标继续输出命令;失败子目标要先输出 REFLEXION 再重规划。",
                                    goals.len(),
                                    remaining_count
                                );
                                if premature_done >= 2 {
                                    let _ = app.emit(
                                        "ai",
                                        AiPayload::Done {
                                            name: session.to_string(),
                                            message: "任务已结束（模型多次提前声明完成，部分子目标未完成）".to_string(),
                                        },
                                    );
                                    finished = true;
                                    break;
                                }
                                continue;
                            }
                            let _ = app.emit("ai", AiPayload::Done { name: session.to_string(), message: "任务完成".to_string() });
                            finished = true;
                            break;
                        }
                        commands = parse_commands(&next);
                        if commands.is_empty() {
                            // P57 L2：空命令但带显式终结标记 GOAL_OK/GOAL_FAIL(用于"校验/查证"类空命令子目标)
                            if let (Some(idx), Some(ok)) = (step_active, goal_marker(&next)) {
                                let (final_ok, skipped) = apply_goal_outcome(
                                    idx,
                                    ok,
                                    &mut go_states,
                                    &mut go_fail_retries,
                                    MAX_GOAL_FAIL_RETRIES,
                                );
                                let _ = app.emit(
                                    "ai",
                                    AiPayload::GoalDone {
                                        name: session.to_string(),
                                        goal_index: idx as u32,
                                        status: if final_ok { "ok".to_string() } else { "failed".to_string() },
                                    },
                                );
                                if final_ok {
                                    last_output = if skipped {
                                        format!("子目标「{}」多次失败已放弃跳过。请继续下一个子目标,或全部完成则只输出 DONE。", goals[idx])
                                    } else {
                                        format!("子目标「{}」已完成。请继续下一个子目标,或全部完成则只输出 DONE。", goals[idx])
                                    };
                                } else {
                                    last_output = format!("子目标「{}」自检失败。请先输出 REFLEXION 再重规划该子目标。", goals[idx]);
                                }
                                continue;
                            }
                            invalid_steps += 1;
                            let _ = app.emit(
                                "ai",
                                AiPayload::CommandStep {
                                    name: session.to_string(),
                                    command: next,
                                    success: false,
                                    message: "模型未给出可执行命令".to_string(),
                                    output: String::new(),
                                },
                            );
                            if invalid_steps >= MAX_INVALID_STEPS {
                                let _ = app.emit(
                                    "ai",
                                    AiPayload::Done {
                                        name: session.to_string(),
                                        message: "模型连续未给出可执行命令，任务已中止".to_string(),
                                    },
                                );
                                finished = true;
                                break;
                            }
                            last_output = "错误: 模型未给出可执行命令，请直接输出 shell 命令".to_string();
                            continue;
                        }
                    }
                    TurnOutcome::Calls(calls) => {
                        // P83：非命令动作先落账，命令收集进共享执行链
                        for FcCall { id, action } in calls {
                            match action {
                                FcAction::Command(cmd) => {
                                    commands.push(cmd);
                                    cmd_ids.push(id);
                                }
                                FcAction::Goal(title) => {
                                    if !goals.iter().any(|x| x == &title) {
                                        goals.push(title.clone());
                                        go_states.push(GoalStatus::Pending);
                                        go_fail_retries.push(0u32);
                                    }
                                    fc_results.push((id, format!("子目标「{title}」已登记")));
                                }
                                FcAction::GoalOk => {
                                    fc_results.push((id, "已标记当前子目标完成".to_string()));
                                    fc_goal_ok = true;
                                }
                                FcAction::Reflect(r) => {
                                    last_reflexion = Some(r.clone());
                                    fc_results.push((id, "已记录反思".to_string()));
                                }
                                FcAction::Noop => {
                                    fc_results.push((id, "无效调用".to_string()));
                                }
                            }
                        }
                        // 激活首个 Pending 子目标（镜像文本路径的 GoalStarted 事件）
                        if let Some(idx) = next_active_goal(&go_states) {
                            if go_states[idx] == GoalStatus::Pending {
                                go_states[idx] = GoalStatus::Active;
                                let _ = app.emit(
                                    "ai",
                                    AiPayload::GoalStarted {
                                        name: session.to_string(),
                                        goal_index: idx as u32,
                                        title: goals[idx].clone(),
                                    },
                                );
                            }
                        }
                        step_active = next_active_goal(&go_states);
                        if commands.is_empty() {
                            // 本回合无命令：回填全部工具结果，直接进入下一轮
                            {
                                let mut ag = agent.lock().await;
                                for (id, out) in &fc_results {
                                    ag.push_tool_result(id, out);
                                }
                            }
                            emit_state(AiRunState::ReadingBack);
                            continue;
                        }
                    }
                }
                invalid_steps = 0;

                // P83：FC 显式 goal_ok——在命令执行前结算当前子目标（Ok 推进）
                if fc_goal_ok {
                    if let Some(idx) = step_active {
                        let (final_ok, _) = apply_goal_outcome(
                            idx,
                            true,
                            &mut go_states,
                            &mut go_fail_retries,
                            MAX_GOAL_FAIL_RETRIES,
                        );
                        let _ = app.emit(
                            "ai",
                            AiPayload::GoalDone {
                                name: session.to_string(),
                                goal_index: idx as u32,
                                status: if final_ok { "ok".to_string() } else { "failed".to_string() },
                            },
                        );
                    }
                }

                // §8.7.2 整份计划卡：parse_commands 之后、逐命令之前广播计划
                let plan_cmds: Vec<PlanCommand> = commands
                    .iter()
                    .map(|c| {
                        let (level, reason) = check_danger(c);
                        PlanCommand { command: c.clone(), level, reason }
                    })
                    .collect();
                let any_danger = plan_cmds.iter().any(|c| c.level != DangerLevel::Safe);
                emit_state(AiRunState::Planning);
                let need_confirm = confirm_all || any_danger;
                let _ = app.emit("ai", AiPayload::Planning { name: session.to_string(), commands: plan_cmds, need_confirm });

                // §8.7.3 计划级一次性确认：整份批准/编辑/放弃。
                // 安全保证：批准计划 = 对本步全部命令（含计划卡上带风险标记的危险命令）的一次性显式授权，
                // 故后续不再对已批准命令二次逐条确认（消除"双确认"冲突）；
                // 用户「编辑」则视为新命令，preapproved 复位，改后新增的危险命令重新逐条确认，
                // 绝不因编辑绕过授权。严格确认模式(confirm_all)仍逐条确认，见下方 need_confirm。
                let mut preapproved = false;
                let commands: Vec<String> = if confirm_all || any_danger {
                    emit_state(AiRunState::AwaitingConfirm);
                    if flush_stale(ctl_rx) {
                        // 滞留 Cancel 兑现：用户已停止，不再弹计划卡等待
                        finished = true;
                        break;
                    }
                    match recv_confirm(ctl_rx).await {
                        Ok(Some(AiControl::Approve)) => {
                            preapproved = true;
                            let _ = app.emit(
                                "ai",
                                AiPayload::StepOutputEnd { name: session.to_string() },
                            );
                            commands
                        }
                        Ok(Some(AiControl::Edit(cmds))) => cmds,
                        Ok(Some(AiControl::Reject)) => {
                            let _ = app.emit(
                                "ai",
                                AiPayload::Done { name: session.to_string(), message: "已放弃本步计划".to_string() },
                            );
                            finished = true;
                            break;
                        }
                        // 通道关闭或主动取消：直接结束，不额外提示
                        Ok(_) => {
                            finished = true;
                            break;
                        }
                        // 确认超时：中止任务(不静默执行未批计划)
                        Err(()) => {
                            let _ = app.emit(
                                "ai",
                                AiPayload::Done { name: session.to_string(), message: "等待确认超时,本次计划已中止".to_string() },
                            );
                            finished = true;
                            break;
                        }
                    }
                } else {
                    commands
                };
                if finished {
                    break;
                }
                emit_state(AiRunState::Executing);
                // P57 L2：本步确定性子目标结局统计（仅当确已执行命令时才能判子目标成/败）
                let mut step_had_command = false;
                let mut step_any_fail = false;

                for command in commands {
                    if ctl_rx.try_recv().ok() == Some(AiControl::Cancel) {
                        finished = true;
                        break;
                    }
                    let (level, reason) = check_danger(&command);
                    // 危险命令授权判定：
                    // - confirm_all(严格确认模式)恒置真 → 每条命令无论是否为已批准计划仍在逐条确认，绝不裸执行；
                    // - 非严格模式 && 计划已批准(preapproved) → 该命令已被整份授权，跳过二次确认；
                    // - 否则(未批准/编辑后)危险命令仍逐条确认。
                    let need_confirm = confirm_all || (!preapproved && level != DangerLevel::Safe);
                    if need_confirm {
                        let _ = app.emit(
                            "ai",
                            AiPayload::PendingCommand {
                                name: session.to_string(),
                                command: command.clone(),
                                level,
                                reason,
                            },
                        );
                        // 推理/执行期间滞留的陈旧 Approve/Reject 必须先清空,
                        // 否则会被本次确认消费,导致危险命令未经用户确认即执行。
                        loop {
                            match ctl_rx.try_recv() {
                                Ok(AiControl::Cancel) => { finished = true; }
                                Ok(_) => {}
                                Err(_) => break,
                            }
                        }
                        if finished {
                            break;
                        }
                        match recv_confirm(ctl_rx).await {
                            Ok(Some(AiControl::Approve)) => {}
                            Ok(Some(AiControl::Reject)) => {
                                // P70：人类干预信号（跳过）同样进训练轨迹
                                recorder.log(serde_json::json!({
                                    "type": "ai_step", "session": session,
                                    "command": command, "level": format!("{level:?}"), "skipped": true,
                                }));
                                let _ = app.emit(
                                    "ai",
                                    AiPayload::CommandStep {
                                        name: session.to_string(),
                                        command,
                                        success: false,
                                        message: "已跳过".to_string(),
                                        output: String::new(),
                                    },
                                );
                                if is_fc {
                                    fc_outputs.push("用户跳过".to_string());
                                }
                                last_output = "命令已由用户跳过".to_string();
                                continue;
                            }
                            // 计划级 Edit 若滞留到逐条确认（异常时序），按跳过处理，不执行
                            Ok(Some(AiControl::Edit(_))) => { continue; }
                            // 通道关闭或主动取消或确认超时：中止任务，不执行未获批准的命令
                            Ok(_) | Err(()) => {
                                finished = true;
                                break;
                            }
                        }
                    }
                    let (output, code, new_pwd) =
                        task_exec(ssh, app, &ctx.name, &command, &cwd, timeout_secs, ctx.container.as_deref()).await;
                    // P57 L2：以退出码做确定性判成/败（不靠模型自夸）
                    step_had_command = true;
                    if code != 0 {
                        step_any_fail = true;
                    }
                    // 权威 cwd 跟随：命令内部 cd 后由 ###HELM_PWD### 回传
                    if !new_pwd.is_empty() {
                        cwd = new_pwd;
                    }
                    if is_fc {
                        // P83：FC 回合捕获每条命令输出，供工具结果回填
                        fc_outputs.push(truncate_text(&output, max_output_chars));
                    }
                    // P70 训练核心三元组：(任务上下文, 动作命令, 结果输出+退出码+目录)
                    recorder.log(serde_json::json!({
                        "type": "ai_step", "session": session, "host": ctx.host,
                        "step": _step, "command": command,
                        "level": format!("{level:?}"), "exit_code": code, "pwd": cwd,
                        "output": truncate_text(output.trim(), 2000),
                    }));
                    // 工作记忆账本：记录本条命令及结果要点；失败保留退出码，供模型只重规划失败子集
                    let note = truncate_text(&output.trim().replace('\n', " "), 80);
                    ledger.push((command.clone(), code == 0, note));
                    if ledger.len() > MAX_LEDGER {
                        ledger.remove(0);
                    }
                    last_output = format!(
                        "命令: {}\n退出码: {}\n输出:\n{}",
                        command,
                        code,
                        truncate_text(&output, max_output_chars),
                    );
                }
                // P57 L2：确已执行命令后,按退出码确定性更新当前子目标结局。
                // 成功→Ok(自动推进到下一未完成子目标);失败→Failed(停留,供模型 REFLEXION 后重规划该子目标)。
                if !finished && step_had_command {
                    if let Some(idx) = step_active {
                        let (final_ok, skipped) = apply_goal_outcome(
                            idx,
                            !step_any_fail,
                            &mut go_states,
                            &mut go_fail_retries,
                            MAX_GOAL_FAIL_RETRIES,
                        );
                        let _ = app.emit(
                            "ai",
                            AiPayload::GoalDone {
                                name: session.to_string(),
                                goal_index: idx as u32,
                                status: if final_ok { "ok".to_string() } else { "failed".to_string() },
                            },
                        );
                        if skipped {
                            last_output = format!(
                                "子目标「{}」已多次失败,按重试上限放弃跳过,请勿再重试该子目标,直接继续下一个;全部完成则只输出 DONE。\n{}",
                                goals[idx], last_output
                            );
                        }
                    }
                }
                // P83：FC 回合——回填全部工具结果后进入下一轮（不走文本协议尾部）
                if is_fc {
                    {
                        let mut ag = agent.lock().await;
                        for (id, out) in &fc_results {
                            ag.push_tool_result(id, out);
                        }
                        for (i, id) in cmd_ids.iter().enumerate() {
                            let out = fc_outputs.get(i).map(String::as_str).unwrap_or("（未执行）");
                            ag.push_tool_result(id, out);
                        }
                    }
                    emit_state(AiRunState::ReadingBack);
                    continue;
                }
                emit_state(AiRunState::ReadingBack);
                if finished {
                    break;
                }
                last_output = truncate_text(&last_output, max_output_chars);
            }

            if !finished {
                let _ = app.emit(
                    "ai",
                    AiPayload::Done {
                        name: session.to_string(),
                        message: format!("已达到最大执行步数（{}），任务中止", max_steps),
                    },
                );
            }
            emit_state(AiRunState::Idle);
            let mut ag = agent.lock().await;
            ag.clear_history();
            ag.reset_task();
        }
    }
}

/// 在锁定会话上执行单条 Agent 命令（内置目录包装与超时），返回 (输出, 退出码, 结束目录)
async fn task_exec(
    ssh: &Arc<SshManager>,
    app: &AppHandle,
    name: &str,
    command: &str,
    cwd: &str,
    timeout_secs: u64,
    container: Option<&str>,
) -> (String, i32, String) {
    let handle = ssh.exec_handle(name).await;
    let Some(handle) = handle else {
        let _ = app.emit(
            "ai",
            AiPayload::CommandStep {
                name: name.to_string(),
                command: command.to_string(),
                success: false,
                message: "会话已断开".to_string(),
                output: String::new(),
            },
        );
        return (format!("错误: 会话 {} 已断开", name), 1, String::new());
    };
    let kind = ssh.kind_of(name).await;
    match run_task_exec(handle, command, cwd, timeout_secs, kind, container).await {
        Ok(r) => {
            let _ = app.emit(
                "ai",
                AiPayload::CommandStep {
                    name: name.to_string(),
                    command: command.to_string(),
                    success: r.exit_code == 0,
                    message: if r.exit_code == 0 {
                        String::new()
                    } else {
                        format!("退出码 {}", r.exit_code)
                    },
                    output: truncate_text(&r.output, 2000),
                },
            );
            (r.output, r.exit_code, r.pwd)
        }
        Err(e) => {
            let _ = app.emit(
                "ai",
                AiPayload::CommandStep {
                    name: name.to_string(),
                    command: command.to_string(),
                    success: false,
                    message: e.to_string(),
                    output: String::new(),
                },
            );
            (format!("错误: {}", e), 1, String::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_goal_outcome, build_goal_summary, build_progress_summary, next_active_goal, GoalStatus,
    };

    #[test]
    fn progress_summary_empty_is_blank() {
        assert_eq!(build_progress_summary(&[]), "");
    }

    #[test]
    fn progress_summary_success_marks_check_and_trims_note() {
        let ledger = vec![("pwd".to_string(), true, "/home".to_string())];
        let s = build_progress_summary(&ledger);
        assert_eq!(s, "[✓] pwd → /home");
    }

    #[test]
    fn progress_summary_failure_keeps_exit_code_hint() {
        let ledger = vec![("systemctl start httpd".to_string(), false, "Failed".to_string())];
        let s = build_progress_summary(&ledger);
        assert_eq!(s, "[✗] systemctl start httpd（失败）→ Failed");
    }

    #[test]
    fn progress_summary_multiline_joins_lines() {
        let ledger = vec![
            ("ls".to_string(), true, "src".to_string()),
            ("cd /etc".to_string(), true, String::new()),
        ];
        let s = build_progress_summary(&ledger);
        assert_eq!(s, "[✓] ls → src\n[✓] cd /etc → ok");
    }

    #[test]
    fn goal_summary_marks_ok_fail_and_pending() {
        let goals = vec![
            "预检环境".to_string(),
            "安装依赖".to_string(),
            "启动服务".to_string(),
        ];
        let st = vec![GoalStatus::Ok, GoalStatus::Failed, GoalStatus::Pending];
        let s = build_goal_summary(&goals, &st);
        assert_eq!(s, "  ✔ 1. 预检环境\n  ✘ 2. 安装依赖\n  … 3. 启动服务");
    }

    #[test]
    fn goal_summary_empty_if_no_goals() {
        assert_eq!(build_goal_summary(&[], &[]), "");
    }

    #[test]
    fn next_active_picks_first_non_ok() {
        let st = vec![GoalStatus::Ok, GoalStatus::Failed, GoalStatus::Pending];
        assert_eq!(next_active_goal(&st), Some(1)); // 失败者优先停留,供重规划
        let st2 = vec![GoalStatus::Ok, GoalStatus::Ok];
        assert_eq!(next_active_goal(&st2), None); // 全部完成
    }

    #[test]
    fn goal_outcome_success_marks_ok_and_advances() {
        let mut st = vec![GoalStatus::Active, GoalStatus::Pending];
        let mut retr = vec![0u32, 0u32];
        let (final_ok, skipped) = apply_goal_outcome(0, true, &mut st, &mut retr, 2);
        assert_eq!((final_ok, skipped), (true, false));
        assert_eq!(st[0], GoalStatus::Ok);
    }

    #[test]
    fn goal_outcome_fail_within_budget_marks_failed_for_replan() {
        let mut st = vec![GoalStatus::Active];
        let mut retr = vec![0u32];
        let (final_ok, skipped) = apply_goal_outcome(0, false, &mut st, &mut retr, 2);
        assert_eq!((final_ok, skipped), (false, false));
        assert_eq!(st[0], GoalStatus::Failed); // 未超上限：停留供 REFLEXION 重规划
        assert_eq!(retr[0], 1);
    }

    #[test]
    fn goal_outcome_exceeding_retries_abandons_and_advances() {
        // MAX=2：第 1、2 次失败停留重规划；第 3 次调用即累计超限 → 放弃(置 Ok 跳过)
        let mut st = vec![GoalStatus::Active];
        let mut retr = vec![0u32];
        apply_goal_outcome(0, false, &mut st, &mut retr, 2);
        apply_goal_outcome(0, false, &mut st, &mut retr, 2);
        assert_eq!(retr[0], 2);
        assert_eq!(st[0], GoalStatus::Failed);
        let (final_ok, skipped) = apply_goal_outcome(0, false, &mut st, &mut retr, 2);
        assert_eq!((final_ok, skipped), (false, true)); // 放弃：标记为"完成可推进"
        assert_eq!(st[0], GoalStatus::Ok);
        assert_eq!(next_active_goal(&st), None); // 全部(被放弃的子目标)视为完成 → 收敛
    }
}
