// ============================================================================
// snapshot.rs - 服务器配置快照（P127：探测 / 打包（全量 + 变化包）/ 快照链 / 单文件取回）
//
// 命题：**让不可逆命令变可逆**。服务器拍不了虚拟机快照（缺块层抽象，见
// docs/P127-服务器快照-设计方案.md §1），但破坏是**文件级的、有界的**，
// 故文件级 tar 打包足以覆盖"手滑改错配置 / 误删"这一类事故。
//
// 本文件范围：
//   - 一期：探测 / 手动全量打包 / 列表 / 删除
//   - 二期：命令前拦截开关（拦截状态机在 intercept.rs）
//   - 三期：滚动基线的判定在 baseline.rs（打包走本文件的 capture_auto）
//   - 四期（本批）：**变化包（--newer-mtime 增量）+ 快照链（index.json 缓存）
//     + 单文件取回/就地还原 + 链级保留**
// 还原（全量 + 依次解变化包）在 restore.rs；见设计稿 §6.3/§8。
//
// 实测依据（2026-10-13，CentOS 7.9 测试机）：
//   - LVM 快照不可用（vg_free=0，且 XFS 不能缩容腾空间）
//   - reflink 不可用（CentOS 7 的 XFS 默认 reflink=0）
//   - tar 是唯一可行机制；/etc 仅 36M/1732 文件，打包 <1 秒
//   - /tmp 会被 systemd-tmpfiles 10 天清理 → **落点不能用 /tmp**
// ============================================================================

use anyhow::{anyhow, Result};
use data_encoding::BASE64;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::ssh::SshManager;
use crate::task_exec::{run_task_exec, run_task_exec_with_limit};

/// 默认落点（相对 $HOME）。**不能用 /tmp** —— systemd-tmpfiles 默认 10 天清理，
/// 快照会静默消失（用户以为有退路其实没有，比没有快照更危险）。
pub const DEFAULT_DIR: &str = ".helm-snapshots";

// ---------- P127 二期：命令前拦截开关 ----------
//
// 用全局原子而非 CoreState 字段，理由：`send_input` 是**热路径**（每次按键都调），
// 从 config 锁里读会引入不必要的锁竞争；且该开关是进程级策略，无会话维度。
//
// **默认关闭**（与设计稿 §9 一致）：这是会写服务器磁盘、且会拦用户输入的功能，
// 必须显式开启。
static INTERCEPT_ON: AtomicBool = AtomicBool::new(false);

/// 是否启用命令前拦截
pub fn intercept_enabled() -> bool {
    INTERCEPT_ON.load(Ordering::Relaxed)
}

/// 设置命令前拦截开关
pub fn set_intercept_enabled(on: bool) {
    INTERCEPT_ON.store(on, Ordering::Relaxed);
}

/// 敏感文件排除清单（设计稿 §3.3）。
///
/// **关键洞察**：最危险的那几个文件，恰恰是**不想还原**的 ——
/// 还原 `ssh_host_*_key` 会换掉主机密钥（所有客户端报 host key changed），
/// 还原 `shadow` 会回滚密码哈希。而 `/etc/nginx/*.conf` 这类**配置**才是要还原的。
///
/// 故"打包时就排除"这一个动作同时提升安全性与可用性：
/// 包内无密钥 → 无需加密 → 还原时也不会误换主机密钥。
const EXCLUDES: &[&str] = &[
    "etc/shadow",
    "etc/shadow-",
    "etc/gshadow",
    "etc/gshadow-",
    "etc/ssh/ssh_host_*",
    "root/.ssh/id_*",
    "home/*/.ssh/id_*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "etc/ssl/private/*",
    "etc/pki/private/*",
    // 快照目录自身：$HOME 常在打包范围（/root、/home）内，而 index.json 每次打包
    // 都新写入 —— 不排除的话变化包会把自己的索引/旧 tar 卷进来（套娃）。
    // 两条模式覆盖"打包范围内直接可见"与"任意嵌套深度"两种成员形态。
    ".helm-snapshots",
    "*/.helm-snapshots",
];

/// 探测候选路径（设计稿 §6.4：配置密集区，跳过 /proc /sys /dev /var/lib 等）
const CANDIDATE_PATHS: &[&str] = &["/etc", "/root", "/home", "/usr/local", "/opt"];

/// 探测超时（秒）。大机器上 du 扫 /home 可能几十秒 → 超时即判"过大"不纳入。
const PROBE_TIMEOUT_SECS: u64 = 8;
/// 打包超时（秒）。实测 /etc（1732 文件）<1 秒；20000 小文件需 9.5 秒。
const PACK_TIMEOUT_SECS: u64 = 300;
/// 单次快照体积上限（MB）。超过则拒绝 —— 见设计稿 §7.1。
const MAX_SIZE_MB: u64 = 500;
/// 安全系数：可用空间须 > 预估体积 × 此值，否则拒绝打包（设计稿 §7.1）
const SPACE_SAFETY_FACTOR: f64 = 3.0;
/// 链级保留份数（保留最新的 N 条链；每链 = 1 全量 + 最多 MAX_CHG_PER_CHAIN 变化包）
pub const KEEP: usize = 10;
/// 每链最多变化包数。满则开新链 —— 还原代价随链长增长，链越长失链代价也越大。
pub const MAX_CHG_PER_CHAIN: usize = 10;
/// 链索引缓存文件名（快照目录内）
const INDEX_FILE: &str = "index.json";
/// index.json 结构版本
const INDEX_VERSION: u32 = 1;
/// 单文件取回上限（原始字节）。超过只回传前 16MiB 并标记 `truncated`。
pub const MAX_FETCH_BYTES: usize = 16 * 1024 * 1024;
/// fetch 命令输出保留上限：base64(16MiB+1) ≈ 21.34MiB，留余量。
/// （task_exec 默认 64KiB 是给 Agent 步骤的，这里必须显式放大）
const FETCH_RETAIN_CAP: usize = 24 * 1024 * 1024;
/// 成员列表单包输出保留上限（大目录 20k 文件 ≈ 1MB）
const MEMBERS_RETAIN_CAP: usize = 8 * 1024 * 1024;
/// 取回 / 成员列表 / 单文件还原等文件操作的远端命令超时（秒）
const FILE_OP_TIMEOUT_SECS: u64 = 120;

// ---------- 数据结构 ----------

