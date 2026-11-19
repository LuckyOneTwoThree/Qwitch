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

fn collect_idempotent_candidates(val: &Value, out: &mut Vec<String>) {
    const MESSAGE_KEYS: &[&str] = &[
        "message",
        "msg",
        "error_msg",
        "errorMessage",
        "errMsg",
        "detail",
        "error",
        "reason",
    ];
    let Some(obj) = val.as_object() else {
        return;
    };
    for key in MESSAGE_KEYS {
        if let Some(v) = obj.get(*key) {
            if let Some(s) = v.as_str() {
                let t = s.trim();
                if !t.is_empty() {
                    out.push(t.to_string());
                }
            } else if v.is_object() {
                collect_idempotent_candidates(v, out);
            }
        }
    }
    if let Some(data) = obj.get("data").filter(|v| v.is_object()) {
        collect_idempotent_candidates(data, out);
    }
}

fn contains_claim_negation_or_failure(text: &str) -> bool {
    const CN_NEGATIONS: &[&str] = &[
        "失败",
        "未签",
        "未领",
        "尚未",
        "没有签",
        "没有领",
        "没签",
        "没领",
        "不可签",
        "无法签",
    ];
    const EN_NEGATIONS: &[&str] = &[
        "not checked",
        "not claimed",
        "not already",
        "haven't checked",
        "hasn't checked",
        "has not checked",
        "have not checked",
        "never checked",
        "never claimed",
        "failed",
        "failure",
    ];
    let lower = text.to_ascii_lowercase();
    CN_NEGATIONS.iter().any(|pat| text.contains(pat))
        || EN_NEGATIONS.iter().any(|pat| lower.contains(pat))
}

fn contains_explicit_claimed_phrase(text: &str) -> bool {
    const CN_POSITIVES: &[&str] = &[
        "今日已签",
        "今天已签",
        "已签到",
        "已经签到",
        "已签过",
        "今日已领",
        "今天已领",
        "已领取",
        "已经领取",
        "已领过",
        "重复签到",
        "重复领取",
        "请勿重复领",
        "请勿重复签",
        "不可重复领",
        "不可重复签",
    ];
    const EN_POSITIVES: &[&str] = &[
        "already claimed",
        "already checked in",
        "already checked",
        "already received",
        "duplicate claim",
        "repeated claim",
    ];
    let lower = text.to_ascii_lowercase();
    CN_POSITIVES.iter().any(|pat| text.contains(pat))
        || EN_POSITIVES.iter().any(|pat| lower.contains(pat))
}

/// 严格匹配"今日已签/已领"幂等文案，排除 "not checked in" / "not already claimed" / "已签到失败" 等否定或失败消息
pub fn is_idempotent_claimed_phrase(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    let mut extracted = Vec::new();
    if trimmed.starts_with('{') {
        if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
            collect_idempotent_candidates(&val, &mut extracted);
        }
    }
    let candidates: Vec<&str> = if extracted.is_empty() {
        vec![trimmed]
    } else {
        extracted.iter().map(String::as_str).collect()
    };

    if candidates
        .iter()
        .any(|msg| contains_claim_negation_or_failure(msg))
    {
        return false;
    }
    candidates
        .iter()
        .any(|msg| contains_explicit_claimed_phrase(msg))
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
        assert!(is_idempotent_claimed_phrase("今日已领，无需重复签到"));
        assert!(is_idempotent_claimed_phrase("User already claimed today"));
        assert!(is_idempotent_claimed_phrase("Already checked in"));
        assert!(is_idempotent_claimed_phrase(
            r#"{"code":10001,"error":"Already claimed"}"#
        ));
        assert!(!is_idempotent_claimed_phrase("Token already expired"));
        assert!(!is_idempotent_claimed_phrase("Not checked yet"));
        assert!(!is_idempotent_claimed_phrase("Not checked in"));
        assert!(!is_idempotent_claimed_phrase("User has not checked in"));
        assert!(!is_idempotent_claimed_phrase("Not already claimed"));
        assert!(!is_idempotent_claimed_phrase("已签到失败"));
        assert!(!is_idempotent_claimed_phrase("尚未领取"));
        assert!(!is_idempotent_claimed_phrase(
            r#"{"message":"claim failed"}"#
        ));
        assert!(!is_idempotent_claimed_phrase(
            r#"{"error":"Already claimed","message":"User has not checked in"}"#
        ));
    }
}
