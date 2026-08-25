// ============================================================================
// ai_job.rs - AI 任务编排域（自 core.rs 拆出,P42）
//   - run_ai_job 状态机:QA 流式 / Agent 逐步执行(危险命令确认 + cwd 权威跟踪)
//   - task_exec:在锁定会话上执行单条 Agent 命令并回传事件
//   - TaskCtx:任务上下文(锁定会话 + 初始权威 PWD)
// core.rs 只留命令薄封装;新增任务模式/执行策略改本文件。
// ============================================================================

use std::sync::Arc;

use tauri::{AppHandle, Emitter};

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::Mutex;

use crate::agent::{parse_commands, truncate_text, Agent, AgentMode, AiStreamEvent};
use crate::core::{AiControl, AiPayload};
use crate::safety::{check_danger, DangerLevel};
use crate::ssh::SshManager;
use crate::task_exec::run_task_exec;

/// AI 任务上下文（Agent 模式锁定会话 + 目录）
#[derive(Clone)]
pub struct TaskCtx {
    /// 锁定会话名
    pub name: String,
    pub host: String,
    pub user: String,
    /// 任务开始时的权威 PWD（前端 OSC7）
    pub pwd: String,
}

/// 将流式事件映射为前端口径的 AI 事件（正文 → Streaming，思考 → Reasoning）
fn stream_to_payload(evt: AiStreamEvent) -> AiPayload {
    match evt {
        AiStreamEvent::Content(t) => AiPayload::Streaming { text: t },
        AiStreamEvent::Reasoning(t) => AiPayload::Reasoning { text: t },
    }
}