/// 快照包类型：全量 / 变化包（P127 四期）。
/// 一期遗留包无 `-chg` 后缀 —— 文件名后缀判定即视为 Full（链构建时按时间归链）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotKind {
    #[default]
    Full,
    Chg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotEntry {
    /// 文件名（不含目录）
    pub file: String,
    /// 完整路径
    pub path: String,
    /// 打包时间（Unix 秒）
    pub created_at: u64,
    /// 体积（字节）
    pub size: u64,
    /// 已校验通过（tar -t 成功）。**残包不会出现在列表里**。
    pub verified: bool,
    /// 打包的路径列表
    pub paths: Vec<String>,
    /// 包类型：全量 / 变化包（P127 四期）
    #[serde(default)]
    pub kind: SnapshotKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    /// 落点所在分区可用空间（MB）
    pub disk_avail_mb: u64,
    /// 各候选路径体积（MB）；超时的路径不在列表里（视为过大）
    pub path_sizes: Vec<(String, u64)>,
    /// 推荐打包的路径（由体积 + 可用空间推出）
    pub recommended: Vec<String>,
    /// 推荐理由（给用户看的结论，不是选择题）
    pub reason: String,
    /// 是否检测到 LVM 剩余空间（可提示"支持块级快照"）
    pub lvm_free_mb: u64,
    /// 是否支持 reflink
    pub reflink: bool,
    /// 机器规模（内存 MB）
    pub mem_total_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackResult {
    pub file: String,
    pub path: String,
    pub size: u64,
    pub verified: bool,
    /// 打包耗时（毫秒）
    pub elapsed_ms: u64,
    pub paths: Vec<String>,
    /// 本次产出的包类型（P127 四期）：全量 / 变化包
    #[serde(default)]
    pub kind: SnapshotKind,
}

/// 快照链（P127 四期）：一条链 = 1 个全量包 + 至多 `MAX_CHG_PER_CHAIN` 个变化包。
///
/// 还原 = 解链首全量 + 按时间依次解变化包（restore.rs）。因而**删链首全量必须
/// 连带删整链变化包**（§7.3）—— 否剩下一堆无全量的孤儿变化包，既不可还原又误导用户。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotChain {
    /// 链序号（1 起，按时间正序；仅作展示用）
    pub id: u64,
    /// 链首全量包
    pub full: SnapshotEntry,
    /// 变化包（按时间正序）
    pub changes: Vec<SnapshotEntry>,
}

/// 单文件取回结果（P127 四期）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchResult {
    /// 文件内容（base64；被截断时为前 `MAX_FETCH_BYTES` 字节）
    pub content_b64: String,
    /// 是否因超上限被截断
    pub truncated: bool,
    /// 实际回传的原始字节数
    pub size: u64,
}

/// 单文件就地还原结果（P127 四期）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRestoreResult {
    /// 还原目标（绝对路径 = "/" + 成员路径）
    pub target: String,
    /// 覆盖前的备份路径（目标原本不存在时为空）
    pub backup: Option<String>,
    /// 还原文件字节数
    pub size: u64,
}

/// 包内文件项（浏览 / 单文件取回用）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberEntry {
    /// 包内路径（相对 `/`）
    pub path: String,
    /// 文件大小（字节）
    pub size: u64,
}

/// index.json 内容：包名 → 打包路径列表的缓存。tar 包本身不记"打包范围"，
/// 链构建的范围变更检测 / UI 展示 / 单文件浏览都要它。损坏/缺失 → 视为空
/// （功能降级，不报错）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct IndexData {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: Vec<IndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexEntry {
    file: String,
    #[serde(default)]
    paths: Vec<String>,
}

// ---------- 纯函数（可单测） ----------

/// 生成快照文件名：`snap-<主机名>-<UTC 时间戳>.tar.gz`
///
/// 主机名做净化：只留 `[A-Za-z0-9._-]`，其余替换为 `_`
/// （防主机名含 `/` 或空格导致路径穿越/命令注入）。
pub fn snapshot_filename(host: &str, ts: u64) -> String {
    let safe: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    // `..` 必须额外消除:点号在白名单里,故 `../../etc` 净化后仍含 `..`
    // （单测抓到 —— 路径穿越风险,不能只靠字符白名单）
    let safe = safe.replace("..", "_");
    // 前导 `.` 会产出隐藏文件,且 `-` 开头会被 tar 当选项,一并剥掉
    let safe = safe.trim_start_matches(['.', '-']).to_string();
    let safe = if safe.is_empty() { "host".to_string() } else { safe };
    format!("snap-{}-{}.tar.gz", safe, ts)
}

/// 由包文件名判定包类型：`snap-<host>-<ts>-chg.tar.gz` → 变化包，其余 → 全量包。
///
/// 一期遗留包无 `-chg` 后缀，天然落在 Full —— 链构建时按时间归链。
pub fn kind_from_filename(file: &str) -> SnapshotKind {
    let stem = file.strip_suffix(".tar.gz").unwrap_or(file);
    if stem.ends_with("-chg") {
        SnapshotKind::Chg
    } else {
        SnapshotKind::Full
    }
}

/// 快照文件名守卫：只接受 `snap-` 前缀的纯文件名（防路径穿越/选项注入）。
fn check_snapshot_file(file: &str) -> Result<()> {
    if file.is_empty()
        || file.contains('/')
        || file.contains("..")
        || !file.starts_with("snap-")
    {
        return Err(anyhow!("非法的快照文件名"));
    }
    Ok(())
}

/// tar 成员路径守卫（单文件取回/还原的 tar 操作数）。
/// 成员来自 tar 列表输出，但仍是远端字节 —— 同样防注入/穿越。
fn guard_member(member: &str) -> Result<()> {
    if member.is_empty()
        || member.starts_with('/')
        || member.starts_with('-')
        || member.ends_with('/')
        || member.contains('\0')
    {
        return Err(anyhow!("非法的快照成员路径"));
    }
    if member.split('/').any(|c| c == ".." || c == ".") {
        return Err(anyhow!("非法的快照成员路径"));
    }
    Ok(())
}

/// 把包列表（任意顺序）归成快照链：时间正序扫描，每个全量包开一条新链，
/// 变化包归入其之前最近的链；链前无全量的孤儿变化包丢弃（正常流程不会产生）。
///
/// 返回**最新链在前**（UI 直接顺序渲染）。同一秒内全量先于变化包
/// （否则同秒的 chg 会被丢进上一条链或成孤儿）。
pub fn build_chains(entries: &[SnapshotEntry]) -> Vec<SnapshotChain> {
    let mut sorted: Vec<&SnapshotEntry> = entries.iter().collect();
    sorted.sort_by_key(|e| {
        (e.created_at, if e.kind == SnapshotKind::Chg { 1 } else { 0 })
    });
    let mut chains: Vec<SnapshotChain> = Vec::new();
    for e in sorted {
        if e.kind == SnapshotKind::Full {
            chains.push(SnapshotChain {
                id: 0,
                full: e.clone(),
                changes: Vec::new(),
            });
        } else if let Some(last) = chains.last_mut() {
            last.changes.push(e.clone());
        }
    }
    let mut out: Vec<SnapshotChain> = chains
        .into_iter()
        .enumerate()
        .map(|(i, mut c)| {
            c.id = i as u64 + 1;
            c
        })
        .collect();
    out.reverse();
    out
}

