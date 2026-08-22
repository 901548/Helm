// ============================================================================
// fs.rs - 远程文件浏览与管理（P26：SFTP 协议重写，替代 exec 通道）
// 经每会话懒建的 SFTP 子系统执行列目录/读/写/改名/删除等操作。
// 不经过交互 shell，不污染终端输出；路径不依赖远端 shell 转义，天然免疫注入。
// 目录导航在核心侧维护每会话 cwd，前端只传相对路径关键字或绝对路径。
// 危险删除（rm -rf / 等）经 safety::check_danger 拦截（语义等价旧 rm -rf --）。
// ============================================================================

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use data_encoding::BASE64;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FilePermissions, FileType, OpenFlags};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::sync::Mutex;

use crate::safety::{check_danger, DangerLevel};
use crate::ssh::SshManager;

// ---------- 类型 ----------

/// 目录项（camelCase 下发前端）
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub perms: String,
    pub mtime: String,
}

/// 单个操作结果
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FsResult {
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

impl FsResult {
    fn ok() -> Self {
        Self { ok: true, message: "OK".to_string(), truncated: None }
    }

    fn err(msg: String) -> Self {
        Self { ok: false, message: msg, truncated: None }
    }
}

/// 规范化路径：空 → "."
fn normalize(path: &str) -> String {
    if path.trim().is_empty() {
        ".".to_string()
    } else {
        path.trim().to_string()
    }
}

/// 取会话的 SFTP 会话（未连接/子系统不可用 → 错误）
async fn sftp_session(ssh: &Arc<SshManager>, name: &str) -> Result<Arc<SftpSession>, String> {
    ssh.sftp_handle(name)
        .await
        .ok_or_else(|| "未连接会话（或远端不支持 SFTP 子系统）".to_string())
}

/// 组装权限串：类型字符 + rwx 九位（与 `ls -la` 首字段形似）
fn perms_string(typ: &FileType, perms: &FilePermissions) -> String {
    let t = match typ {
        FileType::Dir => 'd',
        FileType::File => '-',
        FileType::Symlink => 'l',
        FileType::Other => '?',
    };
    format!("{}{}", t, perms)
}

/// 把 SystemTime 格式化为 UTC "YYYY-MM-DD HH:MM:SS"（零依赖）
fn fmt_mtime(t: Option<SystemTime>) -> String {
    let Some(t) = t else { return String::new() };
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d, hh, mm, ss)
}

/// 儒略日 → (年,月,日)（Howard Hinnant 算法，纯函数可单测）
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

// ---------- 目录列表 ----------

/// 列出目录内容（经 SFTP），并更新该会话 cwd 为权威绝对路径
///
/// `path` 为空或相对路径时相对 SFTP 会话初始目录；返回前用 canonicalize 求绝对路径。
pub async fn list_dir(
    ssh: &Arc<SshManager>,
    cwd_map: &Mutex<HashMap<String, String>>,
    name: &str,
    path: &str,
) -> Result<Vec<FileEntry>, String> {
    let sftp = sftp_session(ssh, name).await?;
    let norm = normalize(path);
    let cwd = sftp
        .canonicalize(&norm)
        .await
        .map_err(|e| format!("进入目录失败: {}", e))?;
    {
        let mut map = cwd_map.lock().await;
        map.insert(name.to_string(), cwd.clone());
    }

    let mut read_dir = sftp
        .read_dir(&cwd)
        .await
        .map_err(|e| format!("列目录失败: {}", e))?;
    let mut entries = Vec::new();
    for entry in read_dir.by_ref() {
        let md = entry.metadata();
        let typ = entry.file_type();
        entries.push(FileEntry {
            name: entry.file_name(),
            is_dir: typ.is_dir(),
            size: md.len(),
            perms: perms_string(&typ, &md.permissions()),
            mtime: fmt_mtime(md.modified().ok()),
        });
    }
    Ok(entries)
}

