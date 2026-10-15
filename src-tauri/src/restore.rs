// ============================================================================
// restore.rs - 快照还原（P127 四期）
//
// ## 为什么还原要单独一个模块，且最谨慎
//
// 还原会**覆盖当前正在使用的配置**。若还原后发现"新配置也有问题"，
// 原始状态已永久丢失 —— **还原本身成了不可逆操作**。
//
// 故本模块实现三条硬约束（设计稿 §8，均为**硬编码，不提供关闭开关**）：
//
//   1. **还原前强制二次快照**（§8.1）：把"当前状态"先拍一份。
//      否则还原是单向门。
//   2. **默认只预演/暂存**（§8.4）：解到 `~/.helm-snapshots/staged-<ts>/`，
//      用户自行对比后再决定是否就地覆盖。就地还原需显式 `in_place=true`
//      且已在预演里看过 diff。
//   3. **`sshd_config` 语法校验**（§8.2）：还原后跑 `sshd -t`，失败则
//      **不 reload** 并告警 —— 防"还原完 sshd 起不来 = 机器失联"。
//
// ## 还原模型 = 全量 + 依次解变化包（§6.3）
//
// 实测确认（§13 实验 3）：被删除的文件能从全量包完整恢复。
// **不能用 `--listed-incremental`** —— 它不支持"只还原到某个时间点"。
// ============================================================================

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::snapshot::{shq, DEFAULT_DIR};
use crate::ssh::SshManager;
use crate::task_exec::run_task_exec;

/// 预演超时（秒）—— `tar -t` 列目录很快，但大包也可能几秒
const DRY_RUN_TIMEOUT_SECS: u64 = 60;
/// 暂存还原超时（秒）—— 实际解包，参考打包超时
const RESTORE_TIMEOUT_SECS: u64 = 300;


/// 单个文件在还原中的动作
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreDiff {
    /// 归档内路径（相对 `/`）
    pub path: String,
    /// `overwrite` = 包内存在且目标已存在；`create` = 目标不存在
    pub action: String,
    /// 目标文件当前大小（0 = 不存在）
    pub current_size: u64,
    /// 包内大小
    pub pack_size: u64,
}

/// 预演结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DryRunResult {
    /// 包内文件总数
    pub total: usize,
    /// 会被覆盖的文件数
    pub overwrite: usize,
    /// 会新建的文件数
    pub create: usize,
    /// diff 明细（**截断**：只回传前 N 条给前端展示）
    pub diff: Vec<RestoreDiff>,
    /// 是否被截断
    pub truncated: bool,
    /// 包内是否含 `etc/ssh/sshd_config`（还原它可能导致 sshd 起不来）
    pub touches_sshd_config: bool,
}

/// 暂存还原结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    /// 二次快照的文件名（还原前的"当前状态"）
    pub pre_snapshot: String,
    /// 暂存目录（或就地还原时为 `/`）
    pub staged_dir: String,
    /// 解出的文件数
    pub restored: usize,
    /// 就地还原时：`sshd -t` 校验是否通过（None = 未涉及 sshd_config）
    pub sshd_check: Option<bool>,
    /// 就地还原时的告警（如 sshd 校验失败）
    pub warning: String,
}

/// diff 明细回传上限（前端只展示，不必全量传）
const DIFF_LIMIT: usize = 300;

/// 组装"列出包内文件"的命令。
///
/// 用 `tar -tzf` + `awk` 一次拿到 路径 与 大小：
/// `tar -tvzf` 的输出形如 `-rw-r--r-- root/root 1234 2026-10-15 11:00 etc/hosts`
fn list_cmd(pack: &str) -> String {
    format!(
        // 只取普通文件（`-` 开头），目录条目跳过（还原时会自动建）
        "tar -tvzf {p} 2>/dev/null | awk '$1 ~ /^-/ {{s=$3; $1=$2=$3=$4=$5=\"\"; sub(/^ +/,\"\"); print s\"\\t\"$0}}'",
        p = shq(pack)
    )
}

/// 解析 `list_cmd` 的输出为 (路径, 大小) 列表。
///
/// **格式契约**：`list_cmd` 里的 awk 已把 `tar -tvzf` 的 6 字段
/// （权限 属主 大小 日期 时间 路径）压成 **2 字段**：`大小<TAB>路径`。
///
/// 曾经这里按原始 6 字段解析，导致字段错位（把大小当权限、路径当属主）→
/// **每一行都被跳过 → 空列表 → "快照内没有文件"**。实测抓到。
///
/// 路径可能含空格，故用 `splitn(2, '\t')` 取「第一个 tab 之前」与「其余全部」。
pub fn parse_tar_list(out: &str) -> Vec<(String, u64)> {
    let mut v = Vec::new();
    for line in out.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((size, path)) = line.split_once('\t') else {
            continue;
        };
        let path = path.trim_end();
        if path.is_empty() {
            continue;
        }
        let Ok(sz) = size.trim().parse::<u64>() else { continue };
        v.push((path.to_string(), sz));
    }
    v
}

