use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::domain::CredentialEnvelope;
use crate::storage::StorageError;

use std::ptr::write_volatile;
use std::sync::atomic::{compiler_fence, Ordering};

const NONCE_LEN: usize = 12;

/// 凭据样本：`(aad, blob)` 二元组。用于启动期主密钥校验时按行 AAD 解密。
pub type AadBlob = (Vec<u8>, Vec<u8>);

fn zeroize_bytes(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: b 是有效可写的可变引用字节，使用 volatile 防止编译器优化消除清零
        unsafe {
            write_volatile(b, 0);
        }
    }
    compiler_fence(Ordering::SeqCst);
}

/// 新密文格式魔数前缀：`QWA1` = Qwitch AEAD v1（带 AAD 绑定）。
/// 旧库密文无此前缀（`[nonce][ciphertext]`），读取时走兼容分支。
const AAD_MAGIC: &[u8; 4] = b"QWA1";

/// 派生凭据密文的 AAD（附加认证数据）：绑定 `tool_id` 与 `account_id`。
///
/// 同一份密文若被移植到其它账号行（跨账号密文替换攻击），AAD 不匹配将导致
/// AES-GCM 解密认证失败，从而阻断凭据串号读取。
pub fn credential_aad(tool_id: &str, account_id: &str) -> Vec<u8> {
    format!("{tool_id}:{account_id}").into_bytes()
}

pub fn encrypt_credential(
    master_key: &[u8; 32],
    aad: &[u8],
    envelope: &CredentialEnvelope,
) -> Result<Vec<u8>, StorageError> {
    let mut plaintext =
        serde_json::to_vec(envelope).map_err(|e| StorageError::Crypto(e.to_string()))?;

    let cipher =
        Aes256Gcm::new_from_slice(master_key).map_err(|e| StorageError::Crypto(e.to_string()))?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext_res = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext.as_ref(),
                aad,
            },
        )
        .map_err(|e| StorageError::Crypto(format!("Encryption failed: {e}")));
    zeroize_bytes(&mut plaintext);
    let ciphertext = ciphertext_res?;

    let mut out = Vec::with_capacity(AAD_MAGIC.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(AAD_MAGIC);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// 解密凭据密文。`aad` 必须与加密时一致（新格式 `QWA1`）；旧格式密文忽略
/// `aad` 走兼容解密，供 `migrate_credential_aad` 平滑升级。
pub fn decrypt_credential(
    master_key: &[u8; 32],
    aad: &[u8],
    blob: &[u8],
) -> Result<CredentialEnvelope, StorageError> {
    let cipher =
        Aes256Gcm::new_from_slice(master_key).map_err(|e| StorageError::Crypto(e.to_string()))?;

    let mut plaintext =
        if blob.len() >= AAD_MAGIC.len() + NONCE_LEN && &blob[..AAD_MAGIC.len()] == AAD_MAGIC {
            // 新格式：magic + nonce + ciphertext，AAD 参与认证
            let rest = &blob[AAD_MAGIC.len()..];
            let (nonce_bytes, ciphertext) = rest.split_at(NONCE_LEN);
            let nonce = Nonce::from_slice(nonce_bytes);
            cipher
                .decrypt(
                    nonce,
                    Payload {
                        msg: ciphertext,
                        aad,
                    },
                )
                .map_err(|e| StorageError::Crypto(format!("Decryption failed: {e}")))?
        } else {
            // 旧格式：nonce + ciphertext，无 AAD（忽略 aad，兼容历史密文）
            if blob.len() <= NONCE_LEN {
                return Err(StorageError::Crypto("Encrypted blob is too short".into()));
            }
            let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);
            let nonce = Nonce::from_slice(nonce_bytes);
            cipher
                .decrypt(nonce, ciphertext)
                .map_err(|e| StorageError::Crypto(format!("Decryption failed: {e}")))?
        };

    let parsed =
        serde_json::from_slice(&plaintext).map_err(|e| StorageError::Crypto(e.to_string()));
    zeroize_bytes(&mut plaintext);
    parsed
}