/// 把（file → paths）并入索引（同 file 覆盖），并刷新结构版本。
fn upsert_index_entry(data: &mut IndexData, file: &str, paths: &[String]) {
    data.version = INDEX_VERSION;
    if let Some(e) = data.entries.iter_mut().find(|e| e.file == file) {
        e.paths = paths.to_vec();
    } else {
        data.entries.push(IndexEntry {
            file: file.to_string(),
            paths: paths.to_vec(),
        });
    }
}

/// 生成变化包文件名：`snap-<主机名>-<UTC 时间戳>-chg.tar.gz`
/// （`-chg` 后缀即 `kind_from_filename` 的判定依据）。
pub fn chg_filename(host: &str, ts: u64) -> String {
    let base = snapshot_filename(host, ts);
    let stem = base.strip_suffix(".tar.gz").unwrap_or(&base);
    format!("{}-chg.tar.gz", stem)
}

/// 组装 tar 命令（**全部路径经单引号转义，绝不拼接用户原文**）。
///
/// 输出到 stdout 之外用 `-f`，`-C /` 让归档内路径为相对路径
/// （这样还原时 `tar -xf 包 -C /` 即可，无需处理绝对路径警告）。
pub fn build_tar_cmd(dir: &str, file: &str, paths: &[String]) -> String {
    build_tar_cmd_inner(dir, file, paths, None)
}

/// 组装**变化包** tar 命令：加 `--newer-mtime=@<起点时间戳>`（只收比链上一包
/// 更新的文件）。GNU tar 语义，2026-10-17 本地实测可用（GNU tar 1.35）。
pub fn build_chg_tar_cmd(dir: &str, file: &str, paths: &[String], since_ts: u64) -> String {
    build_tar_cmd_inner(dir, file, paths, Some(since_ts))
}

fn build_tar_cmd_inner(dir: &str, file: &str, paths: &[String], newer_than: Option<u64>) -> String {
    let mut cmd = String::from("tar -czf ");
    cmd.push_str(&shq(&format!("{}/{}", dir, file)));
    if let Some(ts) = newer_than {
        cmd.push_str(&format!(" --newer-mtime=@{}", ts));
    }
    // 排除敏感文件（tar 的 --exclude 对 -C 后的相对路径生效）
    for e in EXCLUDES {
        cmd.push_str(" --exclude=");
        cmd.push_str(&shq(e));
    }
    cmd.push_str(" -C /");
    for p in paths {
        // 去掉前导 /（-C / 后应给相对路径）
        let rel = p.trim_start_matches('/');
        if rel.is_empty() {
            continue;
        }
        cmd.push(' ');
        cmd.push_str(&shq(rel));
    }
    cmd
}

/// POSIX 单引号转义：`'` → `'\''`
pub fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 从 `df -Pm <dir>` 输出解析可用空间（MB）。
///
/// 取**最后一行**（跳过表头）；字段：Filesystem 1024-blocks Used Available Capacity Mounted
pub fn parse_df_avail_mb(out: &str) -> Option<u64> {
    // 从后往前找第一个非空且非表头的行（`df` 的挂载信息在最后一行）
    out.lines()
        .rev()
        .find(|l| !l.trim().is_empty() && !l.starts_with("Filesystem"))
        .and_then(|l| l.split_whitespace().nth(3))
        .and_then(|v| v.parse::<u64>().ok())
}

/// 从 `du -sm <path>` 输出解析体积（MB）。`du` 失败/超时时返回 None。
pub fn parse_du_mb(out: &str) -> Option<u64> {
    out.lines()
        .next()
        .and_then(|l| l.split_whitespace().next())
        .and_then(|v| v.parse::<u64>().ok())
}

/// 由探测结果推出推荐打包路径（设计稿 §4.8：**不让用户选档位**）。
///
/// 规则：
///   - 单路径体积 > 上限 → 不纳入（大机器自动排除 /home 这类数据目录）
///   - 累计体积 > 可用空间 / 安全系数 → 停止纳入
///   - `/etc` 永远优先（配置才是"改错了要还原"的东西）
pub fn recommend_paths(
    sizes: &[(String, u64)],
    disk_avail_mb: u64,
    max_size_mb: u64,
) -> (Vec<String>, String) {
    let budget = ((disk_avail_mb as f64) / SPACE_SAFETY_FACTOR).floor() as u64;
    let mut picked: Vec<String> = Vec::new();
    let mut total: u64 = 0;
    let mut skipped: Vec<String> = Vec::new();

    // /etc 优先
    let mut ordered: Vec<&(String, u64)> = sizes.iter().collect();
    ordered.sort_by_key(|(p, _)| if p == "/etc" { 0 } else { 1 });

    for (p, mb) in ordered {
        if *mb > max_size_mb {
            skipped.push(format!("{}（{}MB 超上限）", p, mb));
            continue;
        }
        if total + mb > budget {
            skipped.push(format!("{}（累计超预算）", p));
            continue;
        }
        picked.push(p.clone());
        total += mb;
    }

    let reason = if picked.is_empty() {
        "磁盘可用空间不足，无法打包任何路径。请清理空间后重试。".to_string()
    } else if skipped.is_empty() {
        format!(
            "已按本机情况配置：快照 {}（约 {}MB/次），保留 {} 份。磁盘可用 {}MB。",
            picked.join(" "),
            total,
            KEEP,
            disk_avail_mb
        )
    } else {
        format!(
            "已按本机情况配置：快照 {}（约 {}MB/次），保留 {} 份。磁盘可用 {}MB。已跳过：{}",
            picked.join(" "),
            total,
            KEEP,
            disk_avail_mb,
            skipped.join("、")
        )
    };
    (picked, reason)
}

/// 校验：可用空间是否够打包（设计稿 §7.1）
///
/// **绝不"尽力而为"地硬上** —— 磁盘写满的连带损害（日志写不进、数据库无法提交）
/// 远大于"这次没拍成"。
pub fn space_check(disk_avail_mb: u64, est_mb: u64) -> Result<()> {
    let need = ((est_mb as f64) * SPACE_SAFETY_FACTOR).ceil() as u64;
    if disk_avail_mb < need {
        return Err(anyhow!(
            "可用空间不足：落点分区剩 {}MB，待打包约 {}MB，安全余量需 {}MB。请清理空间或缩小范围。",
            disk_avail_mb,
            est_mb,
            need
        ));
    }
    Ok(())
}

/// 校验：是否超单次体积上限
pub fn size_check(est_mb: u64) -> Result<()> {
    if est_mb > MAX_SIZE_MB {
        return Err(anyhow!(
            "待打包体积约 {}MB，超过单次上限 {}MB。请缩小范围。",
            est_mb,
            MAX_SIZE_MB
        ));
    }
    Ok(())
}

