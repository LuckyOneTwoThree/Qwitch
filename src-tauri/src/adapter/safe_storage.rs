//! Chromium / Electron `safeStorage` 的 macOS `v10` 信封加解密。
//!
//! 豆包工作与码道 CodeArts 都是 Electron 客户端：登录态被 `safeStorage`
//! 加密后存进本地文件（豆包：`Default/Cookies` 的 `encrypted_value`；
//! 码道：`secretStorage/<sha256(key)>.enc`）。要让「不依赖整份 profile 快照」
//! 的切号成立，就必须能**双向**处理这层信封：
//!
//! * 密钥：Keychain 条目 `<AppName> Safe Storage` 的密码；
//! * 派生：`PBKDF2-HMAC-SHA1(password, "saltysalt", 1003, 16)`；
//! * 加密：AES-128-CBC + 16 个空格 IV + PKCS#7，密文前加 `v10` 魔数。
//!
//! 2024 年之后的 Chromium 在 cookie 值前额外拼了 `SHA256(host_key)`（域名绑定），
//! 由调用方自行处理前缀，本模块只负责信封本身。
//!
//! 仅 macOS 实现：Windows 的 `safeStorage` 走 DPAPI（新版还叠加 App-Bound
//! 密钥），无法在无客户端身份的情况下复现，调用方必须在非 macOS 上返回
//! 「不支持」而不是静默失败。

use crate::adapter::AdapterError;
use crate::domain::UnifiedErrorCode;

pub(crate) const SAFE_STORAGE_SALT: &[u8] = b"saltysalt";
pub(crate) const SAFE_STORAGE_ITERATIONS: u32 = 1003;
pub(crate) const SAFE_STORAGE_KEY_LEN: usize = 16;
pub(crate) const SAFE_STORAGE_IV: [u8; 16] = [b' '; 16];

#[cfg(not(target_os = "macos"))]
fn unsupported_platform() -> AdapterError {
    AdapterError::Unified {
        code: UnifiedErrorCode::UnsupportedCapability,
        message: "当前平台不支持读取客户端 safeStorage 凭据（仅 macOS 可复现）".into(),
    }
}

/// 从 Keychain 读取 `<AppName> Safe Storage` 的密码。
#[cfg(target_os = "macos")]
pub(crate) fn macos_safe_storage_password(app_name: &str) -> Result<String, AdapterError> {
    use std::process::Command;

    let service = format!("{app_name} Safe Storage");
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            &service,
            "-a",
            app_name,
            "-w",
        ])
        .output()
        .map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("调用 security 读取钥匙串失败: {e}"),
        })?;
    if !output.status.success() {
        return Err(AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: format!(
                "未能从钥匙串读取「{service}」，请在系统「钥匙串访问」中允许 Qwitch 访问该条目"
            ),
        });
    }
    let pw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if pw.is_empty() {
        return Err(AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: format!("钥匙串条目「{service}」为空"),
        });
    }
    Ok(pw)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn macos_safe_storage_password(_app_name: &str) -> Result<String, AdapterError> {
    Err(unsupported_platform())
}

/// 解密 `v10` 信封（自动剥离 `v10` 前缀）。
pub(crate) fn decrypt_safe_storage(
    payload: &[u8],
    password: &str,
    context: &str,
) -> Result<Vec<u8>, AdapterError> {
    use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};

    let body = if payload.len() > 3 && &payload[..3] == b"v10" {
        &payload[3..]
    } else {
        payload
    };
    if body.is_empty() {
        return Err(AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: format!("{context} 凭据文件为空"),
        });
    }

    let mut key = [0u8; SAFE_STORAGE_KEY_LEN];
    pbkdf2_hmac_sha1(
        password.as_bytes(),
        SAFE_STORAGE_SALT,
        SAFE_STORAGE_ITERATIONS,
        &mut key,
    );

    type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
    let mut buf = body.to_vec();
    let plain = Aes128CbcDec::new(&key.into(), &SAFE_STORAGE_IV.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: format!("{context} 凭据解密失败（钥匙串密码不匹配）"),
        })?;
    Ok(plain.to_vec())
}

/// 加密成 `v10` 信封：`b"v10" + AES-128-CBC(PKCS#7)`。
pub(crate) fn encrypt_safe_storage(
    plain: &[u8],
    password: &str,
    context: &str,
) -> Result<Vec<u8>, AdapterError> {
    use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};

    if plain.is_empty() {
        return Err(AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("{context} 待加密内容为空"),
        });
    }

    let mut key = [0u8; SAFE_STORAGE_KEY_LEN];
    pbkdf2_hmac_sha1(
        password.as_bytes(),
        SAFE_STORAGE_SALT,
        SAFE_STORAGE_ITERATIONS,
        &mut key,
    );

    type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
    let cipher = Aes128CbcEnc::new(&key.into(), &SAFE_STORAGE_IV.into());
    let mut out = cipher.encrypt_padded_vec_mut::<Pkcs7>(plain);

    let mut envelope = Vec::with_capacity(out.len() + 3);
    envelope.extend_from_slice(b"v10");
    envelope.append(&mut out);
    Ok(envelope)
}

/// PBKDF2-HMAC-SHA1（Chromium safeStorage 的密钥派生）。
fn pbkdf2_hmac_sha1(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    use hmac::{Hmac, Mac};
    use sha1::Sha1;

    let mut block_index: u32 = 1;
    let mut filled = 0usize;
    while filled < out.len() {
        let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(password).expect("HMAC 接受任意长度密钥");
        mac.update(salt);
        mac.update(&block_index.to_be_bytes());
        let mut u = mac.finalize().into_bytes();
        let mut acc = u;
        for _ in 1..iterations {
            let mut mac =
                <Hmac<Sha1> as Mac>::new_from_slice(password).expect("HMAC 接受任意长度密钥");
            mac.update(&u);
            u = mac.finalize().into_bytes();
            for (a, b) in acc.iter_mut().zip(u.iter()) {
                *a ^= b;
            }
        }
        let take = (out.len() - filled).min(acc.len());
        out[filled..filled + take].copy_from_slice(&acc[..take]);
        filled += take;
        block_index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbkdf2_matches_known_vector() {
        // RFC 6070 的一轮用例（salt="salt", password="password", dkLen=20）
        let mut out = [0u8; 20];
        pbkdf2_hmac_sha1(b"password", b"salt", 1, &mut out);
        assert_eq!(hex::encode(out), "0c60c80f961f0e71f3a9b524af6012062fe037a6");
    }

    #[test]
    fn encrypt_then_decrypt_roundtrip() {
        let plain = b"{\"session\":{\"accessKey\":\"AK\"}}";
        let env = encrypt_safe_storage(plain, "pw-123", "test").expect("encrypt");
        assert_eq!(&env[..3], b"v10");
        let back = decrypt_safe_storage(&env, "pw-123", "test").expect("decrypt");
        assert_eq!(back, plain);
    }

    #[test]
    fn decrypt_rejects_wrong_password() {
        let env = encrypt_safe_storage(b"secret", "right", "test").expect("encrypt");
        assert!(decrypt_safe_storage(&env, "wrong", "test").is_err());
    }
}