/// 判断密文是否已是带 AAD 的新格式（供 AAD 迁移跳过已升级行）。
pub fn is_aad_sealed(blob: &[u8]) -> bool {
    blob.len() >= AAD_MAGIC.len() && &blob[..AAD_MAGIC.len()] == AAD_MAGIC
}

/// 使用 AES-256-GCM + `QWA1` + AAD 加密任意字节载荷（用于 Agent 原始配置 Stash 等）。
pub fn encrypt_bytes_with_aad(
    master_key: &[u8; 32],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, StorageError> {
    let cipher =
        Aes256Gcm::new_from_slice(master_key).map_err(|e| StorageError::Crypto(e.to_string()))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|e| StorageError::Crypto(format!("Encryption failed: {e}")))?;

    let mut out = Vec::with_capacity(AAD_MAGIC.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(AAD_MAGIC);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// 解密由 `encrypt_bytes_with_aad` 生成的密文。
pub fn decrypt_bytes_with_aad(
    master_key: &[u8; 32],
    aad: &[u8],
    blob: &[u8],
) -> Result<Vec<u8>, StorageError> {
    if blob.len() < AAD_MAGIC.len() + NONCE_LEN || &blob[..AAD_MAGIC.len()] != AAD_MAGIC {
        return Err(StorageError::Crypto(
            "Invalid encrypted stash format".into(),
        ));
    }
    let cipher =
        Aes256Gcm::new_from_slice(master_key).map_err(|e| StorageError::Crypto(e.to_string()))?;
    let rest = &blob[AAD_MAGIC.len()..];
    let (nonce_bytes, ciphertext) = rest.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|e| StorageError::Crypto(format!("Decryption failed: {e}")))
}

pub fn key_fingerprint(master_key: &[u8; 32]) -> String {
    hex::encode(&Sha256::digest(master_key)[..16])
}

pub fn compute_uid_hash(tool_id: &str, uid: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(tool_id.as_bytes());
    hasher.update(b":");
    hasher.update(uid.as_bytes());
    hex::encode(hasher.finalize())
}

/// 以主密钥为盐的 UID 指纹（HMAC-SHA256(tool_id:uid)）。
///
/// 用于替代无盐 `compute_uid_hash`：即使本地数据库泄露，攻击者也无法在
/// 不知道 `master_key` 的情况下离线枚举/彩虹表反查 UID。旧库通过
/// `QwitchDatabase::migrate_uid_hash` 一次性平滑迁移到该算法。
pub fn compute_uid_hash_hmac(master_key: &[u8; 32], tool_id: &str, uid: &str) -> String {
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(master_key).expect("HMAC-SHA256 接受任意长度密钥");
    mac.update(tool_id.as_bytes());
    mac.update(b":");
    mac.update(uid.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn mask_uid(uid: &str) -> String {
    let chars: Vec<char> = uid.chars().collect();
    if chars.len() <= 6 {
        return "***".to_string();
    }
    let prefix: String = chars.iter().take(4).collect();
    let suffix: String = chars
        .iter()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}****{suffix}")
}

pub fn mask_phone(phone: &str) -> String {
    let digits: Vec<char> = phone.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 7 {
        return "***".to_string();
    }
    let prefix: String = digits.iter().take(3).collect();
    let suffix: String = digits
        .iter()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}****{suffix}")
}

pub fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) => {
            if local.ends_with("***") && local.chars().count() <= 5 {
                return email.to_string();
            }
            let stripped = local.trim_start_matches('*');
            let visible: String = stripped.chars().take(2).collect();
            format!("{visible}***@{domain}")
        }
        None => {
            if email.contains("***") {
                return email.to_string();
            }
            // 无 @ 的"邮箱"字段实际可能是手机号
            if email.chars().all(|c| c.is_ascii_digit()) && email.len() >= 7 {
                mask_phone(email)
            } else {
                "***".to_string()
            }
        }
    }
}

