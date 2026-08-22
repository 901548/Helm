// ============================================================================
// ssh.rs - SSH 连接管理模块
// 使用 russh 库管理多个 SSH 会话的建立、远程命令执行与断开连接。
// 所有方法均不 panic，错误通过 anyhow::Result 返回。
// ============================================================================

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use russh::client;
use russh::keys::key::PublicKey;
use russh::{ChannelMsg, Disconnect};
use russh_sftp::client::SftpSession;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::{Mutex, Notify};

use crate::config::{expand_tilde, SessionInfo, SessionKind};
use crate::known_hosts::KnownHostsStore;

/// 单次 exec 输出累积上限（超限即停读，丢尾部）
const MAX_EXEC_OUTPUT: usize = 8 * 1024 * 1024;
/// Agent 任务输出保留尾部上限（尾含 ###HELM_*### 标记，必须保留）

/// 会话连接状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SessionStatus {
    /// 已连接
    Connected,
    /// 未连接
    Disconnected,
    /// 连接中
    Connecting,
}

/// russh 客户端事件处理器：首次连接信任并记录主机密钥（TOFU），
/// 密钥变更时拒绝连接并记录告警（供 connect 呈现给用户）。
pub struct SshHandler {
    /// 目标主机（用于 known_hosts 索引）
    host: String,
    /// 目标端口
    port: u16,
    /// 共享主机密钥库
    store: Arc<Mutex<KnownHostsStore>>,
    /// 本次连接的主机密钥校验失败原因（连接失败后由调用方读取）
    key_error: Arc<Mutex<Option<String>>>,
}

#[async_trait]
impl client::Handler for SshHandler {
    type Error = russh::Error;

    /// TOFU 主机密钥校验：首次信任并记录；指纹一致通过；变更拒绝并告警
    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let mut store = self.store.lock().await;
        let ok = store.verify(&self.host, self.port, server_public_key);
        if !ok {
            if let Some(e) = store.take_pending_error() {
                *self.key_error.lock().await = Some(e);
            }
        }
        Ok(ok)
    }
}

/// 单个 SSH 会话的底层句柄
struct SshSession {
    /// 会话平台类型（P32：open_shell 注入/exec 命令语法按此分支）
    kind: SessionKind,
    /// russh 连接句柄，None 表示尚未建立连接
    handle: Option<Arc<client::Handle<SshHandler>>>,
    /// 交互式 shell 通道（连接成功后打开）
    shell: Option<InteractiveShell>,
    /// SFTP 会话（懒建缓存，P26：文件操作经 SFTP 而非 exec 通道）
    sftp: Option<Arc<SftpSession>>,
}

/// 发送给交互 shell 的任务命令
pub enum ShellCommand {
    /// 把按键字节发给远端
    Input(Vec<u8>),
    /// 通知远端终端尺寸变化
    Resize(u16, u16),
}

/// 交互式 shell 的通道句柄（读写分离）
pub struct InteractiveShell {
    /// 远端输出（待 UI 消费）
    pub rx: UnboundedReceiver<Vec<u8>>,
    /// 发送按键/尺寸命令
    pub tx: UnboundedSender<ShellCommand>,
}

