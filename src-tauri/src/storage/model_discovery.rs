use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use crate::domain::{
    dedup_model_options, default_model_options_for_tool, normalize_upstream_model,
    CredentialEnvelope, CredentialPayload, UpstreamModelOption,
};

fn discovery_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
        .map_err(|e| format!("构造模型探测 HTTP 客户端失败: {e}"))
}

fn is_noise_variant_id(raw_id: &str) -> bool {
    let lower = raw_id.to_ascii_lowercase();
    lower.is_empty()
        || lower == "default"
        || lower.ends_with("-max")
        || lower.contains("_advisor")
        || lower.ends_with("-auto")
        || lower.contains("subagent")
        || lower.contains("sub_agent")
        || lower.contains("search_agent")
        || lower == "sagitta"
        || lower == "aquila"
        || lower == "doubao-for-auto"
        || lower == "minimax-image"
        || lower.starts_with("minimax-h3")
        || lower.starts_with("minimax-hailuo")
}

fn as_opt_f64(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// 从本地 `~/.config/magpie/plugin-providers.json` 读取已探测的线上模型目录（二级真实目录缓存）
pub fn load_magpie_provider_models(tool_id: &str) -> Option<Vec<UpstreamModelOption>> {
    let provider_id = match tool_id {
        "workbuddy" => "workbuddy",
        "qoder_cn" => "qoder-cn",
        "trae_cn" => "trae-cn",
        "minimax_code_cn" => "minimax-code",
        _ => return None,
    };

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))?;
    let path = home.join(".config/magpie/plugin-providers.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let root: Value = serde_json::from_str(&raw).ok()?;
    let providers = root.as_array()?;

    let entry = providers
        .iter()
        .find(|item| item.get("id").and_then(|v| v.as_str()) == Some(provider_id))?;
    let models = entry.get("models").and_then(|v| v.as_array())?;

    let mut parsed = Vec::new();
    for m in models {
        let Some(raw_id) = m.get("id").and_then(|v| v.as_str()).map(str::trim) else {
            continue;
        };
        if is_noise_variant_id(raw_id) {
            continue;
        }
        let display = m.get("name").and_then(|v| v.as_str());
        let rate = as_opt_f64(m.get("rate"));
        let is_free = m.get("free").and_then(|v| v.as_bool()).unwrap_or(false)
            || rate.is_some_and(|r| r == 0.0 && raw_id.contains("free"));
        parsed.push(normalize_upstream_model(
            tool_id,
            raw_id,
            display,
            rate,
            is_free,
            "live",
        ));
    }

    let deduped = dedup_model_options(parsed);
    if deduped.is_empty() {
        None
    } else {
        Some(deduped)
    }
}

/// 解析 WorkBuddy `/v3/config` 真实响应中的 CLI 可用模型列表
pub fn parse_workbuddy_v3_config(body: &Value) -> Vec<UpstreamModelOption> {
    let data = body.get("data").unwrap_or(body);
    let mut cli_model_ids: Vec<String> = Vec::new();
    if let Some(agents) = data.get("agents").and_then(|v| v.as_array()) {
        for ag in agents {
            let name = ag.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name.eq_ignore_ascii_case("cli") || cli_model_ids.is_empty() {
                if let Some(arr) = ag.get("models").and_then(|v| v.as_array()) {
                    let ids: Vec<String> = arr
                        .iter()
                        .filter_map(|v| {
                            v.as_str()
                                .map(ToOwned::to_owned)
                                .or_else(|| v.get("id").and_then(|x| x.as_str()).map(ToOwned::to_owned))
                        })
                        .collect();
                    if name.eq_ignore_ascii_case("cli") {
                        cli_model_ids = ids;
                        break;
                    } else if !ids.is_empty() {
                        cli_model_ids = ids;
                    }
                }
            }
        }
    }

    let mut details_by_id: HashMap<String, &Value> = HashMap::new();
    let mut ordered_models: Vec<(String, Option<&Value>)> = Vec::new();

    match data.get("models") {
        Some(Value::Array(arr)) => {
            for item in arr {
                if let Some(id) = item
                    .get("id")
                    .or_else(|| item.get("modelId"))
                    .and_then(|v| v.as_str())
                {
                    details_by_id.insert(id.to_string(), item);
                    if cli_model_ids.is_empty() {
                        ordered_models.push((id.to_string(), Some(item)));
                    }
                }
            }
        }
        Some(Value::Object(map)) => {
            for (k, item) in map {
                details_by_id.insert(k.clone(), item);
                if cli_model_ids.is_empty() {
                    ordered_models.push((k.clone(), Some(item)));
                }
            }
        }
        _ => {}
    }

    if !cli_model_ids.is_empty() {
        for id in cli_model_ids {
            let detail = details_by_id.get(&id).copied();
            ordered_models.push((id, detail));
        }
    }

    let mut out = Vec::new();
    for (raw_id, detail_opt) in ordered_models {
        if raw_id.trim().is_empty() {
            continue;
        }
        let enabled = detail_opt
            .and_then(|d| d.get("enabled").or_else(|| d.get("enable")))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !enabled {
            continue;
        }
        let display = detail_opt.and_then(|d| {
            d.get("name")
                .or_else(|| d.get("displayName"))
                .or_else(|| d.get("modelName"))
                .and_then(|v| v.as_str())
        });
        let rate = detail_opt.and_then(|d| {
            as_opt_f64(
                d.get("rate")
                    .or_else(|| d.get("creditRate"))
                    .or_else(|| d.get("multiplier"))
                    .or_else(|| d.get("priceFactor")),
            )
        });
        let is_free = detail_opt
            .and_then(|d| d.get("free").or_else(|| d.get("isFree")))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || rate.is_some_and(|r| r == 0.0 && raw_id.contains("free"));
        out.push(normalize_upstream_model(
            "workbuddy",
            &raw_id,
            display,
            rate,
            is_free,
            "live",
        ));
    }

    dedup_model_options(out)
}

