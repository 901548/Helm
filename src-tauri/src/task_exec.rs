// ============================================================================
// task_exec.rs - Agent 任务命令执行域（自 ssh.rs 拆出,P42）
//   - 命令组装（Linux bash / Windows PowerShell EncodedCommand 双分支,P34）
//   - 输出解析（###HELM_*### 标记：退出码 + 结束目录回传）
//   - 在 exec 通道上带超时执行单条命令
// 新增平台分支只改本文件,不涉及连接/PTY/SFTP 管理（ssh.rs）。
// ============================================================================

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use russh::ChannelMsg;

use crate::config::SessionKind;
use crate::ssh::SshHandler;

/// Agent 任务输出保留尾部上限（尾含 ###HELM_*### 标记，必须保留）
const MAX_TASK_OUTPUT_RETAIN: usize = 64 * 1024;

/// Agent 单步命令执行结果
pub struct TaskExecResult {
    /// 合并的 stdout+stderr（不含标记）
    pub output: String,
    /// 命令退出码（解析失败时 -1）
    pub exit_code: i32,
    /// 命令结束后的当前目录（bash $PWD，绝对路径）
    pub pwd: String,
}

/// 组装 Agent 单步执行命令：先 `cd` 到目标目录，再执行命令，
/// 结尾打 `###HELM_END###` / `###HELM_EXIT###<rc>` / `###HELM_PWD###<pwd>` 标记。
/// stderr 经 `2>&1` 并入 stdout，使输出完整且可统一解析。
pub fn task_exec_cmd(cwd: &str, cmd: &str) -> String {
    let cwd = cwd.replace('\'', "'\\''");
    format!(
        "cd '{cwd}'; {{ {cmd}; }} 2>&1; rc=$?; \
         printf '\\n###HELM_END###\\n###HELM_EXIT###%s\\n###HELM_PWD###%s\\n' \"$rc\" \"$PWD\""
    )
}

/// 将单步命令注入远端 Docker 容器执行（kind=Docker）。
/// 复用 `task_exec_cmd` 生成的内层脚本（含 cd + ###HELM_*### 标记），
/// 经 `docker exec -i <container> sh` 送入容器运行；内层含引号，故先 base64
/// 编码再管道解码，规避 docker exec 参数引号冲突。容器内需有 `sh` 与
/// `base64 -d`（coreutils/busybox 常见已内置）。标记解析与 Linux 版完全一致。
pub fn docker_exec_cmd(container: &str, cwd: &str, cmd: &str) -> String {
    let inner = task_exec_cmd(cwd, cmd);
    let b64 = data_encoding::BASE64.encode(inner.as_bytes());
    format!("docker exec -i {container} sh -c \"printf '%s' '{b64}' | base64 -d | sh\"")
}

/// Windows 版 Agent 单步执行命令（P34）：经 `powershell -EncodedCommand` 调用，
/// 不依赖远端默认 shell（cmd/PowerShell 均可），避免层层引号转义。
/// `Set-Location -LiteralPath` 切目录，`###HELM_*###` 标记与 Linux 版一致，
/// 使 `parse_task_output` 复用（退出码/结束目录回传语义不变）。
pub fn task_exec_cmd_windows(cwd: &str, cmd: &str) -> String {
    let cwd = cwd.replace('\'', "''");
    let script = format!(
        "Set-Location -LiteralPath '{cwd}'; {{ {cmd}; }} 2>&1; $rc=$LASTEXITCODE; \
         if ($null -eq $rc) {{ $rc = 0 }}; Write-Output ''; Write-Output '###HELM_END###'; \
         Write-Output \"###HELM_EXIT###$rc\"; Write-Output \"###HELM_PWD###$((Get-Location).Path)\""
    );
    format!("powershell -NoProfile -NonInteractive -EncodedCommand {}", encode_powershell(&script))
}

/// 将 PowerShell 脚本编码为 UTF-16LE base64（EncodedCommand 专用）
fn encode_powershell(script: &str) -> String {
    let mut u16: Vec<u8> = Vec::with_capacity(script.len() * 2);
    for u in script.encode_utf16() {
        u16.extend_from_slice(&u.to_le_bytes());
    }
    data_encoding::BASE64.encode(&u16)
}

