use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::adapter::AdapterError;
use crate::domain::UnifiedErrorCode;

/// 从 JSON 字段安全提取 UID 字符串（兼容字符串与整型数字，拒绝空串/布尔/对象）
pub fn json_uid_to_string(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        let t = s.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
        return None;
    }
    if let Some(n) = v.as_i64() {
        return Some(n.to_string());
    }
    v.as_u64().map(|n| n.to_string())
}

/// 生成稳定且不泄露原文前缀的兜底 UID（SHA-256 前 12 个十六进制字符）
pub fn hash_fallback_uid(prefix: &str, raw: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw);
    let hex_digest = hex::encode(hasher.finalize());
    format!("{prefix}_{}", &hex_digest[..12])
}

/// 校验服务端返回的路径片段（如 campaignId），防止路径穿越或查询参数注入
pub fn is_valid_path_segment(seg: &str) -> bool {
    !seg.is_empty()
        && seg.len() <= 128
        && !seg.contains("..")
        && seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 严格匹配"今日已签/已领"幂等文案，避免把 "already expired" / "not checked" 误判为已签
pub fn is_idempotent_claimed_phrase(text: &str) -> bool {
    let lower = text.to_lowercase();
    text.contains("已签")
        || text.contains("已领")
        || text.contains("重复领取")
        || lower.contains("already claimed")
        || lower.contains("already checked")
        || lower.contains("checked in")
        || lower.contains("already received")
}

/// HTTP 200 内的业务码统一映射：401/403/429 必须落到可熔断的错误码，
/// 不得退化为 NetworkError 导致调度器继续请求同一平台
pub fn map_business_code(code: i64, context: &str) -> Option<AdapterError> {
    let (unified, hint) = match code {
        401 => (UnifiedErrorCode::AuthExpired, "登录态已过期"),
        403 => (UnifiedErrorCode::RiskTriggered, "触发平台风控或无权限"),
        429 => (UnifiedErrorCode::RateLimited, "请求过于频繁被限流"),
        _ => return None,
    };
    Some(AdapterError::Unified {
        code: unified,
        message: format!("{context}：业务码 {code}（{hint}）"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn business_codes_map_to_circuit_breaker_errors() {
        assert!(matches!(
            map_business_code(401, "ctx"),
            Some(AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                ..
            })
        ));
        assert!(matches!(
            map_business_code(403, "ctx"),
            Some(AdapterError::Unified {
                code: UnifiedErrorCode::RiskTriggered,
                ..
            })
        ));
        assert!(matches!(
            map_business_code(429, "ctx"),
            Some(AdapterError::Unified {
                code: UnifiedErrorCode::RateLimited,
                ..
            })
        ));
        assert!(map_business_code(0, "ctx").is_none());
        assert!(map_business_code(500, "ctx").is_none());
    }

    #[test]
    fn json_uid_hash_path_segment_and_idempotent_phrase_helpers() {
        assert_eq!(
            json_uid_to_string(&serde_json::json!("  u_123  ")).as_deref(),
            Some("u_123")
        );
        assert_eq!(
            json_uid_to_string(&serde_json::json!(987654321)).as_deref(),
            Some("987654321")
        );
        assert_eq!(
            json_uid_to_string(&serde_json::json!(u64::MAX)).as_deref(),
            Some(u64::MAX.to_string().as_str())
        );
        assert!(json_uid_to_string(&serde_json::json!("   ")).is_none());
        assert!(json_uid_to_string(&serde_json::json!(true)).is_none());

        let h1 = hash_fallback_uid("wb_manual", b"eyJhbGciOiJIUzI1NiJ9.aaa");
        let h2 = hash_fallback_uid("wb_manual", b"eyJhbGciOiJIUzI1NiJ9.bbb");
        assert!(h1.starts_with("wb_manual_"));
        assert!(!h1.contains("eyJhbGciOiJI"));
        assert_ne!(h1, h2);

        assert!(is_valid_path_segment("camp_2026-11.v1"));
        assert!(!is_valid_path_segment(""));
        assert!(!is_valid_path_segment("../admin"));
        assert!(!is_valid_path_segment("camp/sub"));
        assert!(!is_valid_path_segment("camp?x=1"));

        assert!(is_idempotent_claimed_phrase("今日已签到"));
        assert!(is_idempotent_claimed_phrase("本周期已领取"));
        assert!(is_idempotent_claimed_phrase("User already claimed today"));
        assert!(is_idempotent_claimed_phrase("Already checked in"));
        assert!(!is_idempotent_claimed_phrase("Token already expired"));
        assert!(!is_idempotent_claimed_phrase("Not checked yet"));
    }
}