// ---------- 执行层 ----------

/// 在会话上执行一条命令并取回 (stdout, exit_code)
async fn run(ssh: &Arc<SshManager>, name: &str, cmd: &str, timeout: u64) -> Result<(String, i32)> {
    let handle = ssh
        .exec_handle(name)
        .await
        .ok_or_else(|| anyhow!("会话 {} 未连接", name))?;
    let kind = ssh.kind_of(name).await;
    let r = run_task_exec(handle, cmd, "/", timeout, kind, None).await?;
    Ok((r.output, r.exit_code))
}

/// 单次命令的输出上限（`run_task_exec_with_limit` 的 retain_cap）。
/// 单文件取回的内容经 base64 回传（+33%），24MiB 上限覆盖 `MAX_FETCH_BYTES`
/// 16MiB 的最坏情况；列成员、写索引等普通命令远小于此。
const CAPPED_OUTPUT: usize = FETCH_RETAIN_CAP;

/// 同 `run`，但显式给出输出保留上限（取回 base64 内容必须用）。
async fn run_capped(
    ssh: &Arc<SshManager>,
    name: &str,
    cmd: &str,
    timeout: u64,
    retain_cap: usize,
) -> Result<(String, i32)> {
    let handle = ssh
        .exec_handle(name)
        .await
        .ok_or_else(|| anyhow!("会话 {} 未连接", name))?;
    let kind = ssh.kind_of(name).await;
    let r = run_task_exec_with_limit(handle, cmd, "/", timeout, kind, None, retain_cap).await?;
    Ok((r.output, r.exit_code))
}

/// 快照落点目录（`$HOME/.helm-snapshots`）。
async fn snapshot_dir(ssh: &Arc<SshManager>, name: &str) -> Result<String> {
    let (home_out, _) = run(ssh, name, "printf %s \"$HOME\"", PROBE_TIMEOUT_SECS).await?;
    Ok(format!(
        "{}/{}",
        home_out.trim().trim_end_matches('/'),
        DEFAULT_DIR
    ))
}

/// 读 index.json（缺失/损坏/版本不符 → 空索引：功能降级，不报错）。
async fn read_index(ssh: &Arc<SshManager>, name: &str) -> IndexData {
    let Ok(dir) = snapshot_dir(ssh, name).await else {
        return IndexData::default();
    };
    let cmd = format!("cat {} 2>/dev/null", shq(&format!("{}/{}", dir, INDEX_FILE)));
    let Ok((out, code)) = run(ssh, name, &cmd, PROBE_TIMEOUT_SECS).await else {
        return IndexData::default();
    };
    if code != 0 || out.trim().is_empty() {
        return IndexData::default();
    }
    match serde_json::from_str::<IndexData>(out.trim()) {
        Ok(d) if d.version == INDEX_VERSION => d,
        _ => IndexData::default(),
    }
}

/// 把索引写回 index.json（内容经 base64 落盘，JSON 里不可能含 shell 元字符）。
/// 失败只记日志不中断 —— 索引缺失只影响范围展示/链判定，快照本体不受影响。
async fn write_index(ssh: &Arc<SshManager>, name: &str, data: &IndexData) {
    let Ok(dir) = snapshot_dir(ssh, name).await else {
        return;
    };
    let Ok(json) = serde_json::to_string(data) else {
        return;
    };
    let cmd = format!(
        "printf %s {} | base64 -d > {}",
        shq(&BASE64.encode(json.as_bytes())),
        shq(&format!("{}/{}", dir, INDEX_FILE))
    );
    if let Err(e) = run(ssh, name, &cmd, PROBE_TIMEOUT_SECS).await {
        eprintln!("helm: 快照索引写入失败（{name}）：{e}");
    }
}

/// 探测：磁盘可用 + 各候选路径体积 + 块级能力（LVM/reflink）
pub async fn probe(ssh: &Arc<SshManager>, name: &str) -> Result<ProbeResult> {
    // 1) 落点分区可用空间
    let home = run(ssh, name, "printf %s \"$HOME\"", PROBE_TIMEOUT_SECS)
        .await
        .map(|(o, _)| o.trim().to_string())
        .unwrap_or_else(|_| "/root".to_string());
    let dir = format!("{}/{}", home.trim_end_matches('/'), DEFAULT_DIR);
    let (df_out, _) = run(
        ssh,
        name,
        &format!("mkdir -p {} && df -Pm {}", shq(&dir), shq(&dir)),
        PROBE_TIMEOUT_SECS,
    )
    .await?;
    let disk_avail_mb = parse_df_avail_mb(&df_out).unwrap_or(0);

    // 2) 各候选路径体积（逐条带超时；超时即判"过大"不纳入）
    let mut path_sizes: Vec<(String, u64)> = Vec::new();
    for p in CANDIDATE_PATHS {
        let cmd = format!("du -sm {} 2>/dev/null", shq(p));
        // 超时 / 不存在 / 无权限 → 一律跳过（视为过大，不纳入）
        if let Ok((o, 0)) = run(ssh, name, &cmd, PROBE_TIMEOUT_SECS).await {
            if let Some(mb) = parse_du_mb(&o) {
                path_sizes.push(((*p).to_string(), mb));
            }
        }
    }

    // 3) 块级能力（仅作提示，不实现 —— 设计稿 §12）
    let (lvm_out, _) = run(
        ssh,
        name,
        "vgs --noheadings -o vg_free --units m 2>/dev/null | tr -d ' m' | head -n1",
        PROBE_TIMEOUT_SECS,
    )
    .await
    .unwrap_or_default();
    let lvm_free_mb = lvm_out.trim().parse::<u64>().unwrap_or(0);

    let (rf_out, rf_code) = run(
        ssh,
        name,
        "d=$(mktemp -d); echo x > $d/a; cp --reflink=always $d/a $d/b 2>/dev/null && echo YES || echo NO; rm -rf $d",
        PROBE_TIMEOUT_SECS,
    )
    .await
    .unwrap_or_default();
    let reflink = rf_code == 0 && rf_out.contains("YES");

    // 4) 内存（规模判定）
    let (mem_out, _) = run(
        ssh,
        name,
        "awk '/MemTotal/{print int($2/1024)}' /proc/meminfo",
        PROBE_TIMEOUT_SECS,
    )
    .await
    .unwrap_or_default();
    let mem_total_mb = mem_out.trim().parse::<u64>().unwrap_or(0);

    let (recommended, reason) = recommend_paths(&path_sizes, disk_avail_mb, MAX_SIZE_MB);

    Ok(ProbeResult {
        disk_avail_mb,
        path_sizes,
        recommended,
        reason,
        lvm_free_mb,
        reflink,
        mem_total_mb,
    })
}