/// 把 String 裁剪为只保留尾部 cap 字节（在字符边界处切断，防止多字节字符被截半）。
/// 用于 Agent 任务输出：命令产生的巨大输出只保留尾部，`###HELM_*###` 标记位于末尾不受影响。
fn trim_to_tail(s: &mut String, cap: usize) {
    if s.len() > cap {
        let excess = s.len() - cap;
        let cut = s.floor_char_boundary(excess);
        s.drain(..cut);
    }
}

/// 解析 `task_exec_cmd` 的输出：剥离尾标记，提取退出码与结束目录。
/// 未找到标记时原样返回（exit_code=-1，pwd 为空）。
pub fn parse_task_output(raw: &str) -> (String, i32, String) {
    const END: &str = "###HELM_END###";
    let Some(end_idx) = raw.rfind(END) else {
        return (raw.to_string(), -1, String::new());
    };
    let output = raw[..end_idx].trim_end_matches(['\n', '\r']).to_string();
    let tail = &raw[end_idx + END.len()..];
    let mut exit_code = -1;
    let mut pwd = String::new();
    if let Some(exit_pos) = tail.find("###HELM_EXIT###") {
        let rest = &tail[exit_pos + "###HELM_EXIT###".len()..];
        let code = rest.split('\n').next().unwrap_or("").trim();
        exit_code = code.parse::<i32>().unwrap_or(-1);
    }
    if let Some(pwd_pos) = tail.find("###HELM_PWD###") {
        pwd = tail[pwd_pos + "###HELM_PWD###".len()..]
            .trim_end_matches(['\n', '\r'])
            .to_string();
    }
    (output, exit_code, pwd)
}