/// 取会话当前目录（供前端显示面包屑）
///
/// 优先返回 cwd_map 中已跟踪的目录（由 list_dir 更新），
/// 缺失时才 canonicalize(".") 兜底（SFTP 初始目录通常是用户 home）。
pub async fn current_dir(
    ssh: &Arc<SshManager>,
    cwd_map: &Mutex<HashMap<String, String>>,
    name: &str,
) -> Result<String, String> {
    {
        let map = cwd_map.lock().await;
        if let Some(cwd) = map.get(name) {
            if !cwd.is_empty() {
                return Ok(cwd.clone());
            }
        }
    }
    let sftp = sftp_session(ssh, name).await?;
    let cwd = sftp
        .canonicalize(".")
        .await
        .map_err(|e| format!("获取目录失败: {}", e))?;
    {
        let mut map = cwd_map.lock().await;
        map.insert(name.to_string(), cwd.clone());
    }
    Ok(cwd)
}

// ---------- 文件操作 ----------

/// 逐级创建目录（等价旧 `mkdir -p`）：父目录缺失时递归创建，已存在则跳过
async fn create_dir_all(sftp: &SftpSession, path: &str) -> Result<(), String> {
    // 保留输入的绝对/相对语义:相对路径不再被拼成根下绝对路径
    let absolute = path.starts_with('/');
    let norm = path.trim_matches('/');
    if norm.is_empty() {
        return Ok(());
    }
    let mut acc = String::new();
    for part in norm.split('/') {
        if part.is_empty() {
            continue;
        }
        acc = if acc.is_empty() {
            if absolute {
                format!("/{part}")
            } else {
                part.to_string()
            }
        } else {
            format!("{acc}/{part}")
        };
        if !sftp.try_exists(&acc).await.map_err(|e| e.to_string())? {
            sftp.create_dir(&acc)
                .await
                .map_err(|e| format!("新建目录失败: {}", e))?;
        }
    }
    Ok(())
}

/// 新建目录（父目录缺失时递归创建）
pub async fn mkdir(
    ssh: &Arc<SshManager>,
    name: &str,
    path: &str,
) -> Result<FsResult, String> {
    let sftp = sftp_session(ssh, name).await?;
    match create_dir_all(&sftp, path).await {
        Ok(_) => Ok(FsResult::ok()),
        Err(e) => Ok(FsResult::err(e)),
    }
}

/// 重命名/移动
pub async fn rename(
    ssh: &Arc<SshManager>,
    name: &str,
    old_path: &str,
    new_path: &str,
) -> Result<FsResult, String> {
    let sftp = sftp_session(ssh, name).await?;
    match sftp.rename(old_path, new_path).await {
        Ok(_) => Ok(FsResult::ok()),
        Err(e) => Ok(FsResult::err(format!("失败: {}", e))),
    }
}

/// 递归删除（等价旧 `rm -rf --`），危险路径先经安全校验拦截
pub async fn remove(
    ssh: &Arc<SshManager>,
    name: &str,
    path: &str,
) -> Result<FsResult, String> {
    if let Err(e) = assert_removable(path) {
        return Ok(FsResult::err(format!("危险操作已拦截: {}", e)));
    }
    // 与旧 rm -rf -- 语义一致的危险命令拦截
    let cmd = format!("rm -rf -- {}", path);
    let (level, reason) = check_danger(&cmd);
    if level == DangerLevel::Critical {
        return Ok(FsResult::err(format!("危险操作已拦截: {}", reason)));
    }
    let sftp = sftp_session(ssh, name).await?;
    match remove_recursive(&sftp, path.to_string()).await {
        Ok(_) => Ok(FsResult::ok()),
        Err(e) => Ok(FsResult::err(format!("删除失败: {}", e))),
    }
}