fn redact_kv_secrets(input: &str) -> String {
    const SECRET_KEYS: &[&str] = &[
        "access_token",
        "refresh_token",
        "id_token",
        "accessToken",
        "refreshToken",
        "idToken",
        "authCode",
        "code_verifier",
        "client_secret",
        "authorization",
        "password",
        "verifier",
        "secret",
        "set-cookie",
        "cookie",
        "sessionid_ss",
        "sessionid",
        "sid_tt",
        "sid_guard",
        "uid_tt",
        "sid_ucp_v1",
        "ssid_ucp_v1",
        "odin_tt",
        "passport_csrf_token",
        "token",
        "state",
    ];
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        let at_boundary = i == 0 || (!bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_');
        let rest_bytes = &bytes[i..];
        let matched_key = if at_boundary {
            SECRET_KEYS.iter().find(|&&key| {
                let kb = key.as_bytes();
                if rest_bytes.len() <= kb.len() || !rest_bytes[..kb.len()].eq_ignore_ascii_case(kb)
                {
                    return false;
                }
                let mut j = kb.len();
                if j < rest_bytes.len() && matches!(rest_bytes[j], b'"' | b'\'') {
                    j += 1;
                }
                while j < rest_bytes.len() && rest_bytes[j] == b' ' {
                    j += 1;
                }
                j < rest_bytes.len() && matches!(rest_bytes[j], b'=' | b':')
            })
        } else {
            None
        };

        if let Some(&key) = matched_key {
            let mut j = i + key.len();
            if j < bytes.len() && matches!(bytes[j], b'"' | b'\'') {
                j += 1;
            }
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            // 包含 `=` 或 `:`
            j += 1;
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            let quote = if j < bytes.len() && matches!(bytes[j], b'"' | b'\'') {
                let q = bytes[j];
                j += 1;
                Some(q)
            } else {
                None
            };
            out.push_str(&input[i..j]);
            let is_cookie_header = quote.is_none()
                && (key.eq_ignore_ascii_case("cookie") || key.eq_ignore_ascii_case("set-cookie"));
            let val_start = j;
            while j < bytes.len() {
                let b = bytes[j];
                if let Some(q) = quote {
                    if b == q {
                        break;
                    }
                } else if is_cookie_header {
                    if matches!(b, b'\r' | b'\n') {
                        break;
                    }
                } else if b.is_ascii_whitespace()
                    || matches!(b, b'&' | b',' | b';' | b')' | b']' | b'}' | b'"' | b'\'')
                {
                    break;
                }
                j += 1;
            }
            if j > val_start {
                let val = &input[val_start..j];
                if val.starts_with("[REDACTED") || val == "[JWT_REDACTED]" {
                    out.push_str(val);
                } else if quote.is_none()
                    && key.eq_ignore_ascii_case("authorization")
                    && (val.eq_ignore_ascii_case("bearer")
                        || val.eq_ignore_ascii_case("cloud-ide-jwt")
                        || val.eq_ignore_ascii_case("basic")
                        || val.eq_ignore_ascii_case("token"))
                {
                    let mut cred_start = j;
                    while cred_start < bytes.len() && matches!(bytes[cred_start], b' ' | b'\t') {
                        cred_start += 1;
                    }
                    let mut k = cred_start;
                    while k < bytes.len() {
                        let b = bytes[k];
                        if b.is_ascii_whitespace()
                            || matches!(b, b'&' | b',' | b';' | b')' | b']' | b'}' | b'"' | b'\'')
                        {
                            break;
                        }
                        k += 1;
                    }
                    if k > cred_start {
                        out.push_str(val);
                        out.push_str(&input[j..cred_start]);
                        let cred = &input[cred_start..k];
                        if cred.starts_with("[REDACTED") || cred == "[JWT_REDACTED]" {
                            out.push_str(cred);
                        } else {
                            out.push_str("[REDACTED]");
                        }
                        i = k;
                        continue;
                    }
                    out.push_str(val);
                } else {
                    out.push_str("[REDACTED]");
                }
                i = j;
                continue;
            }
            i = val_start;
            continue;
        }

        if let Some(ch) = input[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

fn redact_auth_schemes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        let rest_bytes = &bytes[i..];
        let matched_scheme_len = ["bearer ", "cloud-ide-jwt "]
            .into_iter()
            .find(|scheme| {
                let sb = scheme.as_bytes();
                rest_bytes.len() >= sb.len() && rest_bytes[..sb.len()].eq_ignore_ascii_case(sb)
            })
            .map(str::len);

        if let Some(prefix_len) = matched_scheme_len {
            let at_boundary = i == 0
                || (!bytes[i - 1].is_ascii_alphanumeric()
                    && bytes[i - 1] != b'-'
                    && bytes[i - 1] != b'_');
            if at_boundary {
                out.push_str(&input[i..i + prefix_len]);
                let mut j = i + prefix_len;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                    out.push(bytes[j] as char);
                    j += 1;
                }
                let tok_start = j;
                while j < bytes.len()
                    && (bytes[j].is_ascii_alphanumeric()
                        || matches!(bytes[j], b'.' | b'_' | b'-' | b'+' | b'/' | b'=' | b'~'))
                {
                    j += 1;
                }
                if j > tok_start {
                    out.push_str("[REDACTED]");
                    i = j;
                    continue;
                }
                i = tok_start;
                continue;
            }
        }

        if let Some(ch) = input[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

fn extract_cn_mobile_digits(token: &str) -> Option<String> {
    if token.contains('*') {
        return None;
    }
    let trimmed = token.strip_prefix('+').unwrap_or(token);
    if !trimmed.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return None;
    }
    let digits: String = trimmed.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 11 && digits.starts_with('1') {
        return Some(digits);
    }
    if digits.len() == 13 && digits.starts_with("861") {
        return Some(digits[2..].to_string());
    }
    None
}

/// 审计日志兜底脱敏：扫描消息中的邮箱/手机号/JWT/认证头片段并打码，
/// 防止用户自定义备注或上游回显报错把 PII/凭据带入日志与导出归档
pub fn sanitize_log_message(message: &str) -> String {
    let pre_redacted = redact_kv_secrets(&redact_auth_schemes(message));
    let mut out = String::with_capacity(pre_redacted.len());
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if token.is_empty() {
            return;
        }
        let t = token.as_str();
        if t.split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
        {
            out.push_str(&mask_email(t));
        } else if t.starts_with("eyJ") && t.len() >= 32 && t.matches('.').count() == 2 {
            out.push_str("[JWT_REDACTED]");
        } else if let Some(mobile) = extract_cn_mobile_digits(t) {
            out.push_str(&mask_phone(&mobile));
        } else {
            out.push_str(t);
        }
        token.clear();
    };
    for ch in pre_redacted.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '%' | '+' | '-' | '@' | '*') {
            token.push(ch);
        } else {
            flush(&mut token, &mut out);
            out.push(ch);
        }
    }
    flush(&mut token, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::CredentialPayload;
    use std::collections::HashMap;

    #[test]
    fn encrypt_decrypt_roundtrip_and_tamper_detection() {
        let key = [7u8; 32];
        let env = CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid: "user_12345678".to_string(),
            display_name: Some("Tester".to_string()),
            display_email: Some("alice@example.com".to_string()),
            payload: CredentialPayload::Token {
                access_token: "secret-token-abc".to_string(),
                refresh_token: Some("refresh-xyz".to_string()),
                expires_at: Some(1_900_000_000),
                extra: HashMap::new(),
            },
        };

        let aad = credential_aad("qoder_cn", "acc-1");
        let mut encrypted = encrypt_credential(&key, &aad, &env).expect("encrypt ok");
        let decrypted = decrypt_credential(&key, &aad, &encrypted).expect("decrypt ok");
        assert_eq!(decrypted.uid, env.uid);
        assert_eq!(decrypted.display_email, env.display_email);

        // 错误 AAD（跨账号密文替换）必须解密失败
        let wrong_aad = credential_aad("qoder_cn", "acc-2");
        assert!(decrypt_credential(&key, &wrong_aad, &encrypted).is_err());

        // 旧格式（无 QWA1 前缀、无 AAD）仍可兼容解密（忽略传入 aad）
        let cipher = Aes256Gcm::new_from_slice(&key).expect("cipher");
        let mut nonce = [0u8; NONCE_LEN];
        nonce[0] = 7;
        let legacy_pt = serde_json::to_vec(&env).expect("serde");
        let legacy_ct = cipher
            .encrypt(Nonce::from_slice(&nonce), legacy_pt.as_ref())
            .expect("legacy encrypt");
        let mut legacy_blob = nonce.to_vec();
        legacy_blob.extend_from_slice(&legacy_ct);
        assert!(decrypt_credential(&key, b"", &legacy_blob).is_ok());

        let last_idx = encrypted.len() - 1;
        encrypted[last_idx] ^= 0xFF;
        assert!(decrypt_credential(&key, &aad, &encrypted).is_err());
    }

    #[test]
    fn uid_hash_hmac_is_salted_deterministic_and_distinct() {
        let k1 = [1u8; 32];
        let k2 = [2u8; 32];
        let a1 = compute_uid_hash_hmac(&k1, "trae_cn", "u_1");
        let a2 = compute_uid_hash_hmac(&k1, "trae_cn", "u_1");
        let b = compute_uid_hash_hmac(&k2, "trae_cn", "u_1");
        let legacy = compute_uid_hash("trae_cn", "u_1");
        // 确定性
        assert_eq!(a1, a2);
        // 不同主密钥 → 不同指纹（加盐）
        assert_ne!(a1, b);
        // 与旧无盐 SHA-256 不同
        assert_ne!(a1, legacy);
        // 64 位十六进制
        assert_eq!(a1.len(), 64);
        assert!(a1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn uid_hash_and_masking_rules() {
        let h1 = compute_uid_hash("trae_cn", "u_12345");
        let h2 = compute_uid_hash("trae_cn", "u_12345");
        let h3 = compute_uid_hash("qoder_cn", "u_12345");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);

        assert_eq!(mask_uid("123456"), "***");
        assert_eq!(mask_uid("u_8f3a91c"), "u_8f****1c");
        assert_eq!(mask_email("alice@example.com"), "al***@example.com");
        assert_eq!(mask_email("not-an-email"), "***");
        assert_eq!(mask_email("13812345678"), "138****5678");
        assert_eq!(mask_phone("13812345678"), "138****5678");
    }

    #[test]
    fn log_sanitizer_masks_email_and_phone_in_free_text() {
        let msg = "已删除账号 alice@example.com (13812345678) 备注 alice@corp.cn";
        let sanitized = sanitize_log_message(msg);
        assert!(!sanitized.contains("alice@example.com"));
        assert!(!sanitized.contains("13812345678"));
        assert!(!sanitized.contains("alice@corp.cn"));
        assert!(sanitized.contains("al***@example.com"));
        assert!(sanitized.contains("138****5678"));
        // 普通文本不受影响
        assert_eq!(
            sanitize_log_message("已导入账号 Qoder 主力"),
            "已导入账号 Qoder 主力"
        );
        // 已脱敏输入不得被二次脱敏破坏（含单字符本地部邮箱 a***@b.com）
        assert_eq!(
            sanitize_log_message("已删除账号 al***@example.com a***@b.com (138****5678)"),
            "已删除账号 al***@example.com a***@b.com (138****5678)"
        );
        assert_eq!(mask_email("a@b.com"), "a***@b.com");
        assert_eq!(mask_email("a***@b.com"), "a***@b.com");

        // 带国码/连字符手机号脱敏
        let phones = sanitize_log_message("联系 +8613812345678 或 8613812345678 或 138-1234-5678");
        assert!(!phones.contains("13812345678"));
        assert!(!phones.contains("138-1234-5678"));
        assert!(phones.contains("138****5678"));

        // Bearer / Cloud-IDE-JWT / 裸 JWT 脱敏
        let secrets = sanitize_log_message(
            "header Authorization: Bearer secret_abc_123, Cloud-IDE-JWT tok-xyz, jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.sig123456",
        );
        assert!(!secrets.contains("secret_abc_123"));
        assert!(!secrets.contains("tok-xyz"));
        assert!(!secrets.contains("sig123456"));
        assert!(secrets.contains("Bearer [REDACTED]"));
        assert!(secrets.contains("Cloud-IDE-JWT [REDACTED]"));
        assert!(secrets.contains("[JWT_REDACTED]"));

        // 复合标签内嵌邮箱/手机号脱敏（同时保留无邮箱意义的 Work @ Office）
        let compound_email = sanitize_log_message("Alice (alice@example.com)");
        assert_eq!(compound_email, "Alice (al***@example.com)");
        let compound_phone = sanitize_log_message("WorkBuddy (13812345678)");
        assert_eq!(compound_phone, "WorkBuddy (138****5678)");
        assert_eq!(sanitize_log_message("Work @ Office"), "Work @ Office");

        // 大文本鉴权头脱敏（验证 O(n) 扫描正确性）
        let padding = "x ".repeat(10_000);
        let big = format!("{padding}Authorization: BeArEr top_secret_999 tail");
        let redacted_big = sanitize_log_message(&big);
        assert!(!redacted_big.contains("top_secret_999"));
        assert!(redacted_big.contains("BeArEr [REDACTED] tail"));

        // 假脱敏前缀邮箱（含 *** 但未真实打码）及 URL/JSON key=value 密钥脱敏
        let masked_fake = sanitize_log_message("账号 ***alice@example.com*** 异常");
        assert!(!masked_fake.contains("alice"));
        assert!(masked_fake.contains("al***@example.com"));

        let url_and_json = sanitize_log_message(
            r#"GET https://example.com/cb?state=st_abc_1&token=tok_xyz_9 {"refresh_token":"rt_secret_77","code_verifier":"cv_999"}"#,
        );
        assert!(!url_and_json.contains("st_abc_1"));
        assert!(!url_and_json.contains("tok_xyz_9"));
        assert!(!url_and_json.contains("rt_secret_77"));
        assert!(!url_and_json.contains("cv_999"));
        assert!(url_and_json.contains("state=[REDACTED]"));
        assert!(url_and_json.contains("token=[REDACTED]"));
        assert!(url_and_json.contains(r#""refresh_token":"[REDACTED]""#));
        assert!(url_and_json.contains(r#""code_verifier":"[REDACTED]""#));

        let basic_auth = sanitize_log_message("header Authorization: Basic dXNlcjpwYXNz ok");
        assert!(!basic_auth.contains("dXNlcjpwYXNz"));
        assert!(basic_auth.contains("Authorization: Basic [REDACTED] ok"));

        let cookie_header = sanitize_log_message(
            "Header Cookie: a=1; sessionid=abc_secret_123; sid_tt=xyz_secret_789",
        );
        assert!(!cookie_header.contains("abc_secret_123"));
        assert!(!cookie_header.contains("xyz_secret_789"));
        assert!(cookie_header.contains("Cookie: [REDACTED]"));

        let standalone_session =
            sanitize_log_message("sessionid_ss=top_ss_999&odin_tt=odin_secret_88");
        assert!(!standalone_session.contains("top_ss_999"));
        assert!(!standalone_session.contains("odin_secret_88"));
        assert!(standalone_session.contains("sessionid_ss=[REDACTED]"));
        assert!(standalone_session.contains("odin_tt=[REDACTED]"));
    }
}