/// 在已有连接句柄上执行 Agent 单步命令（可指定目录与超时），返回输出/退出码/结束目录。
///
/// 目录经 `cd '<cwd>'` 前置，模型中间 `cd` 也通过 `###HELM_PWD###` 标记回传，
/// 供任务循环维护权威 cwd。超时秒数最小抬升为 1。
pub async fn run_task_exec(
    handle: Arc<russh::client::Handle<SshHandler>>,
    cmd: &str,
    cwd: &str,
    timeout_secs: u64,
    kind: SessionKind,
    container: Option<&str>,
) -> Result<TaskExecResult> {
    let full = if kind == SessionKind::Windows {
        task_exec_cmd_windows(cwd, cmd)
    } else if kind == SessionKind::Docker {
        let container = container
            .filter(|c| !c.is_empty())
            .ok_or_else(|| anyhow!("Docker 会话缺少容器名"))?;
        docker_exec_cmd(container, cwd, cmd)
    } else {
        task_exec_cmd(cwd, cmd)
    };
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| anyhow!("打开会话通道失败: {}", e))?;
    channel
        .exec(true, full.as_str())
        .await
        .map_err(|e| anyhow!("发送命令失败: {}", e))?;

    let mut buf = String::new();
    let read_result = tokio::time::timeout(Duration::from_secs(timeout_secs.max(1)), async {
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    buf.push_str(&String::from_utf8_lossy(&data));
                    trim_to_tail(&mut buf, MAX_TASK_OUTPUT_RETAIN);
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    buf.push_str(&String::from_utf8_lossy(&data));
                    trim_to_tail(&mut buf, MAX_TASK_OUTPUT_RETAIN);
                }
                Some(_) => {}
                None => break,
            }
        }
    })
    .await;

    read_result.map_err(|_| anyhow!("命令执行超时（{}秒）: {}", timeout_secs, cmd))?;
    let (output, exit_code, pwd) = parse_task_output(&buf);
    Ok(TaskExecResult { output, exit_code, pwd })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 纯函数单元测试 ----------

    #[test]
    fn task_exec_cmd_prepends_cd_and_markers() {
        let s = task_exec_cmd("/opt/my dir", "ls -la");
        assert!(s.starts_with("cd '/opt/my dir';"));
        assert!(s.contains("2>&1"));
        assert!(s.contains("###HELM_EXIT###"));
        assert!(s.contains("###HELM_PWD###"));
    }

    #[test]
    fn task_exec_cmd_escapes_single_quotes_in_cwd() {
        let s = task_exec_cmd("/a'b", "ls");
        assert!(s.starts_with("cd '/a'\\''b';"), "got: {}", s);
    }

    #[test]
    fn parse_task_output_extracts_ok() {
        let raw = "total 4\n###HELM_END###\n###HELM_EXIT###0\n###HELM_PWD###/opt\n";
        let (out, code, pwd) = parse_task_output(raw);
        assert_eq!(out, "total 4");
        assert_eq!(code, 0);
        assert_eq!(pwd, "/opt");
    }

    #[test]
    fn parse_task_output_failure_code() {
        let raw = "ls: /x: No such file\n###HELM_END###\n###HELM_EXIT###2\n###HELM_PWD###/root\n";
        let (out, code, pwd) = parse_task_output(raw);
        assert!(out.contains("No such file"));
        assert_eq!(code, 2);
        assert_eq!(pwd, "/root");
    }

    #[test]
    fn task_exec_cmd_no_marker_returns_raw() {
        let (out, code, pwd) = parse_task_output("hello world");
        assert_eq!(out, "hello world");
        assert_eq!(code, -1);
        assert_eq!(pwd, "");
    }

    #[test]
    fn task_exec_cmd_windows_uses_encoded_powershell_and_markers() {
        let s = task_exec_cmd_windows("C:\\Data", "Get-ChildItem");
        // 以 powershell -EncodedCommand 开头（不依赖默认 shell）
        assert!(s.starts_with("powershell -NoProfile -NonInteractive -EncodedCommand "));
        let b64 = s.rsplit(' ').next().unwrap();
        let bytes = data_encoding::BASE64.decode(b64.as_bytes()).unwrap();
        let script = String::from_utf16_lossy(
            &bytes
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<_>>(),
        );
        assert!(script.contains("Set-Location -LiteralPath 'C:\\Data'"), "got: {}", script);
        assert!(script.contains("###HELM_EXIT###"));
        assert!(script.contains("###HELM_PWD###"));
    }

    #[test]
    fn task_exec_cmd_windows_escapes_quotes_in_cwd() {
        let s = task_exec_cmd_windows("C:\\my'home", "ls");
        let b64 = s.rsplit(' ').next().unwrap();
        let bytes = data_encoding::BASE64.decode(b64.as_bytes()).unwrap();
        let script = String::from_utf16_lossy(
            &bytes
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<_>>(),
        );
        // 单引号转义为两个单引号（PowerShell）
        assert!(script.contains("'C:\\my''home'"), "got: {}", script);
    }

    #[test]
    fn trim_to_tail_keeps_tail_within_cap() {
        let mut s = "前端中文".repeat(50) + &"y".repeat(200);
        trim_to_tail(&mut s, 64);
        assert!(s.len() <= 64);
        // 原尾部 200 个 y 全为 ASCII，裁剪后保留的尾部仍应全为 y
        assert!(s.chars().all(|c| c == 'y'));
    }

    #[test]
    fn trim_to_tail_leaves_small_strings_alone() {
        let mut s = "hello".to_string();
        trim_to_tail(&mut s, 64);
        assert_eq!(s, "hello");
    }

    #[test]
    fn docker_exec_cmd_embeds_base64_decodeable_inner() {
        let out = docker_exec_cmd("my-app", "/opt", "ls -la");
        // 注入远端 docker CLI：`docker exec -i <container> sh -c ...`
        assert!(out.starts_with("docker exec -i my-app sh -c "));
        // 内层命令经 base64 编码后由容器内 sh 解码执行
        let b64 = out
            .rsplit_once("printf '%s' '")
            .and_then(|(_, rest)| rest.split("'").next())
            .expect("should contain base64 payload");
        let decoded = String::from_utf8(data_encoding::BASE64.decode(b64.as_bytes()).unwrap())
            .unwrap();
        assert!(decoded.contains("cd '/opt'"));
        assert!(decoded.contains("ls -la"));
        assert!(decoded.contains("###HELM_END###"));
        assert!(decoded.contains("###HELM_PWD###"));
    }
}
