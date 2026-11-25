use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use crate::domain::{
    dedup_model_options, default_model_options_for_tool, normalize_upstream_model,
    CredentialEnvelope, CredentialPayload, UpstreamModelOption,
};

/// 探测超时。
///
/// 历史实现为 4s，海外/中转上游（OpenRouter、Anthropic 兼容站、火山方舟等）
/// 在冷链路上经常超过该阈值，导致「探测失败 → 回退静态表 → 缓存锁死旧模型」。
const DISCOVERY_TIMEOUT_SECS: u64 = 10;

/// 各适配器写入 LocalSnapshot 时使用的键名。
/// 必须与 adapter/*.rs 中的常量保持一致，否则本机导入账号一律探测失败。
const WORKBUDDY_AUTH_FILENAME: &str = "workbuddy-desktop.info";
const WORKBUDDY_SESSION_FILENAME: &str = "session.json";
const TRAE_AUTH_ENTRIES_KEY: &str = "icube_auth_entries.json";
const TRAE_DEVICE_KEY: &str = "trae_device.json";
const QODER_MACHINE_ID_FILE: &str = "auth.machine-id";
const MINIMAX_AUTH_JSON: &str = "auth.json";
const DECRYPTED_TOKEN_FILE: &str = "decrypted_token.txt";

/// WorkBuddy 上游版本号（与 gateway 实际发起的请求保持一致）。
/// 网关侧已升级到 5.9.3，探测侧停留在 5.5.6 会被服务端版本门控拒绝。
const WORKBUDDY_USER_AGENT: &str = "WorkBuddy/5.9.3 WorkBuddy/5.9.3 CLI/2.165.0";
/// Trae CN 客户端版本号（与 adapter 中的 DEFAULT_TRAE_APP_VERSION 对齐）。
const TRAE_APP_VERSION: &str = "3.4.22";
const TRAE_SOLO_CLIENT_VERSION: &str = "0.1.69";
const TRAE_SOLO_CLIENT_VERSION_CODE: &str = "20260917";
const TRAE_APP_ID: &str = "6eefa01c-1036-4c7e-9ca5-d891f63bfcd8";
const TRAE_CHAT_FUNCTIONS: &[&str] = &[
    "chat_v3",
    "solo_work_lite",
    "solo_agent",
    "solo_agent_lite",
    "solo_coder",
];
/// 有道龙虾服务端（与 adapter/lobsterai.rs 保持一致）
const LOBSTERAI_SERVER_BASE: &str = "https://lobsterai-server.youdao.com";
const LOBSTERAI_USER_AGENT: &str = "LobsterAI/2026.11.20";

fn discovery_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(DISCOVERY_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("构造模型探测 HTTP 客户端失败: {e}"))
}

/// 判断是否为应被过滤的噪声变体。
///
/// 历史实现包含 `lower.ends_with("-max")`，把 `qwen4-max`、`qwen3.8-max`、
/// `gpt-4-max` 等**正常主力模型**一并过滤掉。静态目录里这些模型是明确存在的
/// （见 domain/pools.rs 的 qoder_cn / trae_cn 目录），一旦实时探测把它们滤掉，
/// 实时结果反而比静态兜底更差 —— 这正是「部分工具模型不完整」的直接原因之一。
/// 现已移除该规则；噪声过滤只保留自动路由、搜索/子智能体等确实不应作为
/// 手动可选模型的伪条目。
fn is_noise_variant_id(raw_id: &str) -> bool {
    let lower = raw_id.to_ascii_lowercase();
    lower.is_empty()
        || lower == "default"
        // 纯自动路由入口（不是可选模型）
        || lower == "auto"
        || lower.contains("_advisor")
        || lower.ends_with("-auto")
        || lower.contains("subagent")
        || lower.contains("sub_agent")
        || lower.contains("search_agent")
        || lower == "sagitta"
        || lower == "aquila"
        || lower == "doubao-for-auto"
        || lower == "minimax-image"
        || lower.starts_with("minimax-hailuo")
}

/// 一次账号模型探测的结果。
///
/// 关键点：必须区分「实时探测成功」与「回退到静态目录」。
/// 历史实现把两者混为一谈，导致回退出来的静态表被当作探测成功写入缓存，
/// 并向前端弹出绿色的「已完成全部上游账号真实模型目录探测」——
/// 用户看到的是一次**并不存在**的成功。
#[derive(Debug, Clone)]
pub struct ProbeOutcome {
    /// 最终可用的模型列表（实时或回退）。
    pub models: Vec<UpstreamModelOption>,
    /// 数据来源："live"（本次实时探测）/ "cache"（本机二级目录缓存）/ "catalog"（静态兜底目录）。
    pub source: &'static str,
    /// 实时探测失败时的原因，用于向用户如实反馈。
    pub error: Option<String>,
}

impl ProbeOutcome {
    fn live(models: Vec<UpstreamModelOption>) -> Self {
        Self {
            models,
            source: "live",
            error: None,
        }
    }

    /// 来自本机客户端的权威目录（例如码道的内核配置）。
    /// 它不是 HTTP 实时探测，但同样是该账号的真实可用模型，允许落库。
    fn from_client(models: Vec<UpstreamModelOption>) -> Self {
        Self {
            models,
            source: "client",
            error: None,
        }
    }

    fn fallback(
        models: Vec<UpstreamModelOption>,
        source: &'static str,
        error: Option<String>,
    ) -> Self {
        Self {
            models,
            source,
            error,
        }
    }

    /// 是否为本次实时探测的真实结果（只有它才允许写入 live 缓存）。
    pub fn is_live(&self) -> bool {
        self.source == "live"
    }

    /// 是否为「可落库的权威目录」：实时探测，或本机客户端自带的真实清单。
    /// 回退的 cache / catalog 永远不算。
    pub fn is_authoritative(&self) -> bool {
        matches!(self.source, "live" | "client")
    }
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
            tool_id, raw_id, display, rate, is_free, "cache",
        ));
    }

    let deduped = dedup_model_options(parsed);
    if deduped.is_empty() {
        None
    } else {
        Some(deduped)
    }
}