/// 后台 AI 任务状态机：按模式驱动 Agent 引擎，事件经 app emit 回传
pub(crate) async fn run_ai_job(
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
                let _ = app.emit("ai", stream_to_payload(evt));
            });
            let sink: Option<&mut (dyn FnMut(AiStreamEvent) + Send)> = sink.as_mut().map(|f| f as _);
            let chat_fut = ag.chat(input, sink);
            tokio::pin!(chat_fut);
            tokio::select! {
                result = &mut chat_fut => match result {
                    Ok(reply) => {
                        let _ = app.emit("ai", AiPayload::Done { message: reply });
                    }
                    Err(e) => {
                        let _ = app.emit("ai", AiPayload::Error { message: e.to_string() });
                    }
                },
                // 收到 Cancel(ai_stop)时,drop chat_fut 会中断进行中的 HTTP 请求
                ctl = ctl_rx.recv() => {
                    if ctl == Some(AiControl::Cancel) {
                        let _ = app.emit("ai", AiPayload::Done { message: "已停止".to_string() });
                    }
                }
            }
            drop(chat_fut);
        }
        AgentMode::Agent => {
            let ctx = match ctx {
                Some(ctx) => ctx,
                None => {
                    let _ = app.emit("ai", AiPayload::Done { message: "任务已结束（会话未锁定）".to_string() });
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
            // 连续未给出可执行命令的次数：到达阈值即中止，防弱模型死循环（提示词回显等）
            let mut invalid_steps = 0;
            const MAX_INVALID_STEPS: u32 = 2;

            for _step in 0..max_steps {
                if ctl_rx.try_recv().ok() == Some(AiControl::Cancel) {
                    finished = true;
                    break;
                }
                let _ = app.emit("ai", AiPayload::StepBegin);
                let ctx_desc = if ctx.user.is_empty() {
                    format!("会话 {}，当前目录 {}", ctx.name, cwd)
                } else {
                    format!("会话 {} ({}@{})，当前目录 {}", ctx.name, ctx.user, ctx.host, cwd)
                };
                // 模型调用期间同步监听控制通道：用户"停止"时走 select 取消分支,
                // drop 掉未完成的 agent_step(即中断进行中的 HTTP 请求)并释放 agent 锁,
                // 使 ai_stop 的 reset_task/clear_history 立即拿到锁,停止不再等整个 step 超时。
                let mut next = None;
                {
                    let mut ag = agent.lock().await;
                    let mut sink = Some(|evt: AiStreamEvent| {
                        let _ = app.emit("ai", stream_to_payload(evt));
                    });
                    let sink: Option<&mut (dyn FnMut(AiStreamEvent) + Send)> =
                        sink.as_mut().map(|f| f as _);
                    let fut = ag.agent_step(input, &last_output, &ctx_desc, sink);
                    tokio::pin!(fut);
                    // 仅 Cancel 结束任务;陈旧的 Approve/Reject 忽略并继续等模型返回,
                    // 否则一个错发的确认信号会静默终止整个任务。
                    loop {
                        tokio::select! {
                            r = &mut fut => { next = Some(r); break; }
                            ctl = ctl_rx.recv() => {
                                if ctl == Some(AiControl::Cancel) {
                                    let _ = app.emit("ai", AiPayload::Done { message: "已停止".to_string() });
                                    finished = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                let _ = app.emit("ai", AiPayload::StepOutputEnd);
                if finished {
                    break;
                }
                let next = next.expect("select 分支保证:未取消时必有结果");

                let next = match next {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = app.emit("ai", AiPayload::Error { message: e.to_string() });
                        finished = true;
                        break;
                    }
                };
                if next.trim().eq_ignore_ascii_case("DONE") {
                    let _ = app.emit("ai", AiPayload::Done { message: "任务完成".to_string() });
                    finished = true;
                    break;
                }

                let commands = parse_commands(&next);
                if commands.is_empty() {
                    invalid_steps += 1;
                    let _ = app.emit(
                        "ai",
                        AiPayload::CommandStep {
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

                for command in commands {
                    if ctl_rx.try_recv().ok() == Some(AiControl::Cancel) {
                        finished = true;
                        break;
                    }
                    let (level, reason) = check_danger(&command);
                    let need_confirm = confirm_all || level != DangerLevel::Safe;
                    if need_confirm {
                        let _ = app.emit(
                            "ai",
                            AiPayload::PendingCommand {
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
                        match ctl_rx.recv().await {
                            Some(AiControl::Approve) => {}
                            Some(AiControl::Reject) => {
                                let _ = app.emit(
                                    "ai",
                                    AiPayload::CommandStep {
                                        command,
                                        success: false,
                                        message: "已跳过".to_string(),
                                        output: String::new(),
                                    },
                                );
                                last_output = "命令已由用户跳过".to_string();
                                continue;
                            }
                            Some(AiControl::Cancel) | None => {
                                finished = true;
                                break;
                            }
                        }
                    }
                    let (output, code, new_pwd) =
                        task_exec(ssh, app, &ctx.name, &command, &cwd, timeout_secs).await;
                    // 权威 cwd 跟随：命令内部 cd 后由 ###HELM_PWD### 回传
                    if !new_pwd.is_empty() {
                        cwd = new_pwd;
                    }
                    last_output = format!(
                        "命令: {}\n退出码: {}\n输出:\n{}",
                        command,
                        code,
                        truncate_text(&output, max_output_chars),
                    );
                }
                if finished {
                    break;
                }
                last_output = truncate_text(&last_output, max_output_chars);
            }

            if !finished {
                let _ = app.emit(
                    "ai",
                    AiPayload::Done {
                        message: format!("已达到最大执行步数（{}），任务中止", max_steps),
                    },
                );
            }
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
) -> (String, i32, String) {
    let handle = ssh.exec_handle(name).await;
    let Some(handle) = handle else {
        let _ = app.emit(
            "ai",
            AiPayload::CommandStep {
                command: command.to_string(),
                success: false,
                message: "会话已断开".to_string(),
                output: String::new(),
            },
        );
        return (format!("错误: 会话 {} 已断开", name), 1, String::new());
    };
    let kind = ssh.kind_of(name).await;
    match run_task_exec(handle, command, cwd, timeout_secs, kind).await {
        Ok(r) => {
            let _ = app.emit(
                "ai",
                AiPayload::CommandStep {
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