/// 递归删除：目录先删子项再删自身；文件/符号链接直接删链接本身（不跟随）。
/// 递归经 Box::pin 打平，避免 async fn 无限自引用。
fn remove_recursive(
    sftp: &Arc<SftpSession>,
    path: String,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>> {
    Box::pin(async move {
        let md = match sftp.symlink_metadata(&path).await {
            Ok(md) => md,
            // 目标已不存在:幂等成功(重复删除/测试起始清理)
            Err(e) if e.to_string().contains("No such file") => return Ok(()),
            Err(e) => return Err(format!("无法读取 {}: {}", path, e)),
        };
        if md.file_type().is_dir() {
            let mut read_dir = sftp.read_dir(&path).await.map_err(|e| e.to_string())?;
            let child_paths: Vec<String> = read_dir.by_ref().map(|e| e.path()).collect();
            for child in child_paths {
                remove_recursive(sftp, child).await?;
            }
            sftp.remove_dir(&path)
                .await
                .map_err(|e| format!("删除目录失败: {}", e))?;
        } else {
            sftp.remove_file(&path)
                .await
                .map_err(|e| format!("删除文件失败: {}", e))?;
        }
        Ok(())
    })
}

/// 校验待删除路径是否危险（根目录/空路径直接拒绝）
fn assert_removable(path: &str) -> Result<(), String> {
    let raw = path.trim();
    if raw.is_empty() || raw == "/" {
        return Err("不允许删除根目录".to_string());
    }
    Ok(())
}

// ---------- 上传/下载/预览（SFTP 直传，无 base64 shell 中转上限） ----------

/// 上传：把 base64 解码后的字节写入远端文件（append=false 截断覆盖，true 追加到尾部）
pub async fn upload(
    ssh: &Arc<SshManager>,
    name: &str,
    path: &str,
    b64: &str,
    append: bool,
) -> Result<FsResult, String> {
    let sftp = sftp_session(ssh, name).await?;
    let data = BASE64
        .decode(b64.trim().as_bytes())
        .map_err(|e| format!("base64 解码失败: {}", e))?;

    let mut file = if append {
        match sftp.open_with_flags(path, OpenFlags::WRITE | OpenFlags::CREATE).await {
            Ok(f) => f,
            Err(e) => return Ok(FsResult::err(format!("打开文件失败: {}", e))),
        }
    } else {
        match sftp.create(path).await {
            Ok(f) => f,
            Err(e) => return Ok(FsResult::err(format!("创建文件失败: {}", e))),
        }
    };
    if append {
        // 追加模式：seek 到现有内容末尾
        let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
        if let Err(e) = file.seek(SeekFrom::Start(len)).await {
            return Ok(FsResult::err(format!("定位失败: {}", e)));
        }
    }
    if let Err(e) = file.write_all(&data).await {
        return Ok(FsResult::err(format!("写入失败: {}", e)));
    }
    if let Err(e) = file.flush().await {
        return Ok(FsResult::err(format!("写入失败: {}", e)));
    }
    Ok(FsResult::ok())
}

/// 下载：读取远端文件整体并返回 base64（前端解码保存）
/// SFTP 下载大小上限:整文件读入内存再 base64 过 IPC,大文件会内存暴涨
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;

pub async fn download(
    ssh: &Arc<SshManager>,
    name: &str,
    path: &str,
) -> Result<FsResult, String> {
    let sftp = sftp_session(ssh, name).await?;
    let file = match sftp.open(path).await {
        Ok(f) => f,
        Err(e) => return Ok(FsResult::err(format!("打开失败: {}", e))),
    };
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    if len > MAX_DOWNLOAD_BYTES {
        return Ok(FsResult::err(format!(
            "文件过大({} 字节,上限 64MB),暂不支持下载",
            len
        )));
    }
    match sftp.read(path).await {
        Ok(data) => Ok(FsResult {
            ok: true,
            message: BASE64.encode(&data),
            truncated: None,
        }),
        Err(e) => Ok(FsResult::err(format!("读取失败: {}", e))),
    }
}

/// 读取文件前 N 字节用于预览；二进制（含 NUL）拒绝，超限标记 truncated
pub async fn read_file(
    ssh: &Arc<SshManager>,
    name: &str,
    path: &str,
    limit: u64,
) -> Result<FsResult, String> {
    let sftp = sftp_session(ssh, name).await?;
    let cap = limit.max(1);
    let mut file = match sftp.open(path).await {
        Ok(f) => f,
        Err(e) => return Ok(FsResult::err(format!("打开失败: {}", e))),
    };
    let mut buf = vec![0u8; (cap as usize).saturating_add(1)];
    // SFTP 分包到达时单次 read 可能短读,循环读满 cap+1 或 EOF
    let mut total = 0usize;
    loop {
        let n = match file.read(&mut buf[total..]).await {
            Ok(n) => n,
            Err(e) => return Ok(FsResult::err(format!("读取失败: {}", e))),
        };
        if n == 0 {
            break;
        }
        total += n;
        if total as u64 > cap {
            break;
        }
    }
    buf.truncate(total);
    if buf.contains(&0) {
        return Ok(FsResult::err("二进制文件，无法预览".to_string()));
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    Ok(FsResult {
        ok: true,
        message: text,
        truncated: Some(total as u64 > cap),
    })
}

// ---------- 测试 ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_path() {
        assert_eq!(normalize(""), ".");
        assert_eq!(normalize("  /tmp  "), "/tmp");
        assert_eq!(normalize("/etc"), "/etc");
    }

    #[test]
    fn remove_rejects_danger() {
        assert!(assert_removable("/").is_err());
        assert!(assert_removable("").is_err());
        assert!(assert_removable("/home").is_ok());
        assert!(assert_removable("tmp/x").is_ok());
    }

    #[test]
    fn perms_string_builds_ls_style() {
        let perms = FilePermissions {
            other_read: true,
            other_write: false,
            other_exec: true,
            group_read: true,
            group_write: false,
            group_exec: false,
            owner_read: true,
            owner_write: true,
            owner_exec: true,
        };
        assert_eq!(perms_string(&FileType::File, &perms), "-rwxr--r-x");
        assert_eq!(perms_string(&FileType::Dir, &perms), "drwxr--r-x");
        assert_eq!(perms_string(&FileType::Symlink, &perms), "lrwxr--r-x");
    }

    #[test]
    fn fmt_mtime_epoch_zero() {
        // 1970-01-01 00:00:00 UTC
        let t = UNIX_EPOCH + std::time::Duration::from_secs(0);
        assert_eq!(fmt_mtime(Some(t)), "1970-01-01 00:00:00");
        assert_eq!(fmt_mtime(None), "");
    }

    #[test]
    fn fmt_mtime_known_date() {
        // 1786791845 = 2026-08-15 11:04:05 UTC（验证日期与 时:分:秒 后缀）
        let secs = 1786791845u64;
        let t = UNIX_EPOCH + std::time::Duration::from_secs(secs);
        let s = fmt_mtime(Some(t));
        assert!(s.starts_with("2026-08-15"), "got: {}", s);
        assert!(s.ends_with(":04:05"), "got: {}", s);
    }

    #[test]
    fn civil_from_days_roundtrip() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2000-02-29 是闰日
        let y2k = civil_from_days(11016);
        assert_eq!(y2k, (2000, 2, 29));
    }

    /// 真实服务器 SFTP 链路验证（只读）：列根目录 → 读 /etc/hostname → 取当前目录。
    /// 依赖 192.168.79.150 在线，离线时跳过。
    fn live_mgr() -> Option<(Arc<SshManager>, String)> {
        let cfg = crate::config::load_config(None).ok()?;
        let info = cfg.sessions.into_iter().next()?;
        let mgr = Arc::new(SshManager::new());
        let name = info.name.clone();
        Some((mgr, name))
    }

    #[tokio::test]
    async fn sftp_live_readonly() {
        let Some((mgr, name)) = live_mgr() else {
            eprintln!("跳过：无可用会话配置");
            return;
        };
        let info = crate::config::load_config(None)
            .ok()
            .and_then(|c| c.sessions.into_iter().next())
            .expect("配置读取失败");
        mgr.connect(&info).await.expect("连接失败");

        let cwd_map = Mutex::new(HashMap::new());
        let entries = list_dir(&mgr, &cwd_map, &name, "/").await.expect("列目录失败");
        assert!(!entries.is_empty(), "/ 目录不应为空");
        assert!(entries.iter().any(|e| e.is_dir), "应包含目录项");
        for e in &entries {
            assert!(!e.name.is_empty(), "目录项名不应为空");
            assert!(!e.perms.is_empty(), "权限串不应为空");
            assert!(!e.mtime.is_empty(), "mtime 不应为空");
        }

        let cwd = current_dir(&mgr, &cwd_map, &name).await.expect("取当前目录失败");
        assert_eq!(cwd, "/", "cwd 应为 /，实际 {}", cwd);

        let preview = read_file(&mgr, &name, "/etc/hostname", 256)
            .await
            .expect("读文件失败");
        assert!(preview.ok, "hostname 预览应成功: {:?}", preview.message);
        assert!(!preview.message.is_empty(), "hostname 不应为空");

        mgr.disconnect(&name).await;
    }

    /// 真实服务器 SFTP 读写链路验证：mkdir → 分块上传(覆盖+追加) → 列目录 →
    /// 预览(含截断/二进制拒绝) → 下载回读 base64 → 改名 → 递归删除 → 断开。
    /// 依赖 192.168.79.150 在线，离线时跳过。
    #[tokio::test]
    async fn sftp_live_readwrite() {
        let Some((mgr, name)) = live_mgr() else {
            eprintln!("跳过：无可用会话配置");
            return;
        };
        let info = crate::config::load_config(None)
            .ok()
            .and_then(|c| c.sessions.into_iter().next())
            .expect("配置读取失败");
        mgr.connect(&info).await.expect("连接失败");
        let cwd_map = Mutex::new(HashMap::new());

        let dir = "/tmp/helm_smoke_test";
        // 清理历史残留，确保从干净状态开始(失败即显式报错,不静默)
        let r0 = remove(&mgr, &name, dir).await.expect("起始清理调用失败");
        assert!(r0.ok, "起始清理失败: {}", r0.message);

        // 1. 递归 mkdir + 幂等重复
        let r = mkdir(&mgr, &name, dir).await.expect("mkdir 调用失败");
        assert!(r.ok, "mkdir 失败: {}", r.message);
        let r = mkdir(&mgr, &name, dir).await.expect("mkdir 重复调用失败");
        assert!(r.ok, "重复 mkdir 应幂等成功: {}", r.message);

        // 2. 上传：覆盖写入 + 追加
        let file = format!("{dir}/hello.txt");
        let r = upload(&mgr, &name, &file, &BASE64.encode(b"Hello Helm"), false)
            .await
            .expect("upload 调用失败");
        assert!(r.ok, "上传失败: {}", r.message);
        let r = upload(&mgr, &name, &file, &BASE64.encode(b", SFTP!"), true)
            .await
            .expect("append 调用失败");
        assert!(r.ok, "追加失败: {}", r.message);

        // 3. 列目录验证
        let entries = list_dir(&mgr, &cwd_map, &name, dir).await.expect("列目录失败");
        assert_eq!(entries.len(), 1, "目录应恰好 1 项: {:?}", entries);
        assert_eq!(entries[0].name, "hello.txt");

        // 4. 预览：完整读取（不截断）
        let pv = read_file(&mgr, &name, &file, 200).await.expect("预览调用失败");
        assert!(pv.ok, "预览失败: {}", pv.message);
        assert_eq!(pv.message, "Hello Helm, SFTP!");
        assert_eq!(pv.truncated, Some(false));

        // 5. 预览截断：limit 小于文件长
        let pv = read_file(&mgr, &name, &file, 6).await.expect("截断预览调用失败");
        assert!(pv.ok, "截断预览失败: {}", pv.message);
        assert_eq!(pv.truncated, Some(true));

        // 6. 二进制拒绝
        let bin = format!("{dir}/blob.bin");
        let r = upload(&mgr, &name, &bin, &BASE64.encode(&[0u8, 1, 2, 3]), false)
            .await
            .expect("二进制上传调用失败");
        assert!(r.ok, "二进制上传失败: {}", r.message);
        let pv = read_file(&mgr, &name, &bin, 10).await.expect("二进制预览调用失败");
        assert!(!pv.ok && pv.message.contains("二进制"), "应拒绝二进制: {}", pv.message);

        // 7. 下载 base64 回读
        let dl = download(&mgr, &name, &file).await.expect("下载调用失败");
        assert!(dl.ok, "下载失败: {}", dl.message);
        assert_eq!(
            BASE64.decode(dl.message.trim().as_bytes()).unwrap(),
            b"Hello Helm, SFTP!"
        );

        // 8. 改名
        let renamed = format!("{dir}/renamed.txt");
        let r = rename(&mgr, &name, &file, &renamed).await.expect("rename 调用失败");
        assert!(r.ok, "改名失败: {}", r.message);

        // 9. 递归删除目录（含子文件）
        let r = remove(&mgr, &name, dir).await.expect("remove 调用失败");
        assert!(r.ok, "删除失败: {}", r.message);

        // 10. 验证已删除
        let entries = list_dir(&mgr, &cwd_map, &name, "/tmp").await.expect("列 /tmp 失败");
        assert!(
            !entries.iter().any(|e| e.name == "helm_smoke_test"),
            "smoke 目录应已删除"
        );

        mgr.disconnect(&name).await;
    }
}
