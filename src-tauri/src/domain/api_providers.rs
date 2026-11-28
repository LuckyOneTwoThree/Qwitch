use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::{UpstreamModelOption, WireProtocolKind};

pub const API_PROVIDER_PREFIX: &str = "api_";

pub fn is_api_provider_tool_id(tool_id: &str) -> bool {
    tool_id.starts_with(API_PROVIDER_PREFIX)
}

/// 将用户输入的名称或标识规范化为 `api_<slug>`（仅小写字母、数字、下划线，总长 <= 48）。
pub fn normalize_api_provider_slug(raw: &str) -> String {
    let trimmed = raw.trim().to_ascii_lowercase();
    let stripped = trimmed
        .strip_prefix(API_PROVIDER_PREFIX)
        .unwrap_or(&trimmed);

    let mut slug = String::new();
    let mut prev_underscore = false;
    for ch in stripped.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            prev_underscore = false;
        } else if !prev_underscore && !slug.is_empty() {
            slug.push('_');
            prev_underscore = true;
        }
    }
    let slug = slug.trim_matches('_');
    let fallback = if slug.is_empty() { "custom" } else { slug };
    let max_body = 48usize.saturating_sub(API_PROVIDER_PREFIX.len());
    let truncated: String = fallback.chars().take(max_body).collect();
    let clean = truncated.trim_end_matches('_');
    if clean.is_empty() {
        format!("{API_PROVIDER_PREFIX}custom")
    } else {
        format!("{API_PROVIDER_PREFIX}{clean}")
    }
}

/// 脱敏展示 API Key，绝不向前端或日志暴露完整密钥。
pub fn mask_api_key(key: &str) -> String {
    let trimmed = key.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    let len = chars.len();
    if len == 0 {
        return "••••".to_string();
    }
    if len > 10 {
        let prefix: String = chars[..5].iter().collect();
        let suffix: String = chars[len - 4..].iter().collect();
        return format!("{prefix}•••{suffix}");
    }
    if len > 4 {
        let prefix: String = chars[..2].iter().collect();
        let suffix: String = chars[len - 2..].iter().collect();
        return format!("{prefix}•••{suffix}");
    }
    "••••".to_string()
}