/// 解析 Qoder CN `/algo/api/v2/model/list?Encode=1` 真实响应
pub fn parse_qoder_model_list(body: &Value) -> Vec<UpstreamModelOption> {
    let listing = body.get("data").unwrap_or(body);
    let mut combined: Vec<&Value> = Vec::new();
    let mut seen_keys = HashSet::new();

    for scene in ["app", "chat"] {
        if let Some(arr) = listing.get(scene).and_then(|v| v.as_array()) {
            for item in arr {
                let Some(key) = item.get("key").and_then(|v| v.as_str()).map(str::trim) else {
                    continue;
                };
                if key.is_empty() || key == "auto" || key == "default" {
                    continue;
                }
                if seen_keys.insert(key.to_string()) {
                    combined.push(item);
                }
            }
        }
    }

    let mut out = Vec::new();
    for item in combined {
        let enabled = item.get("enable").and_then(|v| v.as_bool()).unwrap_or(true);
        if !enabled {
            continue;
        }
        let Some(key) = item.get("key").and_then(|v| v.as_str()).map(str::trim) else {
            continue;
        };
        let display = item.get("display_name").and_then(|v| v.as_str());
        let price = as_opt_f64(item.get("price_factor").or_else(|| item.get("priceFactor")));
        let promo = item.get("promotion").or_else(|| item.get("prommotion"));
        let promo_active = promo
            .and_then(|p| p.get("active"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let promo_before = promo
            .and_then(|p| as_opt_f64(p.get("before_promotion_price_factor").or_else(|| p.get("beforePromotionPriceFactor"))));
        let is_free = match price {
            Some(p) if p == 0.0 => !(promo_active && promo_before.is_some_and(|b| b > 0.0)),
            Some(_) => false,
            None => item
                .get("is_free")
                .or_else(|| item.get("isFree"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        };
        out.push(normalize_upstream_model(
            "qoder_cn",
            key,
            display,
            price,
            is_free,
            "live",
        ));
    }

    dedup_model_options(out)
}

fn collect_trae_model_objects(val: &Value, out: &mut Vec<UpstreamModelOption>) {
    match val {
        Value::String(s) => {
            let t = s.trim();
            if (t.starts_with('{') || t.starts_with('[')) && t.len() > 2 {
                if let Ok(nested) = serde_json::from_str::<Value>(t) {
                    collect_trae_model_objects(&nested, out);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr {
                if let Some(obj) = item.as_object() {
                    let id_opt = obj
                        .get("model_name")
                        .or_else(|| obj.get("model"))
                        .or_else(|| obj.get("id"))
                        .or_else(|| obj.get("name"))
                        .and_then(|v| v.as_str())
                        .map(str::trim);
                    if let Some(raw_id) = id_opt {
                        if !is_noise_variant_id(raw_id) && !raw_id.contains(' ') {
                            let enabled = obj
                                .get("enabled")
                                .or_else(|| obj.get("enable"))
                                .or_else(|| obj.get("available"))
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true);
                            if enabled {
                                let display = obj
                                    .get("display_name")
                                    .or_else(|| obj.get("displayName"))
                                    .or_else(|| obj.get("name"))
                                    .and_then(|v| v.as_str());
                                let rate = as_opt_f64(
                                    obj.get("rate")
                                        .or_else(|| obj.get("price_factor"))
                                        .or_else(|| obj.get("credit_cost")),
                                );
                                let is_free = obj
                                    .get("free")
                                    .or_else(|| obj.get("is_free"))
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false);
                                out.push(normalize_upstream_model(
                                    "trae_cn",
                                    raw_id,
                                    display,
                                    rate,
                                    is_free,
                                    "live",
                                ));
                            }
                        }
                    }
                }
                collect_trae_model_objects(item, out);
            }
        }
        Value::Object(map) => {
            for v in map.values() {
                collect_trae_model_objects(v, out);
            }
        }
        _ => {}
    }
}

/// 解析 Trae CN `/api/ide/v1/batch_get_detail_param` 响应
pub fn parse_trae_detail_param(body: &Value) -> Vec<UpstreamModelOption> {
    let mut raw = Vec::new();
    collect_trae_model_objects(body, &mut raw);
    dedup_model_options(raw)
}

/// 解析 MiniMax Code CN `/mavis/api/v1/models` 响应
pub fn parse_minimax_models(body: &Value) -> Vec<UpstreamModelOption> {
    let root = body.get("data").unwrap_or(body);
    let providers = root
        .get("providers")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut out = Vec::new();
    for prov in providers {
        let pname = prov.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if !pname.is_empty() && pname != "minimax" {
            continue;
        }
        if let Some(models) = prov.pointer("/config/models").and_then(|v| v.as_array()) {
            for m in models {
                let Some(raw_id) = m.get("id").and_then(|v| v.as_str()).map(str::trim) else {
                    continue;
                };
                if is_noise_variant_id(raw_id) {
                    continue;
                }
                let kind = m.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if matches!(kind, "image" | "video" | "speech" | "audio") {
                    continue;
                }
                let display = m.get("name").and_then(|v| v.as_str());
                out.push(normalize_upstream_model(
                    "minimax_code_cn",
                    raw_id,
                    display,
                    Some(0.0),
                    false,
                    "live",
                ));
            }
        }
    }
    dedup_model_options(out)
}

/// 解析通用 OpenAI / Anthropic `/v1/models` 响应
pub fn parse_openai_models_list(tool_id: &str, body: &Value) -> Vec<UpstreamModelOption> {
    let arr = body
        .get("data")
        .and_then(|v| v.as_array())
        .or_else(|| body.as_array())
        .cloned()
        .unwrap_or_default();

    let mut out = Vec::new();
    for m in arr {
        let Some(raw_id) = m.get("id").and_then(|v| v.as_str()).map(str::trim) else {
            continue;
        };
        if raw_id.is_empty() {
            continue;
        }
        let display = m
            .get("display_name")
            .or_else(|| m.get("name"))
            .and_then(|v| v.as_str());
        out.push(normalize_upstream_model(
            tool_id,
            raw_id,
            display,
            None,
            false,
            "live",
        ));
    }
    dedup_model_options(out)
}

fn extract_snapshot_file_json(files: &HashMap<String, String>, key: &str) -> Option<Value> {
    let b64 = files.get(key)?;
    let bytes = BASE64.decode(b64).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn extract_snapshot_plain_string(files: &HashMap<String, String>, key: &str) -> Option<String> {
    let b64 = files.get(key)?;
    let bytes = BASE64.decode(b64).ok()?;
    let s = String::from_utf8(bytes).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn extract_workbuddy_probe_params(cred: &CredentialEnvelope) -> Option<(String, String, String, Option<String>)> {
    match &cred.payload {
        CredentialPayload::Token { access_token, extra, .. } => {
            if access_token.trim().is_empty() {
                return None;
            }
            let base = extra
                .get("endpoint")
                .or_else(|| extra.get("domain"))
                .filter(|s| s.starts_with("http"))
                .cloned()
                .unwrap_or_else(|| "https://copilot.tencent.com".into());
            let domain = extra.get("domain").cloned();
            Some((base.trim_end_matches('/').to_string(), access_token.clone(), cred.uid.clone(), domain))
        }
        CredentialPayload::LocalSnapshot { files } => {
            let sess = extract_snapshot_file_json(files, "session.json")?;
            let token = sess
                .pointer("/auth/accessToken")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned)
                .or_else(|| extract_snapshot_plain_string(files, "decrypted_token.txt"))?;
            let base = sess
                .pointer("/auth/endpoint")
                .and_then(|v| v.as_str())
                .filter(|s| s.starts_with("http"))
                .unwrap_or("https://copilot.tencent.com")
                .trim_end_matches('/')
                .to_string();
            let uid = sess
                .pointer("/account/uid")
                .or_else(|| sess.pointer("/account/id"))
                .and_then(|v| v.as_str())
                .unwrap_or(&cred.uid)
                .to_string();
            let domain = sess.pointer("/auth/domain").and_then(|v| v.as_str()).map(ToOwned::to_owned);
            Some((base, token, uid, domain))
        }
        CredentialPayload::Cookies { .. } => None,
    }
}

fn extract_token_and_extra(cred: &CredentialEnvelope) -> (Option<String>, Option<String>, Option<String>) {
    match &cred.payload {
        CredentialPayload::Token { access_token, extra, .. } => (
            Some(access_token.clone()).filter(|s| !s.trim().is_empty()),
            extra.get("base_url").cloned(),
            extra.get("machine_id").cloned(),
        ),
        CredentialPayload::LocalSnapshot { files } => {
            let tok = extract_snapshot_plain_string(files, "decrypted_token.txt")
                .or_else(|| extract_snapshot_plain_string(files, "access_token.txt"));
            let mid = extract_snapshot_plain_string(files, "machine_id");
            (tok, None, mid)
        }
        CredentialPayload::Cookies { .. } => (None, None, None),
    }
}

/// 按单个账号凭据在线探测其真实的可用模型列表；探测失败或离线时优雅回退到本地在线缓存 / 内置真实协议目录
pub async fn probe_models_for_account(cred: &CredentialEnvelope) -> Vec<UpstreamModelOption> {
    let live_res = try_probe_live_models(cred).await;
    if let Some(models) = live_res {
        if !models.is_empty() {
            return models;
        }
    }
    if let Some(cached) = load_magpie_provider_models(&cred.tool_id) {
        return cached;
    }
    default_model_options_for_tool(&cred.tool_id)
}

async fn try_probe_live_models(cred: &CredentialEnvelope) -> Option<Vec<UpstreamModelOption>> {
    let client = discovery_http_client().ok()?;
    match cred.tool_id.as_str() {
        "workbuddy" => {
            let (base, token, uid, domain) = extract_workbuddy_probe_params(cred)?;
            let url = format!("{base}/v3/config");
            let mut req = client
                .get(&url)
                .bearer_auth(&token)
                .header("Accept", "application/json")
                .header("X-Product", "SaaS")
                .header("X-IDE-Type", "WorkBuddy")
                .header("User-Agent", "CLI/2.0.0 WorkBuddy/5.5.6")
                .header("X-Requested-With", "XMLHttpRequest");
            if !uid.trim().is_empty() && !CredentialEnvelope::is_fallback_uid(&uid) {
                req = req.header("X-User-Id", uid);
            }
            if let Some(dom) = domain.filter(|s| !s.trim().is_empty()) {
                req = req.header("X-Domain", dom);
            }
            let resp = req.send().await.ok()?;
            if !resp.status().is_success() {
                return None;
            }
            let json: Value = resp.json().await.ok()?;
            let parsed = parse_workbuddy_v3_config(&json);
            if parsed.is_empty() { None } else { Some(parsed) }
        }
        "qoder_cn" => {
            let (token_opt, _, mid_opt) = extract_token_and_extra(cred);
            let token = token_opt?;
            let url = "https://gateway.qoder.com.cn/algo/api/v2/model/list?Encode=1";
            let mut req = client
                .get(url)
                .bearer_auth(&token)
                .header("Accept", "application/json")
                .header("User-Agent", "Qoder/claim")
                .header("Cosy-ClientType", "10");
            if let Some(mid) = mid_opt.filter(|s| !s.trim().is_empty()) {
                req = req.header("Cosy-MachineId", mid);
            }
            let resp = req.send().await.ok()?;
            if !resp.status().is_success() {
                return None;
            }
            let json: Value = resp.json().await.ok()?;
            let parsed = parse_qoder_model_list(&json);
            if parsed.is_empty() { None } else { Some(parsed) }
        }
        "trae_cn" => {
            let (token_opt, _, _) = extract_token_and_extra(cred);
            let token = token_opt?;
            let url = "https://trae-api-cn.mchost.guru/api/ide/v1/batch_get_detail_param";
            let resp = client
                .post(url)
                .header("Authorization", format!("Cloud-IDE-JWT {token}"))
                .header("Accept", "application/json")
                .header("X-User-Region", "CN")
                .json(&json!({
                    "ide_version": "2.0.15",
                    "function_names": ["IDE", "SOLO"]
                }))
                .send()
                .await
                .ok()?;
            if !resp.status().is_success() {
                return None;
            }
            let json: Value = resp.json().await.ok()?;
            let parsed = parse_trae_detail_param(&json);
            if parsed.is_empty() { None } else { Some(parsed) }
        }
        "minimax_code_cn" => {
            let (token_opt, _, _) = extract_token_and_extra(cred);
            let url = "https://agent.minimax.cn/mavis/api/v1/models";
            let mut req = client.get(url).header("Accept", "application/json");
            if let Some(tok) = token_opt {
                req = req.bearer_auth(tok);
            }
            let resp = req.send().await.ok()?;
            if !resp.status().is_success() {
                return None;
            }
            let json: Value = resp.json().await.ok()?;
            let parsed = parse_minimax_models(&json);
            if parsed.is_empty() { None } else { Some(parsed) }
        }
        "custom_api_key" => {
            let (token_opt, base_opt, _) = extract_token_and_extra(cred);
            let token = token_opt?;
            let base = base_opt
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "https://api.openai.com".into());
            let trimmed = base.trim_end_matches('/');
            let url = if trimmed.ends_with("/v1") {
                format!("{trimmed}/models")
            } else {
                format!("{trimmed}/v1/models")
            };
            let resp = client
                .get(&url)
                .bearer_auth(&token)
                .header("x-api-key", &token)
                .header("anthropic-version", "2023-06-01")
                .header("Accept", "application/json")
                .send()
                .await
                .ok()?;
            if !resp.status().is_success() {
                return None;
            }
            let json: Value = resp.json().await.ok()?;
            let parsed = parse_openai_models_list("custom_api_key", &json);
            if parsed.is_empty() { None } else { Some(parsed) }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_workbuddy_qoder_minimax_and_openai_model_payloads() {
        let wb_json = json!({
            "data": {
                "agents": [{ "name": "cli", "models": ["deepseek-v4-pro", "hy4flash-free", "glm-5.3"] }],
                "models": [
                    { "id": "deepseek-v4-pro", "name": "DeepSeek-V4-Pro", "rate": 0.51, "free": false },
                    { "id": "hy4flash-free", "name": "Hy4 flash", "rate": 0, "free": true },
                    { "id": "glm-5.3", "name": "GLM-5.3", "rate": 0.69, "free": false },
                    { "id": "unused-web-only", "name": "Unused", "rate": 1.0 }
                ]
            }
        });
        let wb = parse_workbuddy_v3_config(&wb_json);
        assert_eq!(wb.len(), 3);
        assert_eq!(wb[0].id, "deepseek-v4-pro");
        assert_eq!(wb[0].credit_multiplier, Some(0.51));
        assert!(wb[1].is_free);
        assert_eq!(wb[1].id, "hy4flash-free");

        let qoder_json = json!({
            "data": {
                "chat": [
                    { "key": "dmodel", "display_name": "DeepSeek-V4-Pro", "enable": true, "price_factor": 0.5 },
                    { "key": "gmodel", "display_name": "GLM-5.3", "enable": true, "price_factor": 0.8 },
                    { "key": "kmodel_latest", "display_name": "Kimi-K3.1-Pro", "enable": false, "price_factor": 1.0 },
                    { "key": "auto", "display_name": "Auto", "enable": true, "price_factor": 0 }
                ]
            }
        });
        let qd = parse_qoder_model_list(&qoder_json);
        assert_eq!(qd.len(), 2);
        assert_eq!(qd[0].id, "deepseek-v4-pro");
        assert_eq!(qd[0].vendor_wire_id, "dmodel");
        assert_eq!(qd[0].credit_multiplier, Some(0.5));
        assert_eq!(qd[1].id, "glm-5.3");
        assert_eq!(qd[1].vendor_wire_id, "gmodel");

        let mm_json = json!({
            "providers": [{
                "name": "minimax",
                "config": {
                    "models": [
                        { "id": "MiniMax-M3.1-Flash-Preview", "name": "M3.1-Flash-Preview", "type": "chat" },
                        { "id": "MiniMax-Image", "name": "MiniMax Image", "type": "image" }
                    ]
                }
            }]
        });
        let mm = parse_minimax_models(&mm_json);
        assert_eq!(mm.len(), 1);
        assert_eq!(mm[0].id, "minimax-m3.1-flash");
        assert_eq!(mm[0].vendor_wire_id, "MiniMax-M3.1-Flash-Preview");
    }
}