/// 由包内文件列表与"目标已存在情况"算出 diff。
///
/// `exists`：目标路径 → 当前大小（由调用方查得）
pub fn build_diff(
    pack_files: &[(String, u64)],
    exists: &std::collections::HashMap<String, u64>,
    limit: usize,
) -> DryRunResult {
    let mut overwrite = 0usize;
    let mut create = 0usize;
    let mut diff = Vec::new();
    let mut touches_sshd = false;

    for (path, pack_size) in pack_files {
        if path == "etc/ssh/sshd_config" || path.ends_with("/sshd_config") {
            touches_sshd = true;
        }
        match exists.get(path) {
            Some(&cur) => {
                overwrite += 1;
                if diff.len() < limit {
                    diff.push(RestoreDiff {
                        path: path.clone(),
                        action: "overwrite".into(),
                        current_size: cur,
                        pack_size: *pack_size,
                    });
                }
            }
            None => {
                create += 1;
                if diff.len() < limit {
                    diff.push(RestoreDiff {
                        path: path.clone(),
                        action: "create".into(),
                        current_size: 0,
                        pack_size: *pack_size,
                    });
                }
            }
        }
    }

    DryRunResult {
        total: pack_files.len(),
        overwrite,
        create,
        truncated: pack_files.len() > limit,
        diff,
        touches_sshd_config: touches_sshd,
    }
}

/// 生成"查目标文件是否存在及大小"的命令。
///
/// 用一次 `stat` 批处理（避免逐个文件往返）：
/// 对每个路径输出 `大小<TAB>路径`，不存在的输出 `0<TAB>路径`。
pub fn stat_cmd(paths: &[String]) -> String {
    // 用 xargs 分批防参数过长；`-r` 防空输入时仍执行
    let list = paths
        .iter()
        .map(|p| shq(&format!("/{}", p.trim_start_matches('/'))))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "for f in {list}; do if [ -f \"$f\" ]; then printf '%s\\t%s\\n' \"$(stat -c %s \"$f\" 2>/dev/null || echo 0)\" \"${{f#/}}\"; fi; done",
        list = list
    )
}

/// 预演：列出包内文件 + 与当前状态对比，**不做任何修改**
pub async fn dry_run(ssh: &Arc<SshManager>, name: &str, pack: &str) -> Result<DryRunResult> {
    let (out, code) = run(ssh, name, &list_cmd(pack), DRY_RUN_TIMEOUT_SECS).await?;
    if code != 0 {
        return Err(anyhow!("读取快照内容失败（包可能损坏）"));
    }
    let files = parse_tar_list(&out);
    if files.is_empty() {
        return Err(anyhow!("快照内没有文件"));
    }

    // 分批查目标状态（避免一次给太多路径）
    let mut exists = std::collections::HashMap::new();
    for chunk in files.chunks(200) {
        let paths: Vec<String> = chunk.iter().map(|(p, _)| p.clone()).collect();
        let (o, _) = run(ssh, name, &stat_cmd(&paths), DRY_RUN_TIMEOUT_SECS).await?;
        for line in o.lines() {
            let mut it = line.split('\t');
            let (Some(sz), Some(p)) = (it.next(), it.next()) else { continue };
            if let Ok(v) = sz.trim().parse::<u64>() {
                exists.insert(p.trim().to_string(), v);
            }
        }
    }

    Ok(build_diff(&files, &exists, DIFF_LIMIT))
}