/// 解析 WorkBuddy `/v3/config` 真实响应中的可用模型列表。
///
/// 历史实现只要在 `data.agents` 中命中名为 `cli` 的 agent，就**只输出该 agent
/// 的模型**并丢弃 `data.models` 中的其余条目 —— 服务端按 agent 维度下发模型时，
/// `data.models` 往往包含其它 agent/端（web、ide 等）之外的完整目录，于是实时
/// 探测结果天然少于静态兜底。现改为：**合并 `data.models` 全量条目**，并把
/// `cli` agent 声明的模型排到最前（CLI 场景优先），不再丢弃任何条目。
pub fn parse_workbuddy_v3_config(body: &Value) -> Vec<UpstreamModelOption> {
    let data = body.get("data").unwrap_or(body);
    let mut cli_model_ids: Vec<String> = Vec::new();
    let mut fallback_agent_ids: Vec<String> = Vec::new();
    if let Some(agents) = data.get("agents").and_then(|v| v.as_array()) {
        for ag in agents {
            let name = ag.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let Some(arr) = ag.get("models").and_then(|v| v.as_array()) else {
                continue;
            };
            let ids: Vec<String> = arr
                .iter()
                .filter_map(|v| {
                    v.as_str()
                        .map(ToOwned::to_owned)
                        .or_else(|| v.get("id").and_then(|x| x.as_str()).map(ToOwned::to_owned))
                })
                .collect();
            if ids.is_empty() {
                continue;
            }
            if name.eq_ignore_ascii_case("cli") {
                cli_model_ids = ids;
            } else {
                fallback_agent_ids.extend(ids);
            }
        }
    }

    let mut details_by_id: HashMap<String, &Value> = HashMap::new();
    let mut ordered_models: Vec<(String, Option<&Value>)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // 先收全量 `data.models`（数组或对象两种形态都支持）
    match data.get("models") {
        Some(Value::Array(arr)) => {
            for item in arr {
                if let Some(id) = item
                    .get("id")
                    .or_else(|| item.get("modelId"))
                    .and_then(|v| v.as_str())
                {
                    details_by_id.insert(id.to_string(), item);
                    if seen.insert(id.to_string()) {
                        ordered_models.push((id.to_string(), Some(item)));
                    }
                }
            }
        }
        Some(Value::Object(map)) => {
            for (k, item) in map {
                details_by_id.insert(k.clone(), item);
                if seen.insert(k.clone()) {
                    ordered_models.push((k.clone(), Some(item)));
                }
            }
        }
        _ => {}
    }

    // CLI agent 声明的模型排前（CLI 场景优先），已存在的不重复插入
    let mut ordered: Vec<(String, Option<&Value>)> = Vec::with_capacity(ordered_models.len());
    let mut ordered_seen: HashSet<String> = HashSet::new();
    for id in cli_model_ids.iter().chain(fallback_agent_ids.iter()) {
        if ordered_seen.insert(id.clone()) {
            let detail = details_by_id.get(id).copied();
            ordered.push((id.clone(), detail));
        }
    }
    for (id, detail) in ordered_models {
        if ordered_seen.insert(id.clone()) {
            ordered.push((id, detail));
        }
    }

    let mut out = Vec::new();
    for (raw_id, detail_opt) in ordered {
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

/// 解析 Qoder CN `/algo/api/v2/model/list?Encode=1` 真实响应。
///
/// 历史实现只遍历 `["app", "chat"]` 两个写死的场景桶，`code` / `agent` / `quest` /
/// `cli` 等桶里的模型全部被丢弃。现改为遍历 `data` 下**所有**值为数组的场景桶。
pub fn parse_qoder_model_list(body: &Value) -> Vec<UpstreamModelOption> {
    let listing = body.get("data").unwrap_or(body);
    let mut combined: Vec<&Value> = Vec::new();
    let mut seen_keys = HashSet::new();

    // `data` 既可能是「场景名 -> 模型数组」的对象，也可能直接是模型数组。
    let mut scenes: Vec<(&str, &Value)> = Vec::new();
    match listing {
        Value::Object(map) => {
            for (scene, value) in map {
                if value.is_array() {
                    scenes.push((scene.as_str(), value));
                }
            }
        }
        Value::Array(arr) => {
            scenes.push(("", listing));
            let _ = arr;
        }
        _ => {}
    }

    // 场景顺序决定模型展示顺序：优先「对话/应用」场景（用户日常主用），
    // 其次编码场景，最后是 agent / quest 等附加场景。
    // 不排序的话 serde_json 的 BTreeMap 会按字母序把 `agent` 排在 `chat` 前面，
    // 导致对话主力模型被挤到列表末尾。
    scenes.sort_by_key(|(scene, _)| match *scene {
        "chat" => 0,
        "app" => 1,
        "code" => 2,
        "agent" => 3,
        "cli" => 4,
        "quest" => 5,
        _ => 6,
    });

    for (_scene, arr) in scenes {
        let Some(items) = arr.as_array() else {
            continue;
        };
        for item in items {
            let enabled = item.get("enable").and_then(|v| v.as_bool()).unwrap_or(true);
            if !enabled {
                continue;
            }
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

    let mut out = Vec::new();
    for item in combined {
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
        let promo_before = promo.and_then(|p| {
            as_opt_f64(
                p.get("before_promotion_price_factor")
                    .or_else(|| p.get("beforePromotionPriceFactor")),
            )
        });
        let is_free = match price {
            Some(0.0) => !(promo_active && promo_before.is_some_and(|b| b > 0.0)),
            Some(_) => false,
            None => item
                .get("is_free")
                .or_else(|| item.get("isFree"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        };
        out.push(normalize_upstream_model(
            "qoder_cn", key, display, price, is_free, "live",
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
                    if obj
                        .get("is_custom_model")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    let id_opt = obj
                        .get("config_name")
                        .or_else(|| obj.get("model_name"))
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
                                    "trae_cn", raw_id, display, rate, is_free, "live",
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

/// 解析 MiniMax Code CN `/mavis/api/v1/models` 响应。
///
/// 实际线上接口返回 `providers[].config.models` 为 JSON Object（以模型 id 为 key，
/// 配合可选 `model_order` 排序数组）；同时兼容历史/测试的 JSON Array 与扁平 `data.models`。
pub fn parse_minimax_models(body: &Value) -> Vec<UpstreamModelOption> {
    let root = body.get("data").unwrap_or(body);
    let mut out = Vec::new();

    let push_entry = |raw_id: &str, m: &Value, out: &mut Vec<UpstreamModelOption>| {
        let raw_id = raw_id.trim();
        if is_noise_variant_id(raw_id) {
            return;
        }
        let kind = m.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if matches!(kind, "image" | "video" | "speech" | "audio") {
            return;
        }
        let display = m.get("name").and_then(|v| v.as_str());
        let mut opt = normalize_upstream_model(
            "minimax_code_cn",
            raw_id,
            display,
            Some(0.0),
            false,
            "live",
        );
        if let Some(ctx) = m
            .pointer("/limit/context")
            .and_then(|v| v.as_u64())
            .filter(|&n| n > 0)
        {
            opt.context_window = Some(ctx);
        }
        if let Some(max_out) = m
            .pointer("/limit/output")
            .and_then(|v| v.as_u64())
            .filter(|&n| n > 0)
        {
            opt.max_output_tokens = Some(max_out);
        }
        out.push(opt);
    };

    let push_models = |models: &Value, order_val: Option<&Value>, out: &mut Vec<UpstreamModelOption>| {
        if let Some(obj) = models.as_object() {
            let mut ordered_keys: Vec<String> = Vec::new();
            let mut seen = HashSet::new();
            if let Some(order_arr) = order_val.and_then(|v| v.as_array()) {
                for k in order_arr.iter().filter_map(|v| v.as_str()) {
                    if obj.contains_key(k) && seen.insert(k.to_string()) {
                        ordered_keys.push(k.to_string());
                    }
                }
            }
            for k in obj.keys() {
                if seen.insert(k.clone()) {
                    ordered_keys.push(k.clone());
                }
            }
            for k in ordered_keys {
                if let Some(m) = obj.get(&k) {
                    push_entry(&k, m, out);
                }
            }
            return;
        }
        if let Some(arr) = models.as_array() {
            for m in arr {
                let Some(raw_id) = m.get("id").and_then(|v| v.as_str()) else {
                    continue;
                };
                push_entry(raw_id, m, out);
            }
        }
    };

    if let Some(providers) = root.get("providers").and_then(|v| v.as_array()) {
        // 若存在 providerId == "minimax" 的主条目则优先解析它，否则回退遍历全部 provider
        let minimax_providers: Vec<&Value> = providers
            .iter()
            .filter(|p| {
                p.get("providerId").and_then(|v| v.as_str()) == Some("minimax")
                    || p.get("name").and_then(|v| v.as_str()) == Some("minimax")
            })
            .collect();
        let target_providers: Vec<&Value> = if minimax_providers.is_empty() {
            providers.iter().collect()
        } else {
            minimax_providers
        };
        for prov in target_providers {
            if let Some(models) = prov.pointer("/config/models") {
                push_models(models, prov.pointer("/config/model_order"), &mut out);
            }
        }
    }
    // 扁平结构：`data.models` / 根级 `models`
    for key in ["models", "model_list"] {
        if let Some(models) = root.get(key) {
            push_models(models, root.get("model_order"), &mut out);
        }
    }

    dedup_model_options(out)
}

/// 从常见响应结构中提取模型数组。
///
/// 兼容 `data` / `models` / `items` / `list` 以及根级数组，
/// 单条模型的 id 支持 `id` / `model` / `name`。
pub fn parse_openai_models_list(tool_id: &str, body: &Value) -> Vec<UpstreamModelOption> {
    let arr = ["data", "models", "items", "list"]
        .iter()
        .find_map(|k| body.get(*k).and_then(|v| v.as_array()))
        .or_else(|| body.as_array())
        .cloned()
        .unwrap_or_default();

    let mut out = Vec::new();
    for m in arr {
        let Some(raw_id) = m
            .get("id")
            .or_else(|| m.get("model"))
            .or_else(|| m.get("name"))
            .and_then(|v| v.as_str())
            .map(str::trim)
        else {
            continue;
        };
        if raw_id.is_empty() {
            continue;
        }
        let display = m
            .get("display_name")
            .or_else(|| m.get("name"))
            .or_else(|| m.get("displayName"))
            .and_then(|v| v.as_str());
        out.push(normalize_upstream_model(
            tool_id, raw_id, display, None, false, "live",
        ));
    }
    dedup_model_options(out)
}

/// 拼接 OpenAI 兼容的 `/models` 端点。
///
/// 历史实现只在 `base_url` 以 `/v1` 结尾时才避免重复拼接，导致智谱
/// `/api/paas/v4`、火山方舟 `/api/v3`、Gemini `/v1beta/openai` 被拼成
/// `/v4/v1/models` 之类的非法路径并 404。现改为：路径中**任一段**已是版本号
/// （`v1` / `v3` / `v4` / `v1beta` …）时直接追加 `/models`。
pub fn build_models_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    let path_part = trimmed
        .split_once("://")
        .map(|(_, rest)| rest.split_once('/').map(|(_, p)| p).unwrap_or(""))
        .unwrap_or(trimmed);
    let has_version = path_part.split('/').any(|seg| {
        let s = seg.to_ascii_lowercase();
        s.len() > 1 && s.starts_with('v') && s[1..].starts_with(|c: char| c.is_ascii_digit())
    });
    if has_version {
        format!("{trimmed}/models")
    } else {
        format!("{trimmed}/v1/models")
    }
}

/// 判断自定义网关是否使用 Anthropic 风格鉴权（`x-api-key` + `anthropic-version`）。
fn is_anthropic_style_base(base_url: &str) -> bool {
    base_url.to_ascii_lowercase().contains("anthropic")
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

/// WorkBuddy 探测所需参数。
#[derive(Debug, Clone)]
struct WorkBuddyProbeParams {
    endpoint: String,
    token: String,
    uid: String,
    domain: Option<String>,
    enterprise_id: Option<String>,
}

/// WorkBuddy 探测所需参数：`(endpoint, token, uid, domain, enterprise_id)`。
///
/// 历史实现只认 `session.json`，而适配器实际写入的是 `workbuddy-desktop.info`，
/// 导致**所有本机导入的 WorkBuddy 账号**探测必然返回 `None`。
/// 现按适配器的真实键名读取，并用 `decrypted_token.txt`（适配器解密后落盘）
/// 作为 token 来源（`auth.accessToken` 在磁盘上是 sym-v1 密文对象，不可直读）。
fn extract_workbuddy_probe_params(cred: &CredentialEnvelope) -> Option<WorkBuddyProbeParams> {
    match &cred.payload {
        CredentialPayload::Token {
            access_token,
            extra,
            ..
        } => {
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
            let enterprise_id = extra
                .get("enterprise_id")
                .cloned()
                .filter(|s| !s.trim().is_empty());
            Some(WorkBuddyProbeParams {
                endpoint: base.trim_end_matches('/').to_string(),
                token: access_token.clone(),
                uid: cred.uid.clone(),
                domain,
                enterprise_id,
            })
        }
        CredentialPayload::LocalSnapshot { files } => {
            let sess = extract_snapshot_file_json(files, WORKBUDDY_AUTH_FILENAME)
                .or_else(|| extract_snapshot_file_json(files, WORKBUDDY_SESSION_FILENAME));
            let token =
                extract_snapshot_plain_string(files, DECRYPTED_TOKEN_FILE).or_else(|| {
                    sess.as_ref()
                        .and_then(|s| s.pointer("/auth/accessToken"))
                        .and_then(|v| v.as_str())
                        .map(ToOwned::to_owned)
                })?;
            if token.trim().is_empty() {
                return None;
            }
            let base = sess
                .as_ref()
                .and_then(|s| s.pointer("/auth/endpoint"))
                .and_then(|v| v.as_str())
                .filter(|s| s.starts_with("http"))
                .unwrap_or("https://copilot.tencent.com")
                .trim_end_matches('/')
                .to_string();
            let uid = sess
                .as_ref()
                .and_then(|s| s.pointer("/account/uid"))
                .and_then(|v| v.as_str())
                .unwrap_or(&cred.uid)
                .to_string();
            let domain = sess
                .as_ref()
                .and_then(|s| s.pointer("/auth/domain"))
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned);
            let enterprise_id = sess
                .as_ref()
                .and_then(|s| s.pointer("/account/enterpriseId"))
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned)
                .filter(|s| !s.trim().is_empty());
            Some(WorkBuddyProbeParams {
                endpoint: base,
                token,
                uid,
                domain,
                enterprise_id,
            })
        }
        CredentialPayload::Cookies { .. } => None,
    }
}

/// 从 Trae 的 `icube_auth_entries.json` 快照中尽力提取明文 token。
///
/// 该文件中的值可能是加密串（需客户端二进制解密），此处只处理可直接解析的
/// JSON 明文形态；取不到时由调用方回退到 `decrypted_token.txt`。
fn extract_trae_token_from_auth_entries(files: &HashMap<String, String>) -> Option<String> {
    let raw = files.get(TRAE_AUTH_ENTRIES_KEY)?;
    let bytes = BASE64.decode(raw).ok()?;
    let entries: Map<String, Value> = serde_json::from_slice(&bytes).ok()?;
    let primary = entries
        .get("iCubeAuthInfo://icube.cloudide")
        .or_else(|| entries.values().next())?;
    let parsed = match primary {
        Value::String(s) if s.trim_start().starts_with('{') => {
            serde_json::from_str::<Value>(s).ok()?
        }
        other => other.clone(),
    };
    parsed
        .get("token")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

/// Trae 账号级设备身份（`trae_device.json`），用于补齐服务端版本门控所需的设备头。
fn extract_trae_device_headers(files: &HashMap<String, String>) -> Vec<(&'static str, String)> {
    let Some(dev) = extract_snapshot_file_json(files, TRAE_DEVICE_KEY) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut push = |name: &'static str, value: Option<&Value>| {
        if let Some(v) = value
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            out.push((name, v.to_string()));
        }
    };
    push("x-device-id", dev.get("device_id"));
    push("x-device-brand", dev.get("device_brand"));
    push("x-device-type", dev.get("device_type"));
    push("x-os-version", dev.get("os_version"));
    push("x-app-version", dev.get("app_version"));
    out
}

/// 通用凭据提取：`(token, base_url, machine_id)`。
///
/// 各工具的快照键名互不相同，历史实现只认 `decrypted_token.txt` / `machine_id`，
/// 导致 Trae 的 `icube_auth_entries.json`、Qoder 的 `auth.machine-id` 读不到。
fn extract_token_and_extra(
    cred: &CredentialEnvelope,
) -> (Option<String>, Option<String>, Option<String>) {
    match &cred.payload {
        CredentialPayload::Token {
            access_token,
            extra,
            ..
        } => (
            Some(access_token.clone()).filter(|s| !s.trim().is_empty()),
            extra.get("base_url").cloned(),
            extra.get("machine_id").cloned(),
        ),
        CredentialPayload::LocalSnapshot { files } => {
            let tok = extract_snapshot_plain_string(files, DECRYPTED_TOKEN_FILE)
                .or_else(|| extract_snapshot_plain_string(files, "access_token.txt"))
                .or_else(|| extract_trae_token_from_auth_entries(files))
                .or_else(|| extract_minimax_token_from_auth_json(files));
            let mid = extract_snapshot_plain_string(files, QODER_MACHINE_ID_FILE)
                .or_else(|| extract_snapshot_plain_string(files, "machine_id"));
            (tok, None, mid)
        }
        CredentialPayload::Cookies { .. } => (None, None, None),
    }
}

/// 从 MiniMax `auth.json` 快照的 records 中提取 access token（离线兜底）。
fn extract_minimax_token_from_auth_json(files: &HashMap<String, String>) -> Option<String> {
    let auth = extract_snapshot_file_json(files, MINIMAX_AUTH_JSON)?;
    let records = auth.get("records")?.as_object()?;
    records
        .values()
        .filter_map(|v| v.as_object())
        .filter_map(|rec| rec.get("accessToken").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .max_by_key(|s| s.len())
        .map(ToOwned::to_owned)
}

/// 按单个账号凭据在线探测其真实的可用模型列表。
///
/// 返回值显式区分实时探测与回退来源，调用方据此决定是否写入 live 缓存、
/// 以及如何向用户反馈真实结果（而不是一律报成功）。
pub async fn probe_models_for_account(cred: &CredentialEnvelope) -> ProbeOutcome {
    match try_probe_live_models(cred).await {
        Ok(models) if !models.is_empty() => {
            // 实时结果登记到 wire-id 注册表：静态映射表未收录的模型，
            // 只有这里能拿到上游真实 ID（大小写敏感的 wire id）。
            crate::domain::register_live_wire_ids(&cred.tool_id, &models);
            if models.iter().all(|m| m.source == "client") {
                ProbeOutcome::from_client(models)
            } else {
                ProbeOutcome::live(models)
            }
        }
        Ok(_) => {
            let err = Some("上游返回了空模型列表".to_string());
            let (models, source) = fallback_models(cred);
            ProbeOutcome::fallback(models, source, err)
        }
        Err(e) => {
            let (models, source) = fallback_models(cred);
            ProbeOutcome::fallback(models, source, Some(e))
        }
    }
}

/// 实时探测失败时的回退顺序：本机二级目录缓存 → 静态兜底目录。
///
/// 返回值同时携带来源标记：`cache` 表示来自本机二级目录，`catalog` 表示静态兜底。
/// 二者都**不是**实时探测结果，因此都不允许写入 live 缓存。
fn fallback_models(cred: &CredentialEnvelope) -> (Vec<UpstreamModelOption>, &'static str) {
    if let Some(cached) = load_magpie_provider_models(&cred.tool_id) {
        return (cached, "cache");
    }
    (default_model_options_for_tool(&cred.tool_id), "catalog")
}

/// 有道龙虾：`GET /api/models/available`（桌面端模型选择器用的同一个接口）。
///
/// 返回的是「模型预设」列表，逐项可能带 `modelId` / `id`；不可用的预设
/// （`accessible == false`）要排除——它们在下拉框里也是灰的。
async fn probe_lobsterai_models(
    client: &reqwest::Client,
    cred: &CredentialEnvelope,
) -> Result<Vec<UpstreamModelOption>, String> {
    let (token_opt, base_opt, _) = extract_token_and_extra(cred);
    let token = token_opt.ok_or_else(|| "有道龙虾凭据缺少访问令牌".to_string())?;
    let base = base_opt
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| LOBSTERAI_SERVER_BASE.to_string());
    let resp = client
        .get(format!("{base}/api/models/available"))
        .bearer_auth(&token)
        .header("Accept", "application/json")
        .header("User-Agent", LOBSTERAI_USER_AGENT)
        .header("Cache-Control", "no-store")
        .send()
        .await
        .map_err(|e| format!("请求有道龙虾模型目录失败: {}", e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!("有道龙虾模型目录返回 HTTP {}", resp.status()));
    }
    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("解析有道龙虾模型目录失败: {e}"))?;
    let models = parse_lobsterai_models(&json);
    if models.is_empty() {
        Err("有道龙虾模型目录解析结果为空".into())
    } else {
        Ok(models)
    }
}

/// 解析龙虾 `/api/models/available`：`{ code: 0, data: [ { modelId|id, displayName|name } ] }`。
fn parse_lobsterai_models(json: &Value) -> Vec<UpstreamModelOption> {
    if json.get("code").and_then(Value::as_i64).unwrap_or(0) != 0 {
        return Vec::new();
    }
    let list = json.get("data").and_then(|d| {
        d.as_array()
            .or_else(|| d.get("list").and_then(Value::as_array))
    });
    let Some(items) = list else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        if item.get("accessible").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let id = ["modelId", "id", "model_id", "name"]
            .iter()
            .find_map(|k| item.get(*k).and_then(Value::as_str))
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(id) = id else { continue };
        if is_noise_variant_id(id) || !seen.insert(id.to_string()) {
            continue;
        }
        let display = ["displayName", "display_name", "label", "name"]
            .iter()
            .find_map(|k| item.get(*k).and_then(Value::as_str))
            .map(str::trim)
            .filter(|s| !s.is_empty());
        out.push(normalize_upstream_model(
            "lobsterai",
            id,
            display,
            None,
            false,
            "live",
        ));
    }
    out
}

/// 豆包工作（DoubaoWork）的模型清单：读 magpie 插件缓存。
///
/// 豆包工作**没有给第三方用的模型接口**（magpie 也是靠 CDP 驱动桌面端），
/// 但它自己的「工作任务」模型菜单是真实存在的：magpie 的 doubao-work 插件
/// 会把 `brief_list` 的结果缓存到 `magpie-doubao-work/<app>/models.json`。
/// 我们只读这份缓存（不启动、不驱动客户端），拿到的是该账号菜单里真实可选的模型。
fn probe_doubao_work_models() -> Result<Vec<UpstreamModelOption>, String> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        roots.push(PathBuf::from(home).join("Library/Application Support/magpie-doubao-work"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join("magpie-doubao-work"));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    for root in roots {
        for app in ["doubaowork", "doubao"] {
            let p = root.join(app).join("models.json");
            if p.is_file() {
                candidates.push(p);
            }
        }
    }
    if candidates.is_empty() {
        return Err("未找到豆包工作的模型缓存（需先用 magpie 的 doubao-work 插件登录一次）".into());
    }
    // 新缓存优先（文件名固定，按修改时间排序）
    candidates.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .map(std::cmp::Reverse)
    });
    for path in candidates {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(models) = doubao_models_from_cache(&raw) {
            return Ok(models);
        }
    }
    Err("豆包工作的模型缓存为空或格式已变".into())
}

/// 解析 magpie 的 `models.json`：`{ "models": { "<id>": { "label"|"name" } } }`。
fn doubao_models_from_cache(raw: &str) -> Option<Vec<UpstreamModelOption>> {
    let root: Value = serde_json::from_str(raw).ok()?;
    let map = root.get("models")?.as_object()?;
    let mut out = Vec::new();
    for (id, item) in map {
        // 注意：不能用 is_noise_variant_id 过滤——豆包工作的「自动」就是 id `auto`，
        // 是菜单里真实存在的一项，不是噪声变体。
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        let label = item
            .get("label")
            .and_then(Value::as_str)
            .or_else(|| item.get("name").and_then(Value::as_str))
            .map(str::trim)
            .filter(|s| !s.is_empty());
        out.push(normalize_upstream_model(
            "doubaowork",
            id,
            label,
            None,
            false,
            "client",
        ));
    }
    if out.is_empty() {
        return None;
    }
    // JSON 对象在 serde 里是 BTreeMap，顺序不稳定；按展示名排序保证列表可复现
    out.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    Some(out)
}

/// 码道 CodeArts 的模型清单：从本机客户端内核日志里读取。
///
/// 码道没有对外可用的模型目录接口（本地内核 `/cag/model` 需要 IDE 私有的
/// `auth_token`，云端 inferhub 路径未公开），但 IDE 每次启动都会把完整配置
/// POST 给内核，其中 `multimodalModels` 就是该账号当前可用的模型列表。
/// 这份日志是本机明文、只读的，取模型名不涉及任何凭据。
fn probe_codearts_client_models() -> Result<Vec<UpstreamModelOption>, String> {
    let mut logs: Vec<PathBuf> = Vec::new();
    for dir in codearts_log_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("txt") {
                logs.push(path);
            }
        }
    }
    if logs.is_empty() {
        return Err("未找到码道客户端日志：请先启动一次 CodeArts 客户端".into());
    }
    // 新日志优先：配置在每次启动时写入
    logs.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .map(std::cmp::Reverse)
    });

    for path in logs.iter().take(5) {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Some(models) = codearts_models_from_kernel_log(&text) {
            return Ok(models);
        }
    }
    Err("码道客户端日志中未找到模型清单（请打开一次客户端后再刷新）".into())
}