/// 预估打包体积：推荐路径的体积之和。
pub fn estimate_mb(pr: &ProbeResult) -> u64 {
    pr.path_sizes
        .iter()
        .filter(|(p, _)| pr.recommended.contains(p))
        .map(|(_, mb)| *mb)
        .sum()
}

/// 打包快照（全量；供「手动拍快照」与「还原前二次快照」复用）。
///
/// 顺序（设计稿 §7.1，**不可颠倒**）：
///   1. 空间检查（不够即拒绝，绝不硬上）
///   2. tar 打包
///   3. **校验**（tar -t 成功才算数；失败即删残包）
///   4. 校验通过才写 index.json（§14.1）
///
/// **残包比没有包更危险** —— 用户以为有退路，还原时才发现是坏的。
pub async fn pack(
    ssh: &Arc<SshManager>,
    name: &str,
    paths: &[String],
    disk_avail_mb: u64,
    est_mb: u64,
) -> Result<PackResult> {
    pack_impl(ssh, name, paths, disk_avail_mb, est_mb, SnapshotKind::Full, None).await
}

/// 自动快照（滚动基线用）：首拍 / 链满 / 范围变化 → 全量；否则变化包（§6.3）。
///
/// 「范围变化」依据 index.json 记录的上一包范围判定 —— 检测不了就保守打全量
/// （全量永远可单独还原；把范围对不上的变化包接进链才是危险）。
pub async fn capture_auto(ssh: &Arc<SshManager>, name: &str) -> Result<PackResult> {
    let pr = probe(ssh, name).await?;
    if pr.recommended.is_empty() {
        return Err(anyhow!("本机没有适合打包的路径"));
    }
    let est = estimate_mb(&pr);

    let entries = list(ssh, name).await.unwrap_or_default();
    let chains = build_chains(&entries);
    let idx = read_index(ssh, name).await;

    if let Some(chain) = chains.first() {
        let (prev_file, prev_ts) = chain
            .changes
            .last()
            .map(|e| (e.file.as_str(), e.created_at))
            .unwrap_or((chain.full.file.as_str(), chain.full.created_at));
        let scope_ok = idx
            .entries
            .iter()
            .find(|e| e.file == prev_file)
            .map(|e| e.paths == pr.recommended)
            .unwrap_or(false);
        if scope_ok && chain.changes.len() < MAX_CHG_PER_CHAIN {
            return pack_impl(
                ssh,
                name,
                &pr.recommended,
                pr.disk_avail_mb,
                est,
                SnapshotKind::Chg,
                Some(prev_ts),
            )
            .await;
        }
    }
    pack_impl(ssh, name, &pr.recommended, pr.disk_avail_mb, est, SnapshotKind::Full, None).await
}

/// 打包核心（全量 / 变化包共用）。`since_ts` 仅变化包使用（newer-mtime 起点）。
async fn pack_impl(
    ssh: &Arc<SshManager>,
    name: &str,
    paths: &[String],
    disk_avail_mb: u64,
    est_mb: u64,
    kind: SnapshotKind,
    since_ts: Option<u64>,
) -> Result<PackResult> {
    if paths.is_empty() {
        return Err(anyhow!("没有要打包的路径"));
    }
    size_check(est_mb)?;
    space_check(disk_avail_mb, est_mb)?;

    // 主机名 + 时间戳
    let (host_out, _) = run(ssh, name, "hostname 2>/dev/null || echo host", PROBE_TIMEOUT_SECS)
        .await
        .unwrap_or_else(|_| ("host".to_string(), 0));
    let host = host_out.trim().to_string();
    let (ts_out, _) = run(ssh, name, "date -u +%s", PROBE_TIMEOUT_SECS).await?;
    let ts: u64 = ts_out.trim().parse().unwrap_or(0);

    let (home_out, _) = run(ssh, name, "printf %s \"$HOME\"", PROBE_TIMEOUT_SECS).await?;
    let dir = format!("{}/{}", home_out.trim().trim_end_matches('/'), DEFAULT_DIR);
    let file = match since_ts {
        Some(_) => chg_filename(&host, ts),
        None => snapshot_filename(&host, ts),
    };
    let archive = format!("{}/{}", dir, file);

    // 打包
    let tar_cmd = match since_ts {
        Some(s) => build_chg_tar_cmd(&dir, &file, paths, s),
        None => build_tar_cmd(&dir, &file, paths),
    };
    let cmd = format!(
        "mkdir -p {} && {} && ls -l {} | awk '{{print $5}}'",
        shq(&dir),
        tar_cmd,
        shq(&archive)
    );
    let started = std::time::Instant::now();
    let (out, code) = run(ssh, name, &cmd, PACK_TIMEOUT_SECS).await?;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    if code != 0 {
        // 打包失败 → 清理残包（**不能留在列表里冒充可用快照**）
        let _ = run(ssh, name, &format!("rm -f {}", shq(&archive)), 30).await;
        return Err(anyhow!("打包失败（exit {}）：{}", code, out.trim()));
    }

    let size: u64 = out
        .lines()
        .last()
        .and_then(|l| l.trim().parse::<u64>().ok())
        .unwrap_or(0);

    // 校验：tar -t 能列目录才算完整（截断包会 exit≠0，实测确认）
    let (_, vcode) = run(
        ssh,
        name,
        &format!("tar -tzf {} > /dev/null 2>&1", shq(&archive)),
        60,
    )
    .await?;
    let verified = vcode == 0;
    if !verified {
        let _ = run(ssh, name, &format!("rm -f {}", shq(&archive)), 30).await;
        return Err(anyhow!("快照校验失败（包可能被截断），已删除残包。"));
    }

    // 校验通过才记索引（§14.1：exit=0 才写 index.json）
    let mut idx = read_index(ssh, name).await;
    upsert_index_entry(&mut idx, &file, paths);
    write_index(ssh, name, &idx).await;

    Ok(PackResult {
        file,
        path: archive,
        size,
        verified,
        elapsed_ms,
        paths: paths.to_vec(),
        kind,
    })
}

