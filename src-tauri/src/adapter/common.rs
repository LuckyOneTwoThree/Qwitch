use crate::adapter::AdapterError;
use crate::domain::UnifiedErrorCode;

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
}
