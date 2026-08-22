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
    pub fn verify_fingerprint(&mut self, host: &str, port: u16, fp: &str) -> Result<(), String> {
        let id = format!("{host}:{port}");
        match self.entries.get(&id) {
            None => {
                self.entries.insert(id, fp.to_string());
                self.persist();
                Ok(())
            }
            Some(saved) if saved == fp => Ok(()),
            Some(saved) => Err(format!(
                "主机 {} 的主机密钥已变更（旧指纹 {} → 新指纹 {}），可能遭受中间人攻击或服务器重装系统，已拒绝连接。若确认服务器正常，请先在设置中忘记该主机后重试。",
                host, saved, fp
            )),
        }
    }

    /// 遗忘指定主机密钥（密钥变更后手动重置）
    pub fn forget(&mut self, host: &str, port: u16) -> bool {
        let removed = self.entries.remove(&format!("{host}:{port}")).is_some();
        if removed {
            self.persist();
        }
        removed
    }

    /// 查询指定主机当前记录的指纹
    pub fn fingerprint(&self, host: &str, port: u16) -> Option<&str> {
        self.entries.get(&format!("{host}:{port}")).map(|s| s.as_str())
    }

    /// 写回磁盘（内存态跳过）
    fn persist(&self) {
        if let Some(path) = &self.path {
            if let Ok(s) = serde_json::to_string_pretty(&self.entries) {
                let _ = std::fs::write(path, s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_connect_trusts_and_pins() {
        let mut store = KnownHostsStore::in_memory();
        assert!(store.verify_fingerprint("192.168.1.1", 22, "fpA").is_ok());
        assert_eq!(store.fingerprint("192.168.1.1", 22), Some("fpA"));
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
        assert!(store.forget("h", 22));
        assert!(store.verify_fingerprint("h", 22, "fpB").is_ok());
    }
}
