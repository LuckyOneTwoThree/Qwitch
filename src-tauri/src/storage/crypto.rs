use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::domain::CredentialEnvelope;
use crate::storage::StorageError;

const NONCE_LEN: usize = 12;

pub fn encrypt_credential(
    master_key: &[u8; 32],
    envelope: &CredentialEnvelope,
) -> Result<Vec<u8>, StorageError> {
    let plaintext =
        serde_json::to_vec(envelope).map_err(|e| StorageError::Crypto(e.to_string()))?;

    let cipher =
        Aes256Gcm::new_from_slice(master_key).map_err(|e| StorageError::Crypto(e.to_string()))?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_ref())
        .map_err(|e| StorageError::Crypto(format!("Encryption failed: {e}")))?;

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

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| StorageError::Crypto(format!("Decryption failed: {e}")))?;

    serde_json::from_slice(&plaintext).map_err(|e| StorageError::Crypto(e.to_string()))
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
    if email.contains("***") {
        return email.to_string();
    }
    match email.split_once('@') {
        Some((local, domain)) => {
            let visible: String = local.chars().take(2).collect();
            format!("{visible}***@{domain}")
        }
        None => {
            // 无 @ 的"邮箱"字段实际可能是手机号
            if email.chars().all(|c| c.is_ascii_digit()) && email.len() >= 7 {
                mask_phone(email)
            } else {
                "***".to_string()
            }
        }
    }
}

fn redact_auth_schemes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        let rest = &input[i..];
        let rest_lower = rest.to_ascii_lowercase();
        let matched_scheme = ["bearer ", "cloud-ide-jwt "]
            .into_iter()
            .find(|scheme| rest_lower.starts_with(scheme));

        if let Some(scheme) = matched_scheme {
            let at_boundary = i == 0
                || !bytes[i - 1].is_ascii_alphanumeric()
                    && bytes[i - 1] != b'-'
                    && bytes[i - 1] != b'_';
            if at_boundary {
                let prefix_len = scheme.len();
                out.push_str(&rest[..prefix_len]);
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

        if let Some(ch) = rest.chars().next() {
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
    let pre_redacted = redact_auth_schemes(message);
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
    }
}