/// 根据 Base URL 与显式上游协议构造最终推理端点：
/// - 若 `base_url` 以 `#` 结尾，去掉末尾 `#` 原样返回（严格路径模式）；
/// - 否则去掉末尾 `/`，若路径中已包含 `v<数字>` 版本段则直接追加端点，否则自动补 `/v1`。
pub fn build_upstream_endpoint(base_url: &str, protocol: WireProtocolKind) -> String {
    let trimmed = base_url.trim();
    if let Some(strict) = trimmed.strip_suffix('#') {
        return strict.trim_end_matches('/').to_string();
    }
    let base = trimmed.trim_end_matches('/');
    let path_part = base
        .split_once("://")
        .map(|(_, rest)| rest.split_once('/').map(|(_, p)| p).unwrap_or(""))
        .unwrap_or(base);
    let has_version = path_part.split('/').any(|seg| {
        let s = seg.to_ascii_lowercase();
        s.len() > 1 && s.starts_with('v') && s[1..].starts_with(|c: char| c.is_ascii_digit())
    });
    let suffix = match protocol {
        WireProtocolKind::OpenaiChat => "chat/completions",
        WireProtocolKind::AnthropicMessages => "messages",
        WireProtocolKind::OpenaiResponses => "responses",
    };
    if has_version {
        format!("{base}/{suffix}")
    } else {
        format!("{base}/v1/{suffix}")
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApiAuthScheme {
    #[default]
    Bearer,
    XApiKey,
    CustomHeader,
}

impl ApiAuthScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bearer => "bearer",
            Self::XApiKey => "x_api_key",
            Self::CustomHeader => "custom_header",
        }
    }

    pub fn from_str_lossy(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "x_api_key" | "x-api-key" => Self::XApiKey,
            "custom_header" | "custom-header" => Self::CustomHeader,
            _ => Self::Bearer,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApiManualModelEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_wire_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiProviderRecord {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_id: Option<String>,
    pub base_url: String,
    pub wire_protocol: WireProtocolKind,
    pub auth_scheme: ApiAuthScheme,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_header_name: Option<String>,
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_path: Option<String>,
    #[serde(default)]
    pub manual_models: Vec<ApiManualModelEntry>,
    #[serde(default)]
    pub allow_lan_http: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_test_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_test_latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_test_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tested_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveApiProviderInput {
    pub id: Option<String>,
    pub name: String,
    pub preset_id: Option<String>,
    pub base_url: String,
    pub wire_protocol: WireProtocolKind,
    #[serde(default)]
    pub auth_scheme: ApiAuthScheme,
    pub auth_header_name: Option<String>,
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
    pub models_path: Option<String>,
    #[serde(default)]
    pub manual_models: Vec<ApiManualModelEntry>,
    #[serde(default)]
    pub allow_lan_http: bool,
    pub accent_color: Option<String>,
    pub icon: Option<String>,
    pub enabled: Option<bool>,
    /// 创建时可选附带第一把 API Key
    pub initial_api_key: Option<String>,
    pub initial_key_label: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddApiProviderKeyInput {
    pub provider_id: String,
    pub label: Option<String>,
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateApiProviderKeyInput {
    pub account_id: String,
    pub label: Option<String>,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiProviderTestInput {
    pub provider_id: Option<String>,
    pub base_url: String,
    pub wire_protocol: WireProtocolKind,
    #[serde(default)]
    pub auth_scheme: ApiAuthScheme,
    pub auth_header_name: Option<String>,
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
    pub models_path: Option<String>,
    #[serde(default)]
    pub allow_lan_http: bool,
    /// 测试连接时优先使用传入的明文 Key；若为空且传了 `provider_id`，后端从库中取该厂商首把可用 Key
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiProviderTestResult {
    pub ok: bool,
    pub latency_ms: u64,
    pub discovered_models: Vec<UpstreamModelOption>,
    pub model_count: usize,
    pub endpoint_preview: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_slug_formats_valid_api_provider_id() {
        assert_eq!(normalize_api_provider_slug("DeepSeek"), "api_deepseek");
        assert_eq!(
            normalize_api_provider_slug("智谱 Coding Plan"),
            "api_coding_plan"
        );
        assert_eq!(
            normalize_api_provider_slug("api_openrouter"),
            "api_openrouter"
        );
        assert_eq!(normalize_api_provider_slug("   "), "api_custom");
    }

    #[test]
    fn mask_api_key_hides_middle_secret() {
        assert_eq!(mask_api_key("sk-1234567890abcdef"), "sk-12•••cdef");
        assert_eq!(mask_api_key("short"), "sh•••rt");
        assert_eq!(mask_api_key("123"), "••••");
    }

    #[test]
    fn build_upstream_endpoint_respects_version_segments_and_strict_hash() {
        assert_eq!(
            build_upstream_endpoint("https://api.deepseek.com", WireProtocolKind::OpenaiChat),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            build_upstream_endpoint("https://openrouter.ai/api/v1", WireProtocolKind::OpenaiChat),
            "https://openrouter.ai/api/v1/chat/completions"
        );
        assert_eq!(
            build_upstream_endpoint(
                "https://ark.cn-beijing.volces.com/api/v3/",
                WireProtocolKind::OpenaiChat
            ),
            "https://ark.cn-beijing.volces.com/api/v3/chat/completions"
        );
        assert_eq!(
            build_upstream_endpoint(
                "https://open.bigmodel.cn/api/paas/v4",
                WireProtocolKind::OpenaiChat
            ),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
        assert_eq!(
            build_upstream_endpoint(
                "https://api.anthropic.com",
                WireProtocolKind::AnthropicMessages
            ),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            build_upstream_endpoint("https://api.openai.com/v1", WireProtocolKind::OpenaiResponses),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            build_upstream_endpoint(
                "https://relay.example.com/custom/chat#",
                WireProtocolKind::OpenaiChat
            ),
            "https://relay.example.com/custom/chat"
        );
    }
}
