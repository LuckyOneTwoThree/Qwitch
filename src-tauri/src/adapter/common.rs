use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use std::time::Duration;

use crate::adapter::AdapterError;
use crate::domain::{ClaimStatus, QuotaSnapshot, UnifiedErrorCode};

/// 内置适配器共享的 HTTP 客户端（连接池复用 + 20s 请求超时）
pub fn shared_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// 网关推理专用 HTTP 客户端：长连接/长生成需要远超配额查询的超时。
///
/// 复用 20s 超时的 `shared_http_client` 会把耗时 >20s 的长代码补全/长生成直接判失败
/// （网关随后回 502）。这里放宽总超时到 600s、连接超时 15s，与 Agent 端
/// `streamIdleTimeoutMs: 600000` 对齐。
pub fn long_running_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// 统一 HTTP 状态码到 `AdapterError` 的映射（401 过期 / 403 风控 / 429 限流 / 其余网络异常）
pub fn map_http_status(status: reqwest::StatusCode, context: &str) -> AdapterError {
    match status.as_u16() {
        401 => AdapterError::Unified {
            code: UnifiedErrorCode::AuthExpired,
            message: format!("{context}: 登录态已过期 (HTTP 401)"),
        },
        403 => AdapterError::Unified {
            code: UnifiedErrorCode::RiskTriggered,
            message: format!("{context}: 服务端拒绝访问 (HTTP 403)"),
        },
        429 => AdapterError::Unified {
            code: UnifiedErrorCode::RateLimited,
            message: format!("{context}: 请求过于频繁 (HTTP 429)"),
        },
        code => AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("{context}: HTTP {code}"),
        },
    }
}

/// 从 JSON 字段安全提取 UID 字符串（兼容正整型数字与非空非哨兵字符串，拒绝 `0` / `-1` / 布尔 / 对象）
pub fn json_uid_to_string(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        let t = s.trim();
        if !t.is_empty() && !matches!(t, "0" | "-1") {
            return Some(t.to_string());
        }
        return None;
    }
    if let Some(n) = v.as_i64() {
        return (n > 0).then(|| n.to_string());
    }
    v.as_u64().and_then(|n| (n > 0).then(|| n.to_string()))
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
        && seg != "."
        && !seg.starts_with('.')
        && !seg.ends_with('.')
        && !seg.contains("..")
        && seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn collect_idempotent_candidates(val: &Value, out: &mut Vec<String>) {
    const MESSAGE_KEYS: &[&str] = &[
        "message",
        "msg",
        "status_msg",
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
    for nested_key in ["data", "base_resp"] {
        if let Some(nested) = obj.get(nested_key).filter(|v| v.is_object()) {
            collect_idempotent_candidates(nested, out);
        }
    }
}

fn contains_claim_negation(text: &str) -> bool {
    const CN_NEGATIONS: &[&str] = &[
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
    ];
    let lower = text.to_ascii_lowercase();
    CN_NEGATIONS.iter().any(|pat| text.contains(pat))
        || EN_NEGATIONS.iter().any(|pat| lower.contains(pat))
}

fn contains_clause_failure(clause: &str) -> bool {
    let lower = clause.to_ascii_lowercase();
    clause.contains("失败")
        || lower.contains("failed")
        || lower.contains("failure")
        || lower.contains("expired")
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

/// 严格匹配"今日已签/已领"幂等文案，排除 "not checked in" / "not already claimed" / "已签到失败" 等否定或失败消息，
/// 同时兼容 `"签到失败：今日已签到"` 这类前置提示与 MiniMax `base_resp.status_msg`。
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

    if candidates.iter().any(|msg| contains_claim_negation(msg)) {
        return false;
    }
    candidates.iter().any(|msg| {
        msg.split(['：', ':', '，', ',', '；', ';', '\n'])
            .map(str::trim)
            .filter(|clause| !clause.is_empty())
            .any(|clause| {
                !contains_clause_failure(clause) && contains_explicit_claimed_phrase(clause)
            })
    })
}

/// 领取成功后合并额度快照（三个适配器共用）。
///
/// 语义：
/// - 有领后快照：若服务端返回 `Unknown`/`Unsupported` 但领取动作已确认成功，
///   合成为 `Claimed`，避免 UI 残留「可领」按钮；
/// - 无领后快照但领取已确认：用前置快照的套餐与桶信息合成一条 `Claimed` 快照，
///   保证状态闭合（否则调度器可能重复尝试领取）；
/// - 其余情况返回 `None`，交由调用方保持原状。
///
/// 历史实现中 `trae.rs` / `qoder.rs` / `minimax.rs` 各有一份**字节级相同**的副本，
/// 已收敛到此处；任何语义调整只需改这一处，避免继续分叉。
pub fn merge_post_claim_quota(
    inspected: Option<QuotaSnapshot>,
    precheck: Option<&QuotaSnapshot>,
    confirmed_claimed: bool,
) -> Option<QuotaSnapshot> {
    match inspected {
        Some(mut q) => {
            if confirmed_claimed
                && matches!(
                    q.claim_status,
                    ClaimStatus::Unknown | ClaimStatus::Unsupported
                )
            {
                q.claim_status = ClaimStatus::Claimed;
            }
            Some(q)
        }
        None if confirmed_claimed => Some(QuotaSnapshot {
            plan_name: precheck.and_then(|p| p.plan_name.clone()),
            claim_status: ClaimStatus::Claimed,
            buckets: precheck.map(|p| p.buckets.clone()).unwrap_or_default(),
            fetched_at: chrono::Utc::now().to_rfc3339(),
        }),
        None => None,
    }
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
        assert!(json_uid_to_string(&serde_json::json!(0)).is_none());
        assert!(json_uid_to_string(&serde_json::json!(-1)).is_none());
        assert!(json_uid_to_string(&serde_json::json!("0")).is_none());
        assert!(json_uid_to_string(&serde_json::json!("-1")).is_none());

        let h1 = hash_fallback_uid("wb_manual", b"eyJhbGciOiJIUzI1NiJ9.aaa");
        let h2 = hash_fallback_uid("wb_manual", b"eyJhbGciOiJIUzI1NiJ9.bbb");
        assert!(h1.starts_with("wb_manual_"));
        assert!(!h1.contains("eyJhbGciOiJI"));
        assert_ne!(h1, h2);

        assert!(is_valid_path_segment("camp_2026-11.v1"));
        assert!(!is_valid_path_segment(""));
        assert!(!is_valid_path_segment("."));
        assert!(!is_valid_path_segment(".hidden"));
        assert!(!is_valid_path_segment("camp."));
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
        assert!(is_idempotent_claimed_phrase("签到失败：今日已签到"));
        assert!(is_idempotent_claimed_phrase(
            r#"{"base_resp":{"status_code":1001,"status_msg":"今日已签到"}}"#
        ));
        assert!(!is_idempotent_claimed_phrase(
            r#"{"message":"claim failed"}"#
        ));
        assert!(!is_idempotent_claimed_phrase(
            r#"{"error":"Already claimed","message":"User has not checked in"}"#
        ));
    }
}