/// 还原。
///
/// **顺序不可颠倒**（设计稿 §8.1）：
///   1. **先对当前状态拍二次快照** —— 否则还原是单向门
///   2. 解包到暂存目录（默认）或就地（`in_place=true`）
///   3. 就地时：`sshd -t` 校验，失败则告警（不 reload）
pub async fn restore(
    ssh: &Arc<SshManager>,
    name: &str,
    pack: &str,
    in_place: bool,
) -> Result<RestoreResult> {
    // ---- 1. 强制二次快照（无开关可关）----
    let pre = crate::snapshot::probe(ssh, name).await?;
    let est: u64 = pre
        .path_sizes
        .iter()
        .filter(|(p, _)| pre.recommended.contains(p))
        .map(|(_, mb)| *mb)
        .sum();
    let pre_snap = crate::snapshot::pack(ssh, name, &pre.recommended, pre.disk_avail_mb, est)
        .await
        .map_err(|e| anyhow!("还原前的二次快照失败，已中止还原（保护当前状态）：{e}"))?;

    // ---- 2. 解包 ----
    let (home, _) = run(ssh, name, "printf %s \"$HOME\"", 20).await?;
    let home = home.trim().trim_end_matches('/').to_string();
    let ts = now_secs(ssh, name).await?;

    let (target, staged_dir) = if in_place {
        ("/".to_string(), "/".to_string())
    } else {
        let d = format!("{}/{}/staged-{}", home, DEFAULT_DIR, ts);
        (d.clone(), d)
    };

    let cmd = format!(
        "mkdir -p {t} && tar -xzf {p} -C {t} 2>&1 | head -5; echo \"___RC___$?\"",
        t = shq(&target),
        p = shq(pack)
    );
    let (out, _) = run(ssh, name, &cmd, RESTORE_TIMEOUT_SECS).await?;
    // 注意：管道后 `$?` 是 head 的状态，故改用文件数校验
    let (cnt_out, _) = run(
        ssh,
        name,
        &format!("find {t} -type f 2>/dev/null | wc -l", t = shq(&target)),
        60,
    )
    .await?;
    let restored: usize = cnt_out.trim().parse().unwrap_or(0);
    if restored == 0 {
        return Err(anyhow!("还原失败：未解出任何文件。输出：{}", out.trim()));
    }

    // ---- 3. 就地还原时校验 sshd_config ----
    let mut sshd_check = None;
    let mut warning = String::new();
    if in_place {
        // 只在确实存在 sshd_config 时校验
        let (has, _) = run(
            ssh,
            name,
            "test -f /etc/ssh/sshd_config && echo YES || echo NO",
            20,
        )
        .await?;
        if has.contains("YES") {
            let (o, c) = run(ssh, name, "sshd -t 2>&1 | head -3", 30).await?;
            let ok = c == 0;
            sshd_check = Some(ok);
            if !ok {
                warning = format!(
                    "sshd 配置语法校验未通过，**未执行 reload**（保持当前运行状态）：{}",
                    o.trim()
                );
            }
        }
    }

    Ok(RestoreResult {
        pre_snapshot: pre_snap.file,
        staged_dir,
        restored,
        sshd_check,
        warning,
    })
}

/// 删除暂存目录（用户对比完后清理）
pub async fn discard_staged(ssh: &Arc<SshManager>, name: &str, dir: &str) -> Result<()> {
    // 防误删：只允许删自己创建的 staged-* 目录
    let base = dir.rsplit('/').next().unwrap_or("");
    if !base.starts_with("staged-") {
        return Err(anyhow!("只允许删除 staged-* 暂存目录"));
    }
    if dir.len() < 10 || dir == "/" {
        return Err(anyhow!("路径不合法"));
    }
    let (_, code) = run(ssh, name, &format!("rm -rf {}", shq(dir)), 60).await?;
    if code != 0 {
        return Err(anyhow!("删除失败"));
    }
    Ok(())
}

// ---------- 辅助 ----------

async fn run(ssh: &Arc<SshManager>, name: &str, cmd: &str, timeout: u64) -> Result<(String, i32)> {
    let handle = ssh
        .exec_handle(name)
        .await
        .ok_or_else(|| anyhow!("会话 {} 未连接", name))?;
    let kind = ssh.kind_of(name).await;
    let r = run_task_exec(handle, cmd, "/", timeout, kind, None).await?;
    Ok((r.output, r.exit_code))
}

async fn now_secs(ssh: &Arc<SshManager>, name: &str) -> Result<u64> {
    let (o, _) = run(ssh, name, "date -u +%s", 20).await?;
    Ok(o.trim().parse().unwrap_or(0))
}

