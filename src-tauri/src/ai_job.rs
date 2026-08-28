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

use crate::agent::{goal_marker, extract_goals, extract_reflexion, parse_commands, truncate_text, Agent, AgentMode, AiStreamEvent};
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
                        let _ = app.emit("ai", AiPayload::Done { name: session.to_string(), message: reply });
                    }
                    Err(e) => {
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
            // 计划级确认前清空推理/执行期间滞留的陈旧信号，防误消费
            let flush_stale = |ctl_rx: &mut UnboundedReceiver<AiControl>| {
                while let Ok(_) = ctl_rx.try_recv() {}
            };

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
                {
                    let mut ag = agent.lock().await;
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
                let _ = app.emit("ai", AiPayload::StepOutputEnd { name: session.to_string() });
                if finished {
                    break;
                }
                let next = next.expect("select 分支保证:未取消时必有结果");

                let next = match next {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = app.emit("ai", AiPayload::Error { name: session.to_string(), message: e.to_string() });
                        finished = true;
                        break;
                    }
                };
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
                // P57 L2 防"过早收敛"：仍有未完成/失败子目标时,不接受裸 DONE,
                // 提醒模型继续推进或重规划当前子目标（含有界计数器防止弱模型反复假完成死循环）。
                if next.trim().eq_ignore_ascii_case("DONE") {
                    let remaining = !goals.is_empty()
                        && go_states.iter().any(|s| !matches!(s, GoalStatus::Ok));
                    if remaining {
                        premature_done += 1;
                        last_output = format!(
                            "[提示] 子目标任务尚未收口（共 {} 个子目标仍有未完成），不能提前结束：请针对当前失败/未完成的子目标继续输出命令;失败子目标要先输出 REFLEXION 再重规划。",
                            goals.len()
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
                let step_active = active_idx;

                let commands = parse_commands(&next);
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
                invalid_steps = 0;

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
                let _ = app.emit("ai", AiPayload::Planning { name: session.to_string(), commands: plan_cmds });

                // §8.7.3 计划级一次性确认：整份批准/编辑/放弃。
                // 安全保证：批准计划 = 对本步全部命令（含计划卡上带风险标记的危险命令）的一次性显式授权，
                // 故后续不再对已批准命令二次逐条确认（消除"双确认"冲突）；
                // 用户「编辑」则视为新命令，preapproved 复位，改后新增的危险命令重新逐条确认，
                // 绝不因编辑绕过授权。严格确认模式(confirm_all)仍逐条确认，见下方 need_confirm。
                let mut preapproved = false;
                let commands: Vec<String> = if confirm_all || any_danger {
                    emit_state(AiRunState::AwaitingConfirm);
                    flush_stale(ctl_rx);
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