fn codearts_log_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(
            PathBuf::from(home).join("Library/Application Support/codearts/app-logs/logs/kernel"),
        );
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("codearts/app-logs/logs/kernel"));
    }
    dirs
}

/// 从内核日志里抽出模型清单（纯函数，便于单测）。
///
/// 日志行形如：`[init_post] config: {"multimodalModels":[...], ...}`。
/// 取最后一次出现的那行，解析失败则视为没有。
fn codearts_models_from_kernel_log(text: &str) -> Option<Vec<UpstreamModelOption>> {
    const MARKER: &str = "[init_post] config: ";
    let start = text.rfind(MARKER)? + MARKER.len();
    let rest = &text[start..];
    let line_end = rest.find('\n').unwrap_or(rest.len());
    let cfg: Value = serde_json::from_str(rest[..line_end].trim_end()).ok()?;

    let mut raw_ids: Vec<String> = Vec::new();
    if let Some(list) = cfg.get("multimodalModels").and_then(Value::as_array) {
        raw_ids.extend(
            list.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned),
        );
    }
    // 部分版本会把模型放在 provider.models（map 或数组）
    if let Some(models) = cfg.get("provider").and_then(|p| p.get("models")) {
        match models {
            Value::Array(items) => {
                for item in items {
                    if let Some(id) = item
                        .get("id")
                        .and_then(Value::as_str)
                        .or_else(|| item.as_str())
                    {
                        raw_ids.push(id.trim().to_string());
                    }
                }
            }
            Value::Object(map) => raw_ids.extend(map.keys().map(|k| k.trim().to_string())),
            _ => {}
        }
    }

    let mut seen = HashSet::new();
    let models: Vec<UpstreamModelOption> = raw_ids
        .into_iter()
        .filter(|id| !id.is_empty() && seen.insert(id.clone()))
        .map(|id| normalize_upstream_model("codearts", &id, None, None, false, "client"))
        .collect();
    if models.is_empty() {
        None
    } else {
        Some(models)
    }
}