#[cfg(test)]
mod http_mapping_integration_tests {
    use super::{map_business_code, map_http_status};
    use crate::domain::UnifiedErrorCode;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// `map_http_status` 是 20 个调用点的错误码唯一来源，此前零测试。
    /// 这里用 wiremock 起真实 HTTP 服务，验证状态码 → 统一错误码的映射矩阵。
    #[tokio::test]
    async fn http_status_matrix_maps_to_unified_error_codes() {
        let server = MockServer::start().await;
        let client = super::shared_http_client();

        let cases: &[(u16, UnifiedErrorCode)] = &[
            (401, UnifiedErrorCode::AuthExpired),
            (403, UnifiedErrorCode::RiskTriggered),
            (429, UnifiedErrorCode::RateLimited),
            (500, UnifiedErrorCode::NetworkError),
            (502, UnifiedErrorCode::NetworkError),
            (503, UnifiedErrorCode::NetworkError),
        ];

        for (status, expected) in cases {
            Mock::given(method("GET"))
                .and(path(format!("/status/{status}")))
                .respond_with(ResponseTemplate::new(*status))
                .mount(&server)
                .await;

            let resp = client
                .get(format!("{}/status/{status}", server.uri()))
                .send()
                .await
                .expect("request");
            assert_eq!(resp.status().as_u16(), *status);

            let err = map_http_status(resp.status(), "测试上下文");
            match err {
                crate::adapter::AdapterError::Unified { code, .. } => {
                    assert_eq!(code, *expected, "HTTP {status} 应映射为 {expected:?}");
                }
            }
        }
    }

    /// 200 但业务码表示失败：必须落到可熔断的错误码，不得退化为 NetworkError。
    #[tokio::test]
    async fn business_code_matrix_keeps_circuit_breaker_semantics() {
        assert_eq!(
            map_business_code(401, "ctx").map(|e| match e {
                crate::adapter::AdapterError::Unified { code, .. } => code,
            }),
            Some(UnifiedErrorCode::AuthExpired)
        );
        assert_eq!(
            map_business_code(403, "ctx").map(|e| match e {
                crate::adapter::AdapterError::Unified { code, .. } => code,
            }),
            Some(UnifiedErrorCode::RiskTriggered)
        );
        assert_eq!(
            map_business_code(429, "ctx").map(|e| match e {
                crate::adapter::AdapterError::Unified { code, .. } => code,
            }),
            Some(UnifiedErrorCode::RateLimited)
        );
        // 未知业务码不映射（交由调用方处理）
        assert!(map_business_code(0, "ctx").is_none());
    }

    /// 非 JSON 响应体不应被当作成功：验证错误路径不会 panic 且能区分内容类型。
    #[tokio::test]
    async fn non_json_success_body_is_not_silently_accepted() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/html"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<html>WAF blocked</html>")
                    .insert_header("content-type", "text/html"),
            )
            .mount(&server)
            .await;

        let resp = super::shared_http_client()
            .get(format!("{}/html", server.uri()))
            .send()
            .await
            .expect("request");
        assert!(resp.status().is_success());

        // 解析 JSON 必须失败，调用方据此判定为异常而非「已签到」
        let parsed = resp.json::<serde_json::Value>().await;
        assert!(parsed.is_err(), "HTML 响应不得被解析为 JSON 成功体");
    }
}
