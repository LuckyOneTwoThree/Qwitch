use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::domain::CredentialEnvelope;
use crate::storage::StorageError;

use std::ptr::write_volatile;
use std::sync::atomic::{compiler_fence, Ordering};

const NONCE_LEN: usize = 12;

fn zeroize_bytes(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: b 是有效可写的可变引用字节，使用 volatile 防止编译器优化消除清零
        unsafe {
            write_volatile(b, 0);
        }
    }
    compiler_fence(Ordering::SeqCst);
}

pub fn encrypt_credential(
    master_key: &[u8; 32],
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
        .encrypt(nonce, plaintext.as_ref())
        .map_err(|e| StorageError::Crypto(format!("Encryption failed: {e}")));
    zeroize_bytes(&mut plaintext);
    let ciphertext = ciphertext_res?;

    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub fn decrypt_credential(
    master_key: &[u8; 32],
    blob: &[u8],
) -> Result<CredentialEnvelope, StorageError> {
    if blob.len() <= NONCE_LEN {
        return Err(StorageError::Crypto("Encrypted blob is too short".into()));
    }

    let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);
    let cipher =
        Aes256Gcm::new_from_slice(master_key).map_err(|e| StorageError::Crypto(e.to_string()))?;
    let nonce = Nonce::from_slice(nonce_bytes);

    let mut plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| StorageError::Crypto(format!("Decryption failed: {e}")))?;

    let parsed =
        serde_json::from_slice(&plaintext).map_err(|e| StorageError::Crypto(e.to_string()));
    zeroize_bytes(&mut plaintext);
    parsed
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
        "code_verifier",
        "client_secret",
        "authorization",
        "password",
        "verifier",
        "secret",
        "cookie",
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
            let val_start = j;
            while j < bytes.len() {
                let b = bytes[j];
                if let Some(q) = quote {
                    if b == q {
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

        let mut encrypted = encrypt_credential(&key, &env).expect("encrypt ok");
        let decrypted = decrypt_credential(&key, &encrypted).expect("decrypt ok");
        assert_eq!(decrypted.uid, env.uid);
        assert_eq!(decrypted.display_email, env.display_email);

        let last_idx = encrypted.len() - 1;
        encrypted[last_idx] ^= 0xFF;
        assert!(decrypt_credential(&key, &encrypted).is_err());
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
    }
}