/// 列出快照（按时间倒序）。**只列校验通过的包** —— 残包不进列表。
pub async fn list(ssh: &Arc<SshManager>, name: &str) -> Result<Vec<SnapshotEntry>> {
    let (home_out, _) = run(ssh, name, "printf %s \"$HOME\"", PROBE_TIMEOUT_SECS).await?;
    let dir = format!("{}/{}", home_out.trim().trim_end_matches('/'), DEFAULT_DIR);
    // 输出: 文件名<TAB>字节数<TAB>mtime秒
    let cmd = format!(
        "mkdir -p {d} && find {d} -maxdepth 1 -name 'snap-*.tar.gz' -printf '%f\\t%s\\t%T@\\n' 2>/dev/null | sort -t$'\\t' -k3 -nr",
        d = shq(&dir)
    );
    let (out, _) = run(ssh, name, &cmd, PROBE_TIMEOUT_SECS).await?;

    let mut entries = Vec::new();
    for line in out.lines() {
        let mut it = line.split('\t');
        let (Some(f), Some(sz), Some(mt)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if f.is_empty() || !f.starts_with("snap-") {
            continue;
        }
        let size = sz.trim().parse::<u64>().unwrap_or(0);
        // %T@ 形如 "1786791845.123456" → 取整数部分
        let created_at = mt
            .trim()
            .split('.')
            .next()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        entries.push(SnapshotEntry {
            file: f.to_string(),
            path: format!("{}/{}", dir, f),
            created_at,
            size,
            verified: true, // 残包已在 pack 里删掉；此处只列存在的
            paths: Vec::new(),
            kind: kind_from_filename(f),
        });
    }
    // 范围（paths）来自 index.json：缺失的包留空（功能降级，不影响列表本身）
    let idx = read_index(ssh, name).await;
    for e in &mut entries {
        if let Some(ie) = idx.entries.iter().find(|i| i.file == e.file) {
            e.paths = ie.paths.clone();
        }
    }
    Ok(entries)
}

/// 列出包内文件（成员路径 + 大小）。只列普通文件（目录条目跳过；
/// 契约与 restore.rs 的 `parse_tar_list` 一致）。
pub async fn members(ssh: &Arc<SshManager>, name: &str, file: &str) -> Result<Vec<MemberEntry>> {
    check_snapshot_file(file)?;
    let dir = snapshot_dir(ssh, name).await?;
    let archive = format!("{}/{}", dir, file);
    let (out, code) = run_capped(
        ssh,
        name,
        &crate::restore::list_cmd(&archive),
        FILE_OP_TIMEOUT_SECS,
        MEMBERS_RETAIN_CAP,
    )
    .await?;
    if code != 0 {
        return Err(anyhow!("无法读取快照包（exit {}）", code));
    }
    if out.len() >= MEMBERS_RETAIN_CAP {
        // 输出被上限截断会静默丢成员 —— 宁可明确报错，不让 UI 展示残缺列表
        return Err(anyhow!("快照内文件过多，无法完整浏览（超出 {} MiB 列表上限）", MEMBERS_RETAIN_CAP / 1024 / 1024));
    }
    Ok(crate::restore::parse_tar_list(&out)
        .into_iter()
        .map(|(path, size)| MemberEntry { path, size })
        .collect())
}

/// 从包中取回单个文件内容（base64；超 `MAX_FETCH_BYTES` 时截断并置 `truncated`）。
pub async fn fetch_file(
    ssh: &Arc<SshManager>,
    name: &str,
    file: &str,
    member: &str,
) -> Result<FetchResult> {
    check_snapshot_file(file)?;
    guard_member(member)?;
    let dir = snapshot_dir(ssh, name).await?;
    let archive = format!("{}/{}", dir, file);

    // 1) 包内原始大小（顺带确认成员存在；awk 只认普通文件）
    let size_cmd = format!(
        "tar -tvzf {p} 2>/dev/null | awk -v m={m} '$1 ~ /^-/ {{s=$3; $1=$2=$3=$4=$5=\"\"; sub(/^ +/,\"\"); if ($0 == m) {{print s; exit}}}}'",
        p = shq(&archive),
        m = shq(member)
    );
    let (sz_out, _) = run(ssh, name, &size_cmd, FILE_OP_TIMEOUT_SECS).await?;
    let Some(size) = sz_out
        .lines()
        .last()
        .and_then(|l| l.trim().parse::<u64>().ok())
    else {
        return Err(anyhow!("快照内没有该文件：{}", member));
    };

    // 2) 取内容（远端先 head -c 截断，避免整文件回传；base64 无换行）
    let truncated = size > MAX_FETCH_BYTES as u64;
    let take = size.min(MAX_FETCH_BYTES as u64);
    let get_cmd = format!(
        "tar -xzOf {p} {m} 2>/dev/null | head -c {n} | base64 -w0",
        p = shq(&archive),
        m = shq(member),
        n = take
    );
    let (b64, code) = run_capped(ssh, name, &get_cmd, FILE_OP_TIMEOUT_SECS, CAPPED_OUTPUT).await?;
    if code != 0 {
        return Err(anyhow!("读取快照内文件失败（exit {}）", code));
    }
    Ok(FetchResult {
        content_b64: b64.trim().to_string(),
        truncated,
        size,
    })
}

/// 把包内单个文件**就地**还原到原路径（`/` + 成员路径）。
///
/// 这是"整包还原"之外的精细路径：只动一个文件。覆盖前先把原文件备份到
/// 快照目录（`backup-<ts>-<文件名>`），备份失败即中止（**绝不无备份覆盖**）。
pub async fn restore_file(
    ssh: &Arc<SshManager>,
    name: &str,
    file: &str,
    member: &str,
) -> Result<FileRestoreResult> {
    check_snapshot_file(file)?;
    guard_member(member)?;
    let target = format!("/{}", member);
    let dir = snapshot_dir(ssh, name).await?;
    let archive = format!("{}/{}", dir, file);

    let (ts_out, _) = run(ssh, name, "date -u +%s", PROBE_TIMEOUT_SECS).await?;
    let ts: u64 = ts_out.trim().parse().unwrap_or(0);
    let base = member.rsplit('/').next().unwrap_or("file");
    let backup = format!("{}/backup-{}-{}", dir, ts, base);

    let cmd = format!(
        "if [ -e {t} ]; then cp -a {t} {b} || exit 41; echo BACKUP; else echo NOBACKUP; fi; \
         mkdir -p \"$(dirname {t})\" || exit 44; \
         tar -xzf {a} -C / {m} || exit 42; \
         test -f {t} || exit 43; \
         stat -c %s {t}",
        t = shq(&target),
        b = shq(&backup),
        a = shq(&archive),
        m = shq(member)
    );
    let (out, code) = run(ssh, name, &cmd, FILE_OP_TIMEOUT_SECS).await?;
    match code {
        0 => {}
        41 => return Err(anyhow!("备份原文件失败，已中止（未覆盖）")),
        42 => return Err(anyhow!("从快照解包失败，已中止")),
        43 => return Err(anyhow!("解包后目标不存在（该成员可能不是普通文件）")),
        44 => return Err(anyhow!("创建目标目录失败，已中止")),
        c => return Err(anyhow!("还原失败（exit {c}）：{}", out.trim())),
    }
    let had_backup = out.lines().next().map(|l| l.trim() == "BACKUP").unwrap_or(false);
    let size: u64 = out
        .lines()
        .last()
        .and_then(|l| l.trim().parse::<u64>().ok())
        .unwrap_or(0);
    Ok(FileRestoreResult {
        target,
        backup: had_backup.then_some(backup),
        size,
    })
}

/// 执行保留策略：按**链**保留最新 `KEEP` 条，超出删最旧整链。
///
/// 删全量必须连带其全部变化包（设计稿 §7.3）—— 只删全量会留下一串
/// 无法单独还原的孤儿变化包，比"没有快照"更误导。
pub async fn prune(ssh: &Arc<SshManager>, name: &str) -> Result<usize> {
    let entries = list(ssh, name).await?;
    let chains = build_chains(&entries);
    if chains.len() <= KEEP {
        return Ok(0);
    }
    let mut removed = 0;
    let mut gone: Vec<String> = Vec::new();
    for chain in chains.iter().skip(KEEP) {
        for e in std::iter::once(&chain.full).chain(chain.changes.iter()) {
            let (_, code) = run(ssh, name, &format!("rm -f {}", shq(&e.path)), 30).await?;
            if code == 0 {
                removed += 1;
                gone.push(e.file.clone());
            }
        }
    }
    if !gone.is_empty() {
        let mut idx = read_index(ssh, name).await;
        idx.entries.retain(|e| !gone.contains(&e.file));
        write_index(ssh, name, &idx).await;
    }
    Ok(removed)
}

/// 删除快照。若删的是**全量包**，连带删除其链上的全部变化包（孤儿清理）。
pub async fn remove(ssh: &Arc<SshManager>, name: &str, file: &str) -> Result<()> {
    check_snapshot_file(file)?;
    let entries = list(ssh, name).await?;
    let chains = build_chains(&entries);
    let mut victims: Vec<String> = Vec::new();
    if let Some(chain) = chains
        .iter()
        .find(|c| c.full.file == file || c.changes.iter().any(|e| e.file == file))
    {
        if chain.full.file == file {
            victims.push(chain.full.file.clone());
            victims.extend(chain.changes.iter().map(|e| e.file.clone()));
        } else {
            victims.push(file.to_string());
        }
    } else {
        // 索引外的异常文件（如孤儿变化包）：只删本身
        victims.push(file.to_string());
    }

    let dir = snapshot_dir(ssh, name).await?;
    for v in &victims {
        let (_, code) = run(ssh, name, &format!("rm -f {}", shq(&format!("{}/{}", dir, v))), 30).await?;
        if code != 0 {
            return Err(anyhow!("删除失败"));
        }
    }
    let mut idx = read_index(ssh, name).await;
    idx.entries.retain(|e| !victims.contains(&e.file));
    write_index(ssh, name, &idx).await;
    Ok(())
}

// ---------- 单测 ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_sanitizes_host() {
        assert_eq!(snapshot_filename("master", 100), "snap-master-100.tar.gz");
        // 危险字符必须被净化（防路径穿越）
        assert_eq!(snapshot_filename("a/b", 1), "snap-a_b-1.tar.gz");
        assert_eq!(snapshot_filename("a b;rm -rf /", 1), "snap-a_b_rm_-rf__-1.tar.gz");
        assert_eq!(snapshot_filename("", 1), "snap-host-1.tar.gz");
        // 路径穿越:净化后不得含 "..",且不得以 . 或 - 开头
        let f = snapshot_filename("../../etc", 1);
        assert!(!f.contains(".."), "不得含 ..: {f}");
        assert!(!f.starts_with('.'), "不得以 . 开头: {f}");
        // 前导 `-` 必须被剥掉:否则 tar 会把文件名当选项。
        // 注意判据要写对 —— "snap-rf-1.tar.gz" 里**确实含** "-rf-" 子串
        // (snap[-rf-]1),故不能用 contains("-rf-");应断言它不以 "snap--" 开头。
        let g = snapshot_filename("-rf", 1);
        assert_eq!(g, "snap-rf-1.tar.gz", "前导 - 须剥掉: {g}");
        assert!(!g.starts_with("snap--"), "不得出现双横线: {g}");
    }

    #[test]
    fn tar_cmd_quotes_paths() {
        let cmd = build_tar_cmd("/root/.helm-snapshots", "s.tar.gz", &["/etc".into()]);
        assert!(cmd.starts_with("tar -czf '/root/.helm-snapshots/s.tar.gz'"));
        assert!(cmd.contains("--exclude='etc/shadow'"));
        assert!(cmd.contains("--exclude='etc/ssh/ssh_host_*'"));
        assert!(cmd.contains("-C /"));
        assert!(cmd.ends_with(" 'etc'"));
    }

    #[test]
    fn tar_cmd_escapes_single_quote() {
        let cmd = build_tar_cmd("/d", "f", &["/a'b".into()]);
        // 单引号必须转义为 '\''，否则可注入
        assert!(cmd.contains("'a'\\''b'"), "实际: {cmd}");
    }

    #[test]
    fn parse_df_and_du() {
        let df = "Filesystem             1M-blocks  Used Available Capacity Mounted on\n\
                  /dev/mapper/centos-root     17280  5120     12160      30% /\n";
        assert_eq!(parse_df_avail_mb(df), Some(12160));
        assert_eq!(parse_du_mb("36\t/etc\n"), Some(36));
        assert_eq!(parse_du_mb(""), None);
        assert_eq!(parse_du_mb("du: cannot access\n"), None);
    }

    #[test]
    fn recommend_prefers_etc_and_skips_oversized() {
        // 上限 500MB:/etc(36) 与 /opt(120) 可进,/home(40000) 与 /root(832) 超限
        let sizes = vec![
            ("/etc".to_string(), 36),
            ("/home".to_string(), 40000),
            ("/root".to_string(), 832),
            ("/opt".to_string(), 120),
        ];
        let (picked, reason) = recommend_paths(&sizes, 12160, 500);
        assert!(picked.contains(&"/etc".to_string()));
        assert!(picked.contains(&"/opt".to_string()));
        assert!(!picked.contains(&"/home".to_string()), "超上限的必须被排除");
        assert!(!picked.contains(&"/root".to_string()), "832MB > 上限 500MB,须排除");
        assert!(reason.contains("已跳过"), "理由须说明跳过项: {reason}");
    }

    #[test]
    fn recommend_respects_space_budget() {
        // 可用 100MB → 预算 33MB，只有 /etc 装得下
        let sizes = vec![("/etc".to_string(), 30), ("/root".to_string(), 800)];
        let (picked, _) = recommend_paths(&sizes, 100, 500);
        assert_eq!(picked, vec!["/etc".to_string()]);
    }

    #[test]
    fn recommend_never_picks_etc_last() {
        // /etc 必须优先（即使输入顺序把它放最后）
        let sizes = vec![
            ("/opt".to_string(), 10),
            ("/usr/local".to_string(), 10),
            ("/etc".to_string(), 10),
        ];
        let (picked, _) = recommend_paths(&sizes, 10000, 500);
        assert_eq!(picked.first().map(|s| s.as_str()), Some("/etc"));
    }

    #[test]
    fn space_check_rejects_when_tight() {
        // 可用 100MB，待打包 50MB → 需 150MB → 拒绝
        assert!(space_check(100, 50).is_err());
        // 可用 200MB，待打包 50MB → 需 150MB → 通过
        assert!(space_check(200, 50).is_ok());
    }

    #[test]
    fn size_check_enforces_cap() {
        assert!(size_check(400).is_ok());
        assert!(size_check(600).is_err());
    }

    #[test]
    fn space_check_error_mentions_numbers() {
        let e = space_check(100, 50).unwrap_err().to_string();
        // 报错必须给出具体数字（用户才能判断要清理多少）
        assert!(e.contains("100"), "{e}");
        assert!(e.contains("50"), "{e}");
        assert!(e.contains("150"), "{e}");
    }

    fn entry(file: &str, ts: u64) -> SnapshotEntry {
        SnapshotEntry {
            file: file.to_string(),
            path: format!("/h/.helm-snapshots/{file}"),
            created_at: ts,
            size: 1,
            verified: true,
            paths: vec!["/etc".to_string()],
            kind: kind_from_filename(file),
        }
    }

    #[test]
    fn kind_from_filename_detects_chg() {
        assert_eq!(kind_from_filename("snap-master-100.tar.gz"), SnapshotKind::Full);
        assert_eq!(kind_from_filename("snap-master-100-chg.tar.gz"), SnapshotKind::Chg);
        // 主机名里含 "chg" 不算（后缀必须在 `-<ts>-chg` 位置）
        assert_eq!(kind_from_filename("snap-chgbox-100.tar.gz"), SnapshotKind::Full);
    }

    #[test]
    fn chg_filename_round_trips_kind() {
        let f = chg_filename("a/b", 7);
        assert_eq!(f, "snap-a_b-7-chg.tar.gz");
        assert_eq!(kind_from_filename(&f), SnapshotKind::Chg);
    }

    #[test]
    fn build_chg_tar_cmd_includes_newer_mtime() {
        let cmd = build_chg_tar_cmd("/d", "x-chg.tar.gz", &["/etc".into()], 1234567);
        assert!(cmd.contains("--newer-mtime=@1234567"), "{cmd}");
        assert!(cmd.starts_with("tar -czf '/d/x-chg.tar.gz'"));
        // 全量命令不得含该参数
        let full = build_tar_cmd("/d", "x.tar.gz", &["/etc".into()]);
        assert!(!full.contains("--newer-mtime"));
    }

    #[test]
    fn build_chains_groups_changes_under_latest_full() {
        // 输入模拟 list()：时间倒序；两条链
        let entries = vec![
            entry("snap-a-300-chg.tar.gz", 300),
            entry("snap-a-200.tar.gz", 200),
            entry("snap-a-100-chg.tar.gz", 100),
            entry("snap-a-50-chg.tar.gz", 50),
            entry("snap-a-10.tar.gz", 10),
        ];
        let chains = build_chains(&entries);
        assert_eq!(chains.len(), 2);
        // 最新链在前（UI 直接渲染顺序）
        assert_eq!(chains[0].id, 2);
        assert_eq!(chains[0].full.file, "snap-a-200.tar.gz");
        assert_eq!(chains[0].changes.len(), 1);
        assert_eq!(chains[0].changes[0].file, "snap-a-300-chg.tar.gz");
        assert_eq!(chains[1].id, 1);
        assert_eq!(chains[1].full.file, "snap-a-10.tar.gz");
        assert_eq!(chains[1].changes.len(), 2);
        // 链内变化包按时间正序（还原按此顺序叠加）
        assert_eq!(chains[1].changes[0].created_at, 50);
        assert_eq!(chains[1].changes[1].created_at, 100);
    }

    #[test]
    fn build_chains_same_second_full_before_chg() {
        // 同秒的全量+变化包：全量必须先（否则变化包无处归属）
        let entries = vec![
            entry("snap-a-100-chg.tar.gz", 100),
            entry("snap-a-100.tar.gz", 100),
        ];
        let chains = build_chains(&entries);
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].full.file, "snap-a-100.tar.gz");
        assert_eq!(chains[0].changes.len(), 1);
    }

    #[test]
    fn build_chains_drops_orphan_changes() {
        // 链前无全量的孤儿变化包：丢弃（不崩、也不静默塞进别的链）
        let entries = vec![
            entry("snap-a-100-chg.tar.gz", 100),
            entry("snap-a-50-chg.tar.gz", 50),
        ];
        assert!(build_chains(&entries).is_empty());
    }

    #[test]
    fn guard_member_rejects_traversal_and_options() {
        assert!(guard_member("etc/nginx/nginx.conf").is_ok());
        assert!(guard_member("etc/a b.conf").is_ok(), "含空格的包内路径合法");
        for bad in ["", "/etc/passwd", "-rf", "etc/../shadow", "etc/./hosts", "etc/dir/"] {
            assert!(guard_member(bad).is_err(), "必须拒绝: {bad}");
        }
    }

    #[test]
    fn check_snapshot_file_guard() {
        assert!(check_snapshot_file("snap-a-1.tar.gz").is_ok());
        for bad in ["", "a.tar.gz", "snap-a/../x", "../snap-a-1.tar.gz", "snap-a..b.tar.gz"] {
            assert!(check_snapshot_file(bad).is_err(), "必须拒绝: {bad}");
        }
    }

    #[test]
    fn upsert_index_entry_adds_and_overwrites() {
        let mut d = IndexData::default();
        upsert_index_entry(&mut d, "snap-a-1.tar.gz", &["/etc".into()]);
        assert_eq!(d.version, INDEX_VERSION);
        assert_eq!(d.entries.len(), 1);
        assert_eq!(d.entries[0].paths, vec!["/etc".to_string()]);
        // 同文件覆盖（不重复追加）
        upsert_index_entry(&mut d, "snap-a-1.tar.gz", &["/etc".into(), "/opt".into()]);
        assert_eq!(d.entries.len(), 1);
        assert_eq!(d.entries[0].paths.len(), 2);
    }

    #[test]
    fn estimate_mb_sums_recommended_only() {
        let pr = ProbeResult {
            disk_avail_mb: 1000,
            path_sizes: vec![("/etc".into(), 36), ("/root".into(), 832)],
            recommended: vec!["/etc".into()],
            reason: String::new(),
            lvm_free_mb: 0,
            reflink: false,
            mem_total_mb: 2048,
        };
        assert_eq!(estimate_mb(&pr), 36);
    }
}