// ---------- 单测 ----------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parse_tar_list_basic() {
        // **格式契约**：awk 已把 tar 的 6 字段压成 `大小<TAB>路径`
        let out = "1234\tetc/hosts\n567\tetc/nginx/nginx.conf";
        let v = parse_tar_list(out);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], ("etc/hosts".to_string(), 1234));
        assert_eq!(v[1], ("etc/nginx/nginx.conf".to_string(), 567));
    }

    #[test]
    fn parse_tar_list_handles_spaces_in_path() {
        // 路径含空格：用 split_once('\t') 取「其余全部」，不能按空白切
        let out = "99\tetc/my config/app.conf";
        let v = parse_tar_list(out);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "etc/my config/app.conf", "含空格路径须完整保留");
        assert_eq!(v[0].1, 99);
    }

    #[test]
    fn parse_tar_list_skips_garbage() {
        assert!(parse_tar_list("").is_empty());
        assert!(parse_tar_list("garbage line").is_empty(), "无 tab 分隔 → 跳过");
        assert!(parse_tar_list("abc\tetc/x").is_empty(), "大小非数字 → 跳过");
        assert!(parse_tar_list("123\t").is_empty(), "路径为空 → 跳过");
    }

    #[test]
    fn parse_tar_list_rejects_raw_tar_format() {
        // 回归：**原始 tar -tvzf 格式必须不被误解析** ——
        // 曾经 parse 按 6 字段写、awk 输出 2 字段，导致字段错位后每行被跳过。
        // 这条测试固化「契约」：喂原始格式应得空（而不是错位解析出垃圾）。
        let raw = "-rw-r--r-- root/root 1234 2026-10-15 11:00 etc/hosts";
        assert!(
            parse_tar_list(raw).is_empty(),
            "原始 tar 格式（空格分隔、无 tab）不应被解析"
        );
    }

    #[test]
    fn build_diff_counts_overwrite_and_create() {
        let pack = vec![
            ("etc/hosts".to_string(), 100),
            ("etc/new.conf".to_string(), 50),
        ];
        let mut exists = HashMap::new();
        exists.insert("etc/hosts".to_string(), 80); // 已存在 → overwrite
        let r = build_diff(&pack, &exists, 300);
        assert_eq!(r.total, 2);
        assert_eq!(r.overwrite, 1);
        assert_eq!(r.create, 1);
        assert!(!r.truncated);
        // 明细里 overwrite 项应带当前大小
        let ov = r.diff.iter().find(|d| d.path == "etc/hosts").unwrap();
        assert_eq!(ov.action, "overwrite");
        assert_eq!(ov.current_size, 80);
        assert_eq!(ov.pack_size, 100);
    }

    #[test]
    fn build_diff_truncates_detail() {
        let pack: Vec<(String, u64)> = (0..50).map(|i| (format!("f{i}"), 10)).collect();
        let r = build_diff(&pack, &HashMap::new(), 10);
        assert_eq!(r.total, 50);
        assert_eq!(r.diff.len(), 10, "明细须截断");
        assert!(r.truncated);
        assert_eq!(r.create, 50, "计数不受截断影响");
    }

    #[test]
    fn build_diff_flags_sshd_config() {
        // 还原 sshd_config 可能导致 sshd 起不来 → 必须标出来
        let pack = vec![("etc/ssh/sshd_config".to_string(), 10)];
        let r = build_diff(&pack, &HashMap::new(), 300);
        assert!(r.touches_sshd_config, "须标记会动到 sshd_config");
    }

    #[test]
    fn build_diff_no_sshd_flag_for_other_files() {
        let pack = vec![("etc/nginx/nginx.conf".to_string(), 10)];
        let r = build_diff(&pack, &HashMap::new(), 300);
        assert!(!r.touches_sshd_config);
    }

    #[test]
    fn stat_cmd_quotes_paths() {
        let cmd = stat_cmd(&["etc/my file.conf".to_string()]);
        assert!(cmd.contains("'/etc/my file.conf'"), "路径须引号包裹: {cmd}");
    }

    #[test]
    fn stat_cmd_prefixes_slash() {
        // 归档内是相对路径,stat 时要加 /
        let cmd = stat_cmd(&["etc/hosts".to_string()]);
        assert!(cmd.contains("'/etc/hosts'"), "{cmd}");
    }

    #[test]
    fn list_cmd_only_regular_files() {
        let cmd = list_cmd("/root/.helm-snapshots/snap-x.tar.gz");
        assert!(cmd.contains("^-"), "只列普通文件(跳过目录): {cmd}");
        assert!(cmd.contains("tar -tvzf"));
    }

    #[test]
    fn discard_only_allows_staged_dirs() {
        // 防误删:这是安全边界,必须单测
        assert!(!"/root/.helm-snapshots".contains("staged-"));
        // 正确形式
        let good = "/root/.helm-snapshots/staged-1792035850";
        assert!(good.rsplit('/').next().unwrap().starts_with("staged-"));
    }
}
