// ============================================================================
// known_hosts.rs - 主机密钥 TOFU 存储（P26）
// 首次连接记录服务器公钥指纹（sha256），后续连接比对：
//   - 首次 → 信任并记录（Trust On First Use）
//   - 一致 → 通过
//   - 不一致 → 拒绝连接并给出明确告警（防中间人/密钥轮换误判）
// 持久化于 config.yaml 同目录的 known_hosts.json。
// ============================================================================

use std::collections::HashMap;
use std::path::PathBuf;

use russh::keys::key::PublicKey;

/// 已知主机密钥库：`host:port` → 公钥 sha256 指纹
pub struct KnownHostsStore {
    /// 持久化文件路径；None 表示内存态（不落盘，测试用）
    path: Option<PathBuf>,
    /// host:port → 指纹
    entries: HashMap<String, String>,
}

impl KnownHostsStore {
    /// 从指定文件加载；文件不存在或损坏时返回空库
    pub fn load(path: PathBuf) -> Self {
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { path: Some(path), entries }
    }

    /// 纯内存库（测试/无配置路径时使用，不落盘）
    pub fn in_memory() -> Self {
        Self { path: None, entries: HashMap::new() }
    }

    /// 校验/记录主机密钥。
    ///
    /// Ok(()) = 信任(首次记录或指纹一致);Err(原因) = 指纹变更拒绝。
    /// 失败原因经返回值传递(P47:不再用共享单槽,天然并发安全)。
    pub fn verify(&mut self, host: &str, port: u16, key: &PublicKey) -> Result<(), String> {
        self.verify_fingerprint(host, port, &key.fingerprint())
    }

    /// 校验/记录主机指纹（纯逻辑，供 verify 与测试使用）
    ///
    /// P100-1：首次信任改为**落盘成功才提交内存**，且写盘失败（只读盘/目录不可写）
    /// 直接返回 Err（fail-closed）——旧实现 `let _ = fs::write(...)` 静默吞错，
    /// 会"看着连上了，其实指纹没记住"，下次连接重新 TOFU。
    pub fn verify_fingerprint(&mut self, host: &str, port: u16, fp: &str) -> Result<(), String> {
        let id = format!("{host}:{port}");
        match self.entries.get(&id) {
            None => {
                let mut next = self.entries.clone();
                next.insert(id, fp.to_string());
                self.write_entries(&next)?;
                self.entries = next; // 落盘成功才提交内存
                Ok(())
            }
            Some(saved) if saved == fp => Ok(()),
            Some(saved) => Err(format!(
                "主机 {} 的主机密钥已变更（旧指纹 {} → 新指纹 {}），可能遭受中间人攻击或服务器重装系统，已拒绝连接。若确认服务器正常，请先在设置中忘记该主机后重试。",
                host, saved, fp
            )),
        }
    }

    /// 遗忘指定主机密钥（密钥变更后手动重置）；写盘失败返回 Err
    pub fn forget(&mut self, host: &str, port: u16) -> Result<bool, String> {
        let id = format!("{host}:{port}");
        if !self.entries.contains_key(&id) {
            return Ok(false);
        }
        let mut next = self.entries.clone();
        next.remove(&id);
        self.write_entries(&next)?;
        self.entries = next;
        Ok(true)
    }

    /// 查询指定主机当前记录的指纹
    pub fn fingerprint(&self, host: &str, port: u16) -> Option<&str> {
        self.entries.get(&format!("{host}:{port}")).map(|s| s.as_str())
    }

    /// 写回磁盘（内存态跳过）：原子写 + 错误上报（P100-1）
    fn write_entries(&self, entries: &HashMap<String, String>) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(()); // 内存态（测试）
        };
        let s = serde_json::to_string_pretty(entries)
            .map_err(|e| format!("主机密钥序列化失败: {e}"))?;
        crate::config::atomic_write(path, &s)
            .map_err(|e| format!("写入主机密钥库 {} 失败: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_connect_trusts_and_pins() {
        let mut store = KnownHostsStore::in_memory();
        assert!(store.verify_fingerprint("192.168.1.100", 22, "fpA").is_ok());
        assert_eq!(store.fingerprint("192.168.1.100", 22), Some("fpA"));
    }

    #[test]
    fn same_key_keeps_trusting() {
        let mut store = KnownHostsStore::in_memory();
        store.verify_fingerprint("h", 22, "fpA").unwrap();
        assert!(store.verify_fingerprint("h", 22, "fpA").is_ok());
    }

    #[test]
    fn changed_key_is_rejected_with_message() {
        let mut store = KnownHostsStore::in_memory();
        store.verify_fingerprint("h", 22, "fpA").unwrap();
        let err = store.verify_fingerprint("h", 22, "fpB").unwrap_err();
        assert!(err.contains("主机密钥已变更"));
    }

    #[test]
    fn forget_removes_entry() {
        let mut store = KnownHostsStore::in_memory();
        store.verify_fingerprint("h", 22, "fpA").unwrap();
        assert!(store.forget("h", 22).unwrap());
        assert!(store.verify_fingerprint("h", 22, "fpB").is_ok());
    }

    /// P100-1：写盘失败必须上报（fail-closed），且**不得**把指纹留在内存
    /// （旧实现 `let _ = fs::write` 吞错 → 看着连上了，指纹其实没记住）
    #[test]
    fn persist_failure_is_reported_and_not_committed() {
        // 指向不存在的目录 → 原子写必定失败
        let bad = std::env::temp_dir()
            .join("helm-no-such-dir-p1001")
            .join("known_hosts.json");
        let mut store = KnownHostsStore::load(bad);
        let err = store.verify_fingerprint("h", 22, "fpA").unwrap_err();
        assert!(err.contains("写入主机密钥库"), "错误应说明写盘失败: {err}");
        assert_eq!(store.fingerprint("h", 22), None, "落盘失败不得提交到内存");
        assert!(store.forget("h", 22).is_ok(), "未记录的主机遗忘应为 no-op 成功");
    }

    /// P100-1：写盘成功路径——条目落盘并可从磁盘重新加载
    #[test]
    fn persist_success_round_trips_to_disk() {
        let dir = std::env::temp_dir().join(format!("helm-kh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("known_hosts.json");
        let _ = std::fs::remove_file(&path);
        {
            let mut store = KnownHostsStore::load(path.clone());
            store.verify_fingerprint("h", 22, "fpA").unwrap();
            assert!(store.forget("h", 22).unwrap());
            store.verify_fingerprint("h2", 2222, "fpB").unwrap();
        }
        let reloaded = KnownHostsStore::load(path.clone());
        assert_eq!(reloaded.fingerprint("h2", 2222), Some("fpB"));
        assert_eq!(reloaded.fingerprint("h", 22), None);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