async fn try_probe_live_models(
    cred: &CredentialEnvelope,
) -> Result<Vec<UpstreamModelOption>, String> {
    let client = discovery_http_client()?;
    match cred.tool_id.as_str() {
        "workbuddy" => {
            let params = extract_workbuddy_probe_params(cred).ok_or_else(|| {
                "WorkBuddy 凭据缺少可用的 access token（需先完成一次在线授权）".to_string()
            })?;
            let base = params.endpoint;
            let token = params.token;
            let uid = params.uid;
            let domain = params.domain;
            let enterprise_id = params.enterprise_id;
            let url = format!("{base}/v3/config");
            let mut req = client
                .get(&url)
                .bearer_auth(&token)
                .header("Accept", "application/json")
                .header("X-Product", "SaaS")
                .header("X-IDE-Type", "WorkBuddy")
                .header("X-IDE-Name", "WorkBuddy")
                .header("X-IDE-Version", "5.9.3")
                .header("User-Agent", WORKBUDDY_USER_AGENT)
                .header("X-Requested-With", "XMLHttpRequest");
            if !uid.trim().is_empty() && !CredentialEnvelope::is_fallback_uid(&uid) {
                req = req.header("X-User-Id", uid);
            }
            if let Some(dom) = domain.filter(|s| !s.trim().is_empty()) {
                req = req.header("X-Domain", dom);
            }
            // 企业版账号必须携带企业/租户标识，否则拿不到企业模型或直接被拒
            if let Some(eid) = enterprise_id.filter(|s| !s.trim().is_empty()) {
                req = req.header("X-Enterprise-Id", eid.clone());
                req = req.header("X-Tenant-Id", eid);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| format!("请求 WorkBuddy 模型目录失败: {}", e.without_url()))?;
            if !resp.status().is_success() {
                return Err(format!("WorkBuddy 模型目录返回 HTTP {}", resp.status()));
            }
            let json: Value = resp
                .json()
                .await
                .map_err(|e| format!("解析 WorkBuddy 模型目录失败: {e}"))?;
            let parsed = parse_workbuddy_v3_config(&json);
            if parsed.is_empty() {
                Err("WorkBuddy 模型目录解析结果为空".into())
            } else {
                Ok(parsed)
            }
        }
        "qoder_cn" => {
            let (token_opt, _, mid_opt) = extract_token_and_extra(cred);
            let token = token_opt.ok_or_else(|| "Qoder 凭据缺少 access token".to_string())?;
            let url = "https://gateway.qoder.com.cn/algo/api/v2/model/list?Encode=1";
            let machine_id = mid_opt.unwrap_or_default();
            let mut req = client.get(url);
            if let Ok(cosy_hdrs) = crate::adapter::qoder_cosy::cosy_headers(
                url,
                &cred.uid,
                cred.display_name.as_deref().unwrap_or(""),
                cred.display_email.as_deref().unwrap_or(""),
                &token,
                &machine_id,
                "",
                false,
                None,
                None,
            ) {
                for (k, v) in cosy_hdrs {
                    req = req.header(k, v);
                }
            } else {
                req = req
                    .bearer_auth(&token)
                    .header("Accept", "application/json")
                    .header("User-Agent", "Qoder/claim")
                    .header("Cosy-ClientType", "10");
                if !machine_id.trim().is_empty() {
                    req = req.header("Cosy-MachineId", machine_id);
                }
            }
            let resp = req
                .send()
                .await
                .map_err(|e| format!("请求 Qoder 模型目录失败: {}", e.without_url()))?;
            if !resp.status().is_success() {
                return Err(format!("Qoder 模型目录返回 HTTP {}", resp.status()));
            }
            let json: Value = resp
                .json()
                .await
                .map_err(|e| format!("解析 Qoder 模型目录失败: {e}"))?;
            let parsed = parse_qoder_model_list(&json);
            if parsed.is_empty() {
                Err("Qoder 模型目录解析结果为空".into())
            } else {
                Ok(parsed)
            }
        }
        "trae_cn" => {
            let (token_opt, _, _) = extract_token_and_extra(cred);
            let token = token_opt.ok_or_else(|| "Trae CN 凭据缺少 access token".to_string())?;
            let device_headers = if let CredentialPayload::LocalSnapshot { files } = &cred.payload {
                extract_trae_device_headers(files)
            } else {
                Vec::new()
            };
            let build_trae_req = |url: &str| {
                let mut req = client
                    .post(url)
                    .header("Authorization", format!("Cloud-IDE-JWT {token}"))
                    .header("X-Cloudide-Token", &token)
                    .header("x-ide-token", &token)
                    .header("x-uid", &cred.uid)
                    .header("x-app-id", TRAE_APP_ID)
                    .header("x-request-id", uuid::Uuid::new_v4().to_string())
                    .header("x-app-version", TRAE_SOLO_CLIENT_VERSION)
                    .header("x-app-version-code", TRAE_SOLO_CLIENT_VERSION_CODE)
                    .header("x-ide-version", TRAE_SOLO_CLIENT_VERSION)
                    .header("x-ide-version-code", TRAE_SOLO_CLIENT_VERSION_CODE)
                    .header("x-ide-version-type", "stable")
                    .header("Accept", "application/json")
                    .header("X-User-Region", "CN");
                for (name, value) in &device_headers {
                    if name.eq_ignore_ascii_case("x-app-version")
                        || name.eq_ignore_ascii_case("x-app-version-code")
                        || name.eq_ignore_ascii_case("x-ide-version")
                        || name.eq_ignore_ascii_case("x-ide-version-code")
                        || name.eq_ignore_ascii_case("x-app-id")
                        || name.eq_ignore_ascii_case("x-uid")
                        || name.eq_ignore_ascii_case("x-request-id")
                    {
                        continue;
                    }
                    req = req.header(*name, value);
                }
                req
            };

            let batch_url = "https://trae-api-cn.mchost.guru/api/ide/v1/batch_get_detail_param";
            let batch_resp = build_trae_req(batch_url)
                .json(&json!({
                    "functions": TRAE_CHAT_FUNCTIONS,
                    "agent_type": "",
                    "current_config_info": { "config_name": "", "is_custom_model": false },
                    "mode_type": 0,
                    "access_type": 0,
                    "ab_force_vids": "",
                    "ab_autotest_advanced_mode": 0,
                    "show_custom_model": true
                }))
                .send()
                .await
                .map_err(|e| format!("请求 Trae CN 模型目录失败: {}", e.without_url()))?;
            let batch_status = batch_resp.status();
            let mut parsed = if batch_status.is_success() {
                let json: Value = batch_resp
                    .json()
                    .await
                    .map_err(|e| format!("解析 Trae CN 模型目录失败: {e}"))?;
                parse_trae_detail_param(&json)
            } else {
                Vec::new()
            };

            // batch_get_detail_param 无结果时，按 Magpie 同款方式逐个拉取 get_detail_param
            if parsed.is_empty() {
                let single_url = "https://trae-api-cn.mchost.guru/api/ide/v1/get_detail_param";
                let mut combined = Vec::new();
                for func_name in TRAE_CHAT_FUNCTIONS {
                    let Ok(resp) = build_trae_req(single_url)
                        .json(&json!({
                            "function": func_name,
                            "config_names": Value::Null,
                            "need_prompt": false,
                            "current_config_info": Value::Null,
                            "poly_prompt": true,
                            "mode_type": Value::Null,
                            "agent_type": Value::Null
                        }))
                        .send()
                        .await
                    else {
                        continue;
                    };
                    if !resp.status().is_success() {
                        continue;
                    }
                    if let Ok(json) = resp.json::<Value>().await {
                        combined.extend(parse_trae_detail_param(&json));
                    }
                }
                parsed = dedup_model_options(combined);
            }

            if parsed.is_empty() {
                if !batch_status.is_success() {
                    return Err(format!("Trae CN 模型目录返回 HTTP {batch_status}"));
                }
                Err("Trae CN 模型目录解析结果为空".into())
            } else {
                let _ = TRAE_APP_VERSION;
                Ok(parsed)
            }
        }
        "minimax_code_cn" => {
            let (token_opt, _, _) = extract_token_and_extra(cred);
            let url = "https://agent.minimax.cn/mavis/api/v1/models?region=cn&buildEnv=prod";
            let send_minimax = |token: Option<&str>| {
                let mut req = client
                    .get(url)
                    .header("Accept", "application/json")
                    .header("User-Agent", "MiniMaxAgent")
                    .header("X-Mavis-Agent-Id", "main")
                    .header("X-Mavis-Timezone-Offset", "28800");
                if let Some(t) = token.filter(|s| !s.trim().is_empty()) {
                    req = req.bearer_auth(t);
                }
                req.send()
            };
            let mut resp = send_minimax(token_opt.as_deref())
                .await
                .map_err(|e| format!("请求 MiniMax 模型目录失败: {}", e.without_url()))?;
            if matches!(resp.status().as_u16(), 401 | 403)
                && token_opt.as_deref().is_some_and(|s| !s.trim().is_empty())
            {
                resp = send_minimax(None)
                    .await
                    .map_err(|e| format!("匿名回退请求 MiniMax 模型目录失败: {}", e.without_url()))?;
            }
            if !resp.status().is_success() {
                return Err(format!("MiniMax 模型目录返回 HTTP {}", resp.status()));
            }
            let json: Value = resp
                .json()
                .await
                .map_err(|e| format!("解析 MiniMax 模型目录失败: {e}"))?;
            let parsed = parse_minimax_models(&json);
            if parsed.is_empty() {
                Err("MiniMax 模型目录解析结果为空".into())
            } else {
                Ok(parsed)
            }
        }
        "custom_api_key" => {
            let (token_opt, base_opt, _) = extract_token_and_extra(cred);
            let token = token_opt.ok_or_else(|| "自定义 API Key 缺少密钥".to_string())?;
            let base = base_opt
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "https://api.openai.com".into());
            let anthropic_style = is_anthropic_style_base(&base);
            let mut all: Vec<UpstreamModelOption> = Vec::new();
            let mut next_after: Option<String> = None;
            for page in 0..10 {
                let url = build_models_endpoint(&base);
                let mut req = client.get(&url).header("Accept", "application/json");
                // 鉴权头按协议分开发送：同时发送 Bearer 与 x-api-key 会被部分严格
                // 中转站直接 400/403 拒收。
                if anthropic_style {
                    req = req
                        .header("x-api-key", &token)
                        .header("anthropic-version", "2023-06-01");
                } else {
                    req = req.bearer_auth(&token);
                }
                if let Some(after) = next_after.as_deref() {
                    req = req.query(&[("limit", "100"), ("after_id", after)]);
                }
                let resp = match req.send().await {
                    Ok(r) => r,
                    Err(e) => {
                        if page > 0 && !all.is_empty() {
                            break;
                        }
                        return Err(format!("请求自定义网关模型目录失败: {}", e.without_url()));
                    }
                };
                if !resp.status().is_success() {
                    if page > 0 && !all.is_empty() {
                        break;
                    }
                    return Err(format!("自定义网关模型目录返回 HTTP {}", resp.status()));
                }
                let json: Value = match resp.json().await {
                    Ok(j) => j,
                    Err(e) => {
                        if page > 0 && !all.is_empty() {
                            break;
                        }
                        return Err(format!("解析自定义网关模型目录失败: {e}"));
                    }
                };
                all.extend(parse_openai_models_list("custom_api_key", &json));
                // Anthropic 兼容分页
                let has_more = json
                    .get("has_more")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let last_id = json
                    .get("last_id")
                    .or_else(|| json.get("after_id"))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned);
                if !has_more {
                    break;
                }
                match last_id {
                    Some(id) if next_after.as_deref() != Some(id.as_str()) => {
                        next_after = Some(id);
                    }
                    _ => break,
                }
            }
            let deduped = dedup_model_options(all);
            if deduped.is_empty() {
                Err("自定义网关模型目录解析结果为空".into())
            } else {
                Ok(deduped)
            }
        }
        // 码道没有可调用的模型接口，改从本机客户端读取真实清单
        "codearts" => probe_codearts_client_models(),
        "lobsterai" => probe_lobsterai_models(&client, cred).await,
        // 豆包工作没有模型接口：读 magpie 插件缓存里该账号真实的「工作任务」菜单
        "doubaowork" => probe_doubao_work_models(),
        other => Err(format!("工具 {other} 暂不支持实时模型目录探测")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubao_work_models_parsed_from_magpie_cache() {
        // 结构取自 magpie 插件真实缓存（magpie-doubao-work/<app>/models.json）
        let raw = r#"{"v":2,"at":1795590825420,"models":{
            "auto":{"label":"自动","name":"自动","key":"9","efforts":["低","中","高"]},
            "doubao-2.1-pro":{"label":"豆包 2.1 Pro","key":"5"},
            "deepseek-v4.1-flash":{"label":"DeepSeek V4.1 Flash","key":"person-deepseek-v4-1-flash-261017"}
        }}"#;
        let models = doubao_models_from_cache(raw).expect("models");
        let mut ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["auto", "deepseek-v4.1-flash", "doubao-2.1-pro"]);
        assert!(models.iter().all(|m| m.source == "client"));
        let doubao_pro = models.iter().find(|m| m.id == "doubao-2.1-pro").unwrap();
        assert_eq!(doubao_pro.display_name, "豆包 2.1 Pro");
        // 上下文来自 models.dev 快照（如果有）
        let ds = models
            .iter()
            .find(|m| m.id == "deepseek-v4.1-flash")
            .unwrap();
        assert!(
            ds.context_window.is_some(),
            "DeepSeek V4.1 Flash 应有上下文窗口"
        );

        // 空 / 坏结构 → None，交给上层回退预设
        assert!(doubao_models_from_cache("{}").is_none());
        assert!(doubao_models_from_cache("not json").is_none());
    }

    #[test]
    fn lobsterai_models_parsed_from_available_endpoint() {
        let json = json!({
            "code": 0,
            "data": [
                { "presetId": "p1", "modelId": "glm-5.3", "displayName": "GLM-5.3" },
                { "presetId": "p2", "modelId": "kimi-k3.1-pro", "accessible": false },
                { "modelId": "deepseek-v4-pro" },
                { "modelId": "glm-5.3" },
                { "modelId": "auto" }
            ]
        });
        let models = parse_lobsterai_models(&json);
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["glm-5.3", "deepseek-v4-pro"],
            "不可用预设与噪声变体都要剔除，重复项去重"
        );
        assert!(models.iter().all(|m| m.source == "live"));
        assert_eq!(models[0].display_name, "GLM-5.3");

        // 业务错误码 / 空数据 → 空结果，交给上层回退
        assert!(parse_lobsterai_models(&json!({"code": 401, "data": []})).is_empty());
        assert!(parse_lobsterai_models(&json!({})).is_empty());
    }

    #[test]
    fn codearts_models_parsed_from_kernel_init_config() {
        // 码道没有模型接口，只能从内核启动配置里取真实清单
        let log = concat!(
            "2026-11-25 13:34:26 [INFO] [MainServe]: [init_post] config: ",
            r#"{"userName":"u","multimodalModels":["deepseek-v4.1-flash","glm-5.3-flash","kimi-k3"],"agent":{}}"#,
            "\n2026-11-25 13:34:27 [INFO] something else\n"
        );
        let models = codearts_models_from_kernel_log(log).expect("models");
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["deepseek-v4.1-flash", "glm-5.3-flash", "kimi-k3"]
        );
        assert!(models.iter().all(|m| m.source == "client"));
        // vendor wire id 必须原样保留（大小写敏感）
        assert_eq!(models[0].vendor_wire_id, "deepseek-v4.1-flash");

        // 没有 marker / JSON 损坏 → None，绝不返回猜测值
        assert!(codearts_models_from_kernel_log("no config here").is_none());
        assert!(codearts_models_from_kernel_log("[init_post] config: {not json}\n").is_none());
    }

    #[test]
    fn noise_filter_keeps_legitimate_max_models() {
        // 回归：`-max` 后缀是正常主力模型（Qwen-Max / DeepSeek-Max 等），
        // 历史实现把它们当作噪声滤掉，导致实时结果反而少于静态目录。
        for keep in [
            "qwen4-max",
            "qwen3.8-max",
            "Qwen3-Max",
            "deepseek-v4-max",
            "glm-5.3-max",
            "gpt-4-max",
        ] {
            assert!(
                !is_noise_variant_id(keep),
                "{keep} 是正常模型，不应被噪声过滤"
            );
        }
        for drop in [
            "auto",
            "default",
            "doubao-for-auto",
            "minimax-image",
            "search_agent_v2",
            "sub_agent",
        ] {
            assert!(is_noise_variant_id(drop), "{drop} 应被噪声过滤");
        }
    }

    #[test]
    fn parses_workbuddy_qoder_minimax_and_openai_model_payloads() {
        let wb_json = json!({
            "data": {
                "agents": [{ "name": "cli", "models": ["deepseek-v4-pro", "hy4flash-free", "glm-5.3"] }],
                "models": [
                    { "id": "deepseek-v4-pro", "name": "DeepSeek-V4-Pro", "rate": 0.51, "free": false },
                    { "id": "hy4flash-free", "name": "Hy4 flash", "rate": 0, "free": true },
                    { "id": "glm-5.3", "name": "GLM-5.3", "rate": 0.69, "free": false },
                    { "id": "qwen4-max", "name": "Qwen4 Max", "rate": 0.4, "free": false },
                    { "id": "unused-web-only", "name": "Unused", "rate": 1.0 }
                ]
            }
        });
        let wb = parse_workbuddy_v3_config(&wb_json);
        // 回归：命中 cli agent 后仍必须保留 data.models 中的**全部**模型
        // （含未被任何 agent 声明的 qwen4-max），不得再丢弃其余条目。
        assert_eq!(wb.len(), 5);
        assert_eq!(wb[0].id, "deepseek-v4-pro");
        assert_eq!(wb[0].credit_multiplier, Some(0.51));
        assert!(wb[1].is_free);
        assert_eq!(wb[1].id, "hy4flash-free");
        assert!(wb.iter().any(|m| m.id == "qwen4-max"));

        let qoder_json = json!({
            "data": {
                "chat": [
                    { "key": "dmodel", "display_name": "DeepSeek-V4-Pro", "enable": true, "price_factor": 0.5 },
                    { "key": "gmodel", "display_name": "GLM-5.3", "enable": true, "price_factor": 0.8 },
                    { "key": "kmodel_latest", "display_name": "Kimi-K3.1-Pro", "enable": false, "price_factor": 1.0 },
                    { "key": "auto", "display_name": "Auto", "enable": true, "price_factor": 0 }
                ],
                "code": [
                    { "key": "kmodel", "display_name": "Kimi-K2.8", "enable": true, "price_factor": 0.8 }
                ],
                "agent": [
                    { "key": "qmodel_38max", "display_name": "Qwen3.8 Max", "enable": true, "price_factor": 0.5 }
                ]
            }
        });
        let qd = parse_qoder_model_list(&qoder_json);
        // 回归：必须遍历全部场景桶（chat / code / agent），而非写死的 app+chat。
        assert_eq!(qd.len(), 4);
        assert_eq!(qd[0].id, "deepseek-v4-pro");
        assert_eq!(qd[0].vendor_wire_id, "dmodel");
        assert_eq!(qd[0].credit_multiplier, Some(0.5));
        assert_eq!(qd[1].id, "glm-5.3");
        assert_eq!(qd[1].vendor_wire_id, "gmodel");
        assert!(qd.iter().any(|m| m.vendor_wire_id == "kmodel"));
        assert!(qd.iter().any(|m| m.vendor_wire_id == "qmodel_38max"));

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

        // 回归：非 minimax provider 名与扁平 data.models 也必须被接受。
        let mm_flat = json!({
            "data": { "models": [
                { "id": "MiniMax-M3", "name": "M3", "type": "chat" },
                { "id": "MiniMax-Video", "name": "Video", "type": "video" }
            ] }
        });
        let mmf = parse_minimax_models(&mm_flat);
        assert_eq!(mmf.len(), 1);
        assert_eq!(mmf[0].id, "minimax-m3");
    }

    #[test]
    fn openai_parser_accepts_multiple_payload_shapes() {
        let data_shape = json!({ "data": [{ "id": "gpt-4o" }] });
        assert_eq!(
            parse_openai_models_list("custom_api_key", &data_shape).len(),
            1
        );

        let models_shape = json!({ "models": [{ "model": "glm-4" }] });
        assert_eq!(
            parse_openai_models_list("custom_api_key", &models_shape).len(),
            1
        );

        let items_shape = json!({ "items": [{ "name": "kimi-k2" }] });
        assert_eq!(
            parse_openai_models_list("custom_api_key", &items_shape).len(),
            1
        );

        let root_array = json!([{ "id": "deepseek-v3" }]);
        assert_eq!(
            parse_openai_models_list("custom_api_key", &root_array).len(),
            1
        );
    }

    #[test]
    fn models_endpoint_respects_versioned_base_urls() {
        assert_eq!(
            build_models_endpoint("https://api.openai.com"),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            build_models_endpoint("https://api.openai.com/v1"),
            "https://api.openai.com/v1/models"
        );
        // 回归：智谱 / 火山 / Gemini 的版本化路径不得被拼成 `/v4/v1/models`。
        assert_eq!(
            build_models_endpoint("https://open.bigmodel.cn/api/paas/v4"),
            "https://open.bigmodel.cn/api/paas/v4/models"
        );
        assert_eq!(
            build_models_endpoint("https://ark.cn-beijing.volces.com/api/v3"),
            "https://ark.cn-beijing.volces.com/api/v3/models"
        );
        assert_eq!(
            build_models_endpoint("https://generativelanguage.googleapis.com/v1beta/openai"),
            "https://generativelanguage.googleapis.com/v1beta/openai/models"
        );
    }

    #[test]
    fn qoder_snapshot_reads_real_machine_id_key() {
        // 回归：适配器写入的键名是 `auth.machine-id`，历史实现读 `machine_id`。
        let mut files = HashMap::new();
        files.insert(
            DECRYPTED_TOKEN_FILE.to_string(),
            BASE64.encode("qk-token".as_bytes()),
        );
        files.insert(
            QODER_MACHINE_ID_FILE.to_string(),
            BASE64.encode("machine-123".as_bytes()),
        );
        let cred = CredentialEnvelope {
            tool_id: "qoder_cn".into(),
            uid: "u1".into(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::LocalSnapshot { files },
        };
        let (token, _, machine_id) = extract_token_and_extra(&cred);
        assert_eq!(token.as_deref(), Some("qk-token"));
        assert_eq!(machine_id.as_deref(), Some("machine-123"));
    }

    #[test]
    fn workbuddy_snapshot_reads_real_auth_filename_and_enterprise_id() {
        // 回归：适配器写入 `workbuddy-desktop.info`，历史实现只认 `session.json`。
        let sess = json!({
            "auth": { "accessToken": "wb-token", "endpoint": "https://copilot.tencent.com", "domain": "copilot.tencent.com" },
            "account": { "uid": "42", "enterpriseId": "ent-99" }
        });
        let mut files = HashMap::new();
        files.insert(
            WORKBUDDY_AUTH_FILENAME.to_string(),
            BASE64.encode(serde_json::to_vec(&sess).unwrap()),
        );
        files.insert(
            DECRYPTED_TOKEN_FILE.to_string(),
            BASE64.encode("wb-plain-token".as_bytes()),
        );
        let cred = CredentialEnvelope {
            tool_id: "workbuddy".into(),
            uid: "42".into(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::LocalSnapshot { files },
        };
        let params = extract_workbuddy_probe_params(&cred).unwrap();
        let base = params.endpoint;
        let token = params.token;
        let uid = params.uid;
        let domain = params.domain;
        let enterprise_id = params.enterprise_id;
        assert_eq!(base, "https://copilot.tencent.com");
        assert_eq!(token, "wb-plain-token");
        assert_eq!(uid, "42");
        assert_eq!(domain.as_deref(), Some("copilot.tencent.com"));
        assert_eq!(enterprise_id.as_deref(), Some("ent-99"));
    }

    #[test]
    fn trae_snapshot_reads_auth_entries_token() {
        let entries = json!({
            "iCubeAuthInfo://icube.cloudide": "{\"token\":\"trae-jwt\",\"userId\":\"7\"}"
        });
        let mut files = HashMap::new();
        files.insert(
            TRAE_AUTH_ENTRIES_KEY.to_string(),
            BASE64.encode(serde_json::to_vec(&entries).unwrap()),
        );
        let cred = CredentialEnvelope {
            tool_id: "trae_cn".into(),
            uid: "7".into(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::LocalSnapshot { files },
        };
        let (token, _, _) = extract_token_and_extra(&cred);
        assert_eq!(token.as_deref(), Some("trae-jwt"));
    }

    #[test]
    fn parse_trae_detail_param_reads_config_name_and_skips_custom_models() {
        let body = json!({
            "function_configs": [
                {
                    "function": "solo_agent",
                    "config_info_list": [
                        {
                            "config_name": "DeepSeek-V4.1-Flash",
                            "display_name": "DeepSeek V4.1 Flash",
                            "is_custom_model": false,
                            "enabled": true
                        },
                        {
                            "config_name": "my-custom-slot",
                            "display_name": "Custom",
                            "is_custom_model": true,
                            "enabled": true
                        }
                    ]
                }
            ]
        });
        let parsed = parse_trae_detail_param(&body);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "deepseek-v4.1-flash");
        assert_eq!(parsed[0].vendor_wire_id, "DeepSeek-V4.1-Flash");
        assert_eq!(parsed[0].source, "live");
    }

    #[test]
    fn parse_minimax_models_reads_object_map_and_model_order() {
        let body = json!({
            "providers": [
                {
                    "providerId": "minimax",
                    "config": {
                        "model_order": ["MiniMax-M3.1-Flash", "MiniMax-M3"],
                        "models": {
                            "MiniMax-M3": {
                                "name": "MiniMax M3",
                                "limit": { "context": 512000, "output": 64000 }
                            },
                            "MiniMax-M3.1-Flash": {
                                "name": "MiniMax M3.1 Flash",
                                "limit": { "context": 256000, "output": 32000 }
                            },
                            "hailuo-video-01": {
                                "name": "Video",
                                "type": "video"
                            }
                        }
                    }
                }
            ]
        });
        let parsed = parse_minimax_models(&body);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, "minimax-m3.1-flash");
        assert_eq!(parsed[0].vendor_wire_id, "MiniMax-M3.1-Flash");
        assert_eq!(parsed[0].context_window, Some(256000));
        assert_eq!(parsed[0].max_output_tokens, Some(32000));
        assert_eq!(parsed[1].id, "minimax-m3");
        assert_eq!(parsed[1].context_window, Some(512000));
    }
}