/// SSH 连接管理器：按会话名管理多个连接
///
/// 细粒度锁设计（P26）：`sessions` 锁只做 map 增删/查找，瞬时持有；
/// 网络操作（connect/open_shell/disconnect）一律拿到 `Arc` 句柄后在锁外执行，
/// 因此一个会话的 15s 连接或 10s 开壳不会阻塞其它会话的输入/输出/状态查询。
/// shell 数据到达或通道结束时 `notify` 唤醒输出 poller（取代 50ms 忙轮询）。
pub struct SshManager {
    /// 会话名 -> 会话（每会话独立锁，网络操作不持此锁）
    sessions: Arc<Mutex<HashMap<String, Arc<Mutex<SshSession>>>>>,
    /// 当前活跃会话名
    active: Arc<Mutex<Option<String>>>,
    /// 共享主机密钥库（TOFU）
    known_hosts: Arc<Mutex<KnownHostsStore>>,
    /// 输出事件通知：shell 有输出或通道结束时唤醒输出 poller
    notify: Arc<Notify>,
    /// 连接任务进行中的会话名(双击守卫)
    connecting: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl SshManager {
    /// 创建一个空的连接管理器（内存态主机密钥库，不落盘）
    pub fn new() -> Self {
        Self::with_known_hosts(Arc::new(Mutex::new(KnownHostsStore::in_memory())))
    }

    /// 创建连接管理器并绑定持久化主机密钥库
    pub fn with_known_hosts(known_hosts: Arc<Mutex<KnownHostsStore>>) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            active: Arc::new(Mutex::new(None)),
            known_hosts,
            notify: Arc::new(Notify::new()),
            connecting: Arc::new(Mutex::new(std::collections::HashSet::new())),
        }
    }

    /// 标记会话为"连接中";返回 false 表示已有连接任务进行中(双击守卫)。
    /// connect 的成败出口都会清除标记。
    pub async fn mark_connecting(&self, name: &str) -> bool {
        let newly = self.connecting.lock().await.insert(name.to_string());
        let session = Arc::new(Mutex::new(SshSession {
            kind: SessionKind::Linux,
            handle: None,
            shell: None,
            sftp: None,
        }));
        self.sessions
            .lock()
            .await
            .entry(name.to_string())
            .or_insert(session);
        newly
    }

    /// 清除"连接中"标记(connect 成败出口调用)
    async fn clear_connecting(&self, name: &str) {
        self.connecting.lock().await.remove(name);
    }

    /// 建立 SSH 连接并完成认证
    ///
    /// 认证优先使用密码，未配置密码时尝试私钥。成功返回 true，失败返回错误信息。
    /// 整个连接+认证过程有 45 秒超时：TCP/KEX 在局域网内瞬时完成，但部分服务器
    /// （如 OpenSSH 7.x 开启 UseDNS 且反向解析超时）认证会卡 10~20 秒，需留足余量；
    /// 目标不可达时 TCP connect 数秒内即失败，不会因放宽而长时间挂起。
    /// 网络过程全程不持有 map 锁：成功后短暂加锁写入会话句柄，失败时移除占位。
    pub async fn connect(&self, info: &SessionInfo) -> Result<bool> {
        let name = info.name.clone();

        let result = tokio::time::timeout(Duration::from_secs(45), async {
            let config = Arc::new(client::Config::default());
            let key_error = Arc::new(Mutex::new(None::<String>));
            let handler = SshHandler {
                host: info.host.clone(),
                port: info.port,
                store: self.known_hosts.clone(),
                key_error: key_error.clone(),
            };
            let mut handle = match client::connect(config, (info.host.as_str(), info.port), handler)
                .await
            {
                Ok(h) => h,
                Err(e) => {
                    // 主机密钥变更等 TOFU 拒绝给出明确原因
                    let key_msg = key_error.lock().await.take();
                    return Err(anyhow!(match key_msg {
                        Some(msg) => msg,
                        None => format!("连接 {}:{} 失败: {}", info.host, info.port, e),
                    }));
                }
            };

            let auth_ok = if let Some(password) = info.password.as_deref() {
                // P26：`enc:` 前缀为 DPAPI 密文，先解密再认证；旧版明文原样使用
                let password = if let Some(rest) =
                    password.strip_prefix(crate::crypto::SECRET_ENCRYPTED_PREFIX)
                {
                    crate::crypto::decrypt_secret(rest)
                        .map_err(|e| anyhow!("会话密码解密失败: {}", e))?
                } else {
                    password.to_string()
                };
                handle
                    .authenticate_password(&info.user, &password)
                    .await
                    .map_err(|e| anyhow!("密码认证失败: {}", e))?
            } else if let Some(key_file) = info.key_file.as_deref() {
                let path = expand_tilde(key_file);
                let key = russh::keys::load_secret_key(&path, None)
                    .map_err(|e| anyhow!("加载私钥 {} 失败: {}", key_file, e))?;
                handle
                    .authenticate_publickey(&info.user, Arc::new(key))
                    .await
                    .map_err(|e| anyhow!("公钥认证失败: {}", e))?
            } else {
                return Err(anyhow!("会话 {} 未配置 password 或 key_file", name));
            };
            Ok::<_, anyhow::Error>((handle, auth_ok))
        })
        .await;

        let (handle, auth_ok) = match result {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => {
                self.sessions.lock().await.remove(&name);
                self.clear_connecting(&name).await;
                return Err(e);
            }
            Err(_) => {
                self.sessions.lock().await.remove(&name);
                self.clear_connecting(&name).await;
                return Err(anyhow!(
                    "连接 {}:{} 超时（45秒），请检查服务器可达性",
                    info.host,
                    info.port
                ));
            }
        };

        if !auth_ok {
            self.sessions.lock().await.remove(&name);
            self.clear_connecting(&name).await;
            return Err(anyhow!("{} 认证失败，请检查用户名/密码或密钥", name));
        }

        // 成功：复用 mark_connecting 的占位会话写入句柄；未占位则新建
        let handle = Arc::new(handle);
        let mut sessions = self.sessions.lock().await;
        match sessions.get_mut(&name) {
            Some(session) => {
                session.lock().await.handle = Some(handle);
            }
            None => {
                sessions.insert(
                    name.clone(),
                    Arc::new(Mutex::new(SshSession {
                        kind: info.kind,
                        handle: Some(handle),
                        shell: None,
                        sftp: None,
                    })),
                );
            }
        }
        // 复用占位会话时同步写入 kind（P32：open_shell/exec 语法按此分支）
        if let Some(session) = sessions.get_mut(&name) {
            session.lock().await.kind = info.kind;
        }
        self.clear_connecting(&name).await;
        Ok(true)
    }

    /// 在已连接的会话上打开交互式 shell（PTY），失败返回错误信息
    ///
    /// 通道打开与 PTY/shell 请求有 10 秒超时，避免服务器无响应时无限挂起。
    /// 通道建立全程不持有 map 锁；读写任务把输出送入队列并 notify 唤醒输出 poller。
    pub async fn open_shell(
        &self,
        name: &str,
        cols: u16,
        rows: u16,
        kind: SessionKind,
    ) -> Result<()> {
        let session = self
            .sessions
            .lock()
            .await
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow!("会话 {} 未连接", name))?;
        let handle = session
            .lock()
            .await
            .handle
            .clone()
            .ok_or_else(|| anyhow!("会话 {} 未连接", name))?;

        let mut channel = match tokio::time::timeout(Duration::from_secs(10), async {
            let channel = handle
                .channel_open_session()
                .await
                .map_err(|e| anyhow!("打开会话通道失败: {}", e))?;
            channel
                .request_pty(true, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
                .await
                .map_err(|e| anyhow!("请求 PTY 失败: {}", e))?;
            channel
                .request_shell(true)
                .await
                .map_err(|e| anyhow!("启动 shell 失败: {}", e))?;
            Ok::<_, anyhow::Error>(channel)
        })
        .await
        {
            Ok(Ok(channel)) => channel,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(anyhow!("打开 shell 超时（10秒）: {}", name)),
        };

        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<ShellCommand>();
        let (data_tx, data_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let notify = self.notify.clone();

        // 独立的双向任务：读取远端输出 → UI；UI 按键/尺寸 → 远端。
        // 每送出一次输出/通道结束都 notify 一次，poller 由此被即时唤醒。
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = channel.wait() => match msg {
                        Some(ChannelMsg::Data { data }) => {
                            if data_tx.send(data.to_vec()).is_ok() {
                                notify.notify_one();
                            }
                        }
                        Some(ChannelMsg::ExtendedData { data, .. }) => {
                            if data_tx.send(data.to_vec()).is_ok() {
                                notify.notify_one();
                            }
                        }
                        // 收到退出状态后仍等 EOF(None)再退出:
                        // 部分 shell 在 ExitStatus 后还会 flush 尾部输出,提前 break 会丢
                        Some(ChannelMsg::ExitStatus { .. }) => {}
                        Some(_) => {}
                        None => break,
                    },
                    cmd = cmd_rx.recv() => match cmd {
                        Some(ShellCommand::Input(bytes)) => {
                            let _ = channel.data(bytes.as_slice()).await;
                        }
                        Some(ShellCommand::Resize(c, r)) => {
                            let _ = channel.window_change(c as u32, r as u32, 0, 0).await;
                        }
                        None => break,
                    }
                }
            }
            let _ = channel.close().await;
            // 通道结束：唤醒 poller 以检测连接意外断开
            notify.notify_one();
        });

        // 仅 Linux 会话注入不可见 CWD 标记(PROMPT_COMMAND → OSC 7 `ESC]7;PWD BEL`)。
        // 每次 bash 重画提示符都发出一次,前端据此获得服务器权威 $PWD(覆盖
        // cd / Tab 补全 / 历史回放 / Ctrl+V 粘贴 / 别名 / 符号链接 / 尾斜杠)。
        // 设置行经 stty -echo 隐藏,随后 clear 清屏,用户几乎无感知。
        // Windows 会话不注入：cmd/PowerShell 不认识 stty/PROMPT_COMMAND,注入只会报错污染终端。
        if kind == SessionKind::Linux {
            let helminj = b"stty -echo; export PROMPT_COMMAND='printf \"\\033]7;helm:%s\\a\" \"$PWD\"'; stty echo; clear\r";
            let _ = cmd_tx.send(ShellCommand::Input(helminj.to_vec()));
        }

        session.lock().await.shell = Some(InteractiveShell { rx: data_rx, tx: cmd_tx });
        Ok(())
    }

    /// 向指定会话的交互 shell 发送按键字节
    pub async fn send_input(&self, name: &str, bytes: &[u8]) {
        let session = self.sessions.lock().await.get(name).cloned();
        if let Some(session) = session {
            if let Some(shell) = session.lock().await.shell.as_ref() {
                let _ = shell.tx.send(ShellCommand::Input(bytes.to_vec()));
            }
        }
    }

    /// 向活跃会话的交互 shell 发送按键字节
    pub async fn send_active_input(&self, bytes: &[u8]) {
        let name = self.active.lock().await.clone();
        if let Some(name) = name {
            self.send_input(&name, bytes).await;
        }
    }

    /// 通知所有交互 shell 终端尺寸变化
    pub async fn send_resize_all(&self, cols: u16, rows: u16) {
        let sessions: Vec<_> = self.sessions.lock().await.values().cloned().collect();
        for session in sessions {
            if let Some(shell) = session.lock().await.shell.as_ref() {
                let _ = shell.tx.send(ShellCommand::Resize(cols, rows));
            }
        }
    }

    /// 取回指定会话 shell 的所有待处理输出（供输出 poller 消费）
    pub async fn drain_output(&self, name: &str) -> Option<Vec<u8>> {
        let session = self.sessions.lock().await.get(name)?.clone();
        let mut session = session.lock().await;
        let shell = session.shell.as_mut()?;
        let mut out = Vec::new();
        while let Ok(bytes) = shell.rx.try_recv() {
            out.extend_from_slice(&bytes);
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// 取回指定会话的连接句柄（Arc 克隆，可在不持有管理器锁的情况下执行命令）
    pub async fn exec_handle(&self, name: &str) -> Option<Arc<client::Handle<SshHandler>>> {
        let session = self.sessions.lock().await.get(name)?.clone();
        let session = session.lock().await;
        session.handle.clone()
    }

    /// 取回指定会话的 SFTP 会话（首次调用时懒建并缓存）
    ///
    /// 经独立子系统通道建立，不经过交互 shell、不污染终端输出。
    /// 建通道在锁外执行；成功后再短暂加锁写入缓存。
    pub async fn sftp_handle(&self, name: &str) -> Option<Arc<SftpSession>> {
        let (session, handle) = {
            let session = self.sessions.lock().await.get(name)?.clone();
            let handle = {
                let guard = session.lock().await;
                if let Some(sftp) = guard.sftp.as_ref() {
                    return Some(sftp.clone());
                }
                guard.handle.clone()?
            };
            (session, handle)
        };
        let channel = handle.channel_open_session().await.ok()?;
        channel.request_subsystem(true, "sftp").await.ok()?;
        let sftp = SftpSession::new(channel.into_stream()).await.ok()?;
        // 大文件上传/下载需要更宽的响应超时（默认 10 秒可能偏紧）
        sftp.set_timeout(60);
        let sftp = Arc::new(sftp);
        // 并发懒建防护:握手在锁外进行,期间另一个调用可能已建好通道;
        // 双检后放弃本次重复通道(随 drop 关闭),避免覆盖泄漏。
        {
            let mut guard = session.lock().await;
            if let Some(existing) = guard.sftp.as_ref() {
                return Some(existing.clone());
            }
            guard.sftp = Some(sftp.clone());
        }
        Some(sftp)
    }

    /// 等待任一会话产生输出或通道结束（Notify 事件，取代 50ms 忙轮询）
    pub async fn wait_output(&self) {
        self.notify.notified().await;
    }

    /// 在已有连接句柄上执行命令，30 秒超时
    ///
    /// 收集 stdout；若 stdout 为空则返回 stderr。
    /// 仅在调用瞬间短暂借用句柄，真正的通道读写不占用管理器锁。
    /// 输出超过 `MAX_EXEC_OUTPUT` 时停止累积（防大输出拖垮内存）。
    #[allow(dead_code)]
    pub async fn execute(&self, name: &str, cmd: &str) -> Result<String> {
        let handle = self
            .exec_handle(name)
            .await
            .ok_or_else(|| anyhow!("会话 {} 未连接", name))?;
        run_exec(handle, cmd).await
    }

/// 断开指定会话
    pub async fn disconnect(&self, name: &str) {
        let handle = {
            let Some(session) = self.sessions.lock().await.get(name).cloned() else {
                return;
            };
            let mut session = session.lock().await;
            session.handle.take()
        };
        if let Some(handle) = handle {
            let _ = handle
                .disconnect(Disconnect::ByApplication, "", "English")
                .await;
        }
        self.sessions.lock().await.remove(name);
        let mut active = self.active.lock().await;
        if active.as_deref() == Some(name) {
            *active = None;
        }
    }

    /// 断开并彻底移除指定会话（编辑/删除会话时使用）
    pub async fn remove(&self, name: &str) {
        self.disconnect(name).await;
        self.sessions.lock().await.remove(name);
    }

    /// 查询指定会话的平台类型（未连接/未知时默认 Linux）
    pub async fn kind_of(&self, name: &str) -> SessionKind {
        let session = self.sessions.lock().await.get(name).cloned();
        let Some(session) = session else {
            return SessionKind::Linux;
        };
        let guard = session.lock().await;
        guard.kind
    }

    /// 查询指定会话的连接状态
    pub async fn get_status(&self, name: &str) -> SessionStatus {
        let session = self.sessions.lock().await.get(name).cloned();
        let Some(session) = session else {
            return SessionStatus::Disconnected;
        };
        let session = session.lock().await;
        match &session.handle {
            Some(handle) if handle.is_closed() => SessionStatus::Disconnected,
            Some(_) => SessionStatus::Connected,
            None => SessionStatus::Connecting,
        }
    }

    /// 返回当前活跃会话名
    pub async fn active_name(&self) -> Option<String> {
        self.active.lock().await.clone()
    }

    /// 返回全部已知会话名（含未连接）
    pub async fn session_names(&self) -> Vec<String> {
        self.sessions.lock().await.keys().cloned().collect()
    }

    /// 设置当前活跃会话
    pub async fn set_active(&self, name: &str) {
        *self.active.lock().await = Some(name.to_string());
    }

    /// 清空当前活跃会话
    pub async fn clear_active(&self) {
        *self.active.lock().await = None;
    }
}

impl Default for SshManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 在已有连接句柄上执行命令，30 秒超时
///
/// 句柄由调用方以 Arc 持有，本函数只在通道建立瞬间借用句柄，
/// 因此不会阻塞其它线程对 SshManager 的访问。
pub async fn run_exec(
    handle: Arc<client::Handle<SshHandler>>,
    cmd: &str,
) -> Result<String> {
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| anyhow!("打开会话通道失败: {}", e))?;
    channel
        .exec(true, cmd)
        .await
        .map_err(|e| anyhow!("发送命令失败: {}", e))?;

    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut total_len = 0usize;

    let read_result = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    let s = String::from_utf8_lossy(&data);
                    stdout.push_str(&s);
                    total_len += s.len();
                    if total_len > MAX_EXEC_OUTPUT {
                        break;
                    }
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    let s = String::from_utf8_lossy(&data);
                    stderr.push_str(&s);
                    total_len += s.len();
                    if total_len > MAX_EXEC_OUTPUT {
                        break;
                    }
                }
                Some(_) => {}
                None => break,
            }
        }
    })
    .await;

    read_result.map_err(|_| anyhow!("命令执行超时（30秒）: {}", cmd))?;

    if stdout.is_empty() {
        Ok(stderr)
    } else {
        Ok(stdout)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SessionInfo;

    
    /// 真实服务器连接测试：连接 → 打开交互 shell → 发命令 → 读输出。
    /// 依赖 192.168.79.150 (root/000000) 在线，离线时跳过。
    fn live_session() -> Option<SessionInfo> {
        let cfg = crate::config::load_config(None).ok()?;
        cfg.sessions.into_iter().next()
    }

    /// 打开 shell 并设为活跃
    async fn open_live(mgr: &SshManager, info: &SessionInfo) {
        mgr.open_shell(&info.name, 80, 24, info.kind)
            .await
            .expect("打开 shell 失败");
        mgr.set_active(&info.name).await;
    }

    /// 读取指定会话远端输出直到包含 marker（最多 15 秒），返回累计输出
    async fn read_until(mgr: &SshManager, name: &str, marker: &str) -> String {
        let mut out = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while tokio::time::Instant::now() < deadline {
            let received = mgr.drain_output(name).await.unwrap_or_default();
            if !received.is_empty() {
                out.push_str(&String::from_utf8_lossy(&received));
                if out.contains(marker) {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        out
    }

    #[tokio::test]
    async fn connect_and_run_commands() {
        let Some(info) = live_session() else {
            eprintln!("跳过：无可用会话配置");
            return;
        };
        let mgr = SshManager::new();
        let ok = mgr.connect(&info).await.expect("连接失败");
        assert!(ok, "认证未通过");
        assert_eq!(mgr.get_status(&info.name).await, SessionStatus::Connected);

        open_live(&mgr, &info).await;
        mgr.send_active_input(b"echo HELM_LIVE_OK\n").await;
        let out = read_until(&mgr, &info.name, "HELM_LIVE_OK").await;
        assert!(out.contains("HELM_LIVE_OK"), "未收到命令回显, 输出: {:?}", out);

        mgr.disconnect(&info.name).await;
        assert_eq!(mgr.get_status(&info.name).await, SessionStatus::Disconnected);
    }

    #[tokio::test]
    async fn ctrl_c_sends_sigint() {
        let Some(info) = live_session() else {
            eprintln!("跳过：无可用会话配置");
            return;
        };
        let mgr = SshManager::new();
        mgr.connect(&info).await.expect("连接失败");
        open_live(&mgr, &info).await;

        mgr.send_active_input(b"echo BEFORE\n").await;
        read_until(&mgr, &info.name, "BEFORE").await;

        // 启动一个长期前台进程
        mgr.send_active_input(b"sleep 30\n").await;
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Ctrl+C 应中断 sleep，shell 回到提示符
        mgr.send_active_input(b"\x03").await;
        tokio::time::sleep(Duration::from_millis(500)).await;

        // 若 SIGINT 生效，后续命令能立即执行
        mgr.send_active_input(b"echo AFTER_SIGINT\n").await;
        let out = read_until(&mgr, &info.name, "AFTER_SIGINT").await;
        assert!(
            out.contains("AFTER_SIGINT"),
            "SIGINT 后 shell 未恢复, 输出: {:?}",
            out
        );

        mgr.disconnect(&info.name).await;
    }

    #[tokio::test]
    async fn pty_resize_propagates() {
        let Some(info) = live_session() else {
            eprintln!("跳过：无可用会话配置");
            return;
        };
        let mgr = SshManager::new();
        mgr.connect(&info).await.expect("连接失败");
        open_live(&mgr, &info).await;

        // 查询初始尺寸
        mgr.send_active_input(b"stty size\n").await;
        read_until(&mgr, &info.name, "80 24").await;

        // 改尺寸后再次查询（stty size 输出为 "行 列"）
        mgr.send_resize_all(40, 10).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        mgr.send_active_input(b"stty size\n").await;
        let out = read_until(&mgr, &info.name, "10 40").await;
        assert!(
            out.contains("10 40"),
            "PTY 尺寸未随 window_change 更新, 输出: {:?}",
            out
        );

        mgr.disconnect(&info.name).await;
    }
}
