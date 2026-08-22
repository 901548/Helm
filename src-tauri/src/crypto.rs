// ============================================================================
// crypto.rs - 本地敏感信息加密模块（Windows DPAPI）
// 使用系统凭据级加密（CryptProtectData），密文 base64 后存入 config.yaml。
// 同机同用户可解密；换机器或换账户后需重新填写。
// 用途：AI API Key（P13）、SSH 会话密码（P26）。
// ============================================================================

use anyhow::{anyhow, Result};
use data_encoding::BASE64;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
};

/// 密文落盘前缀：`enc:<base64>` 表示 DPAPI 密文，用于与旧版明文区分（迁移兼容）
pub const SECRET_ENCRYPTED_PREFIX: &str = "enc:";
/// 前端占位掩码：有已保存密钥时回传该哨兵，提交时表示"留空则不修改"
pub const SECRET_MASK: &str = "·";

/// 用 DPAPI 加密明文，返回 base64 编码的密文
pub fn encrypt_secret(plain: &str) -> Result<String> {
    let bytes = plain.as_bytes();
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB::default();
    let result = unsafe {
        CryptProtectData(
            &in_blob,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
    };
    if let Err(e) = result {
        return Err(anyhow!("DPAPI 加密失败: {e}"));
    }
    let cipher = unsafe {
        let slice = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize);
        let v = slice.to_vec();
        LocalFree(Some(HLOCAL(out_blob.pbData as *mut _)));
        v
    };
    Ok(BASE64.encode(&cipher))
}

/// 解密 DPAPI 密文（base64），返回明文
pub fn decrypt_secret(b64: &str) -> Result<String> {
    let cipher = BASE64
        .decode(b64.trim().as_bytes())
        .map_err(|e| anyhow!("密文格式无效: {e}"))?;
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: cipher.len() as u32,
        pbData: cipher.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB::default();
    let result = unsafe {
        CryptUnprotectData(
            &in_blob,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
    };
    if let Err(e) = result {
        return Err(anyhow!("解密失败（可能不是本机/本用户加密）: {e}"));
    }
    let plain = unsafe {
        let slice = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize);
        let v = slice.to_vec();
        LocalFree(Some(HLOCAL(out_blob.pbData as *mut _)));
        v
    };
    String::from_utf8(plain).map_err(|_| anyhow!("密文解码失败"))
}

/// API Key 加密（DPAPI 密文，无前缀，直接 base64）
pub fn encrypt_api_key(plain: &str) -> Result<String> {
    encrypt_secret(plain)
}

/// API Key 解密
pub fn decrypt_api_key(b64: &str) -> Result<String> {
    decrypt_secret(b64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_roundtrip() {
        let plain = "sk-1234567890abcdef";
        let enc = encrypt_api_key(plain).unwrap();
        assert_ne!(enc, plain);
        assert_eq!(decrypt_api_key(&enc).unwrap(), plain);
    }

    #[test]
    fn dpapi_bad_input_fails() {
        assert!(decrypt_api_key("!!!not-base64!!!").is_err());
        // 合法 base64 但非本机加密过的数据应解密失败（不会 panic）
        let fake = BASE64.encode(b"garbage-garbage");
        assert!(decrypt_api_key(&fake).is_err());
    }
}
