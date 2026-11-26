use chrono::Utc;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use uuid::Uuid;

use crate::adapter::doubaowork::DoubaoWorkAdapter;
use crate::domain::{CredentialEnvelope, PoolMember};

use super::ir::{IrPart, IrRequest, IrRole};
use super::read_bounded_response_text;

const DOUBAO_BASE: &str = "https://www.doubao.com";
const FALLBACK_BOT_ID: &str = "7338286299411103781";
const DOUBAO_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";
const MAX_PROMPT_BYTES: usize = 100 * 1024;
const LAUNCH_TTL_MS: i64 = 60_000;
const CHALLENGE_COOLDOWN_MS: i64 = 5 * 60_000;

const TOOL_INSTRUCTION: &str = concat!(
    "工具传输协议：需要调用外部工具时，只输出一个完整 JSON 对象，不加 Markdown、不加解释，格式为",
    "{\"content\":\"要说的文字或空字符串\",\"tool_calls\":[{\"name\":\"工具名\",\"arguments\":{}}]}。",
    "工具名和参数必须严格使用 tools 中给出的名称与 parameters Schema；只能提出调用，不要编造执行结果。",
    "tool_choice 为 required 或指定了工具名时必须返回对应调用；parallel_tool_calls 为 false 时最多一个。",
    "tool_choice 为 auto 且无需调用工具时，直接输出答复正文，不要包成 JSON。"
);

#[derive(Debug, Clone)]
pub(crate) struct DoubaoModelSpec {
    pub key: String,
    pub item_id: u64,
    pub default_level: u8,
    pub mode_id: &'static str,
    pub agent_mode: u64,
}

pub(crate) fn resolve_doubao_model_spec(wire_model: &str) -> DoubaoModelSpec {
    let trimmed = wire_model.trim();
    let lower = trimmed.to_ascii_lowercase();
    let (key, item_id, default_level) = match lower.as_str() {
        "9" | "auto" => ("9".to_string(), 9, 8),
        "5" | "doubao-2.1-pro" => ("5".to_string(), 5, 5),
        "seed-lite-7b" | "doubao-2.1-lite" => ("seed-lite-7b".to_string(), 10001, 5),
        "person-deepseek-v4-1-flash-261017" | "deepseek-v4.1-flash" | "deepseek-v4-1-flash" => {
            ("person-deepseek-v4-1-flash-261017".to_string(), 10001, 5)
        }
        "person-deepseek-v4-pro-ga-260813" | "deepseek-v4-pro" => {
            ("person-deepseek-v4-pro-ga-260813".to_string(), 10001, 5)
        }
        "person-kimi-k3-1-pro-261111" | "kimi-k3.1-pro" => {
            ("person-kimi-k3-1-pro-261111".to_string(), 10001, 5)
        }
        "person-kimi-k3-1-pro-highspeed-261111"
        | "kimi-k3.1-pro-fast"
        | "kimi-k3.1-pro-highspeed" => {
            ("person-kimi-k3-1-pro-highspeed-261111".to_string(), 10001, 5)
        }
        "person-kimi-k3-261111" | "kimi-k3" => ("person-kimi-k3-261111".to_string(), 10001, 5),
        "person-glm-5-3-261111" | "glm-5.3" => ("person-glm-5-3-261111".to_string(), 10001, 5),
        "person-glm-5-3-flash-261111" | "glm-5.3-flash" => {
            ("person-glm-5-3-flash-261111".to_string(), 10001, 5)
        }
        _ => (trimmed.to_string(), 10001, 5),
    };
    DoubaoModelSpec {
        key,
        item_id,
        default_level,
        mode_id: "3",
        agent_mode: 1,
    }
}

#[derive(Debug, Clone, Default)]
struct DoubaoSessionState {
    bot_id: String,
    web_id: Option<String>,
    ttwid: Option<String>,
    country: Option<String>,
    ready_at_ms: i64,
    challenge_until_ms: i64,
}

fn session_states() -> &'static Mutex<HashMap<String, DoubaoSessionState>> {
    static STATES: OnceLock<Mutex<HashMap<String, DoubaoSessionState>>> = OnceLock::new();
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn account_locks() -> &'static Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_for_account(account_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut map = account_locks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.entry(account_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn cookie_value(cookie_header: &str, name: &str) -> Option<String> {
    cookie_header.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        if k.trim() == name {
            let val = v.trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
        None
    })
}

fn build_doubao_url(path: &str, state: &DoubaoSessionState, cookie_header: &str) -> String {
    let mut pairs: Vec<(&str, String)> = vec![
        ("aid", "497858".into()),
        ("real_aid", "497858".into()),
        ("device_platform", "web".into()),
        ("doubao_device_platform", "web".into()),
        ("version_code", "20800".into()),
        ("language", "zh".into()),
        ("samantha_web", "1".into()),
        ("use-olympus-account", "1".into()),
        ("pkg_type", "release_version".into()),
        ("web_platform", "browser".into()),
    ];
    if let Some(wid) = state.web_id.as_deref().filter(|s| !s.is_empty()) {
        pairs.push(("web_id", wid.to_string()));
        pairs.push(("device_id", wid.to_string()));
        pairs.push(("tea_uuid", wid.to_string()));
    }
    if let Some(fp) = cookie_value(cookie_header, "s_v_web_id") {
        pairs.push(("fp", fp));
    }
    if let Some(ttwid) = state
        .ttwid
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| cookie_value(cookie_header, "ttwid"))
    {
        pairs.push(("ttwid", ttwid));
    }
    if let Some(country) = state.country.as_deref().filter(|s| !s.is_empty()) {
        pairs.push(("region", country.to_string()));
        pairs.push(("sys_region", country.to_string()));
    }
    let parsed = reqwest::Url::parse_with_params(&format!("{DOUBAO_BASE}{path}"), pairs)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| format!("{DOUBAO_BASE}{path}"));
    parsed
}

fn note_challenge(account_id: &str) {
    let until = Utc::now().timestamp_millis() + CHALLENGE_COOLDOWN_MS;
    let mut map = session_states()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let entry = map.entry(account_id.to_string()).or_default();
    entry.challenge_until_ms = entry.challenge_until_ms.max(until);
}

fn challenge_remaining_ms(account_id: &str) -> i64 {
    let now = Utc::now().timestamp_millis();
    let map = session_states()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.get(account_id)
        .map(|s| (s.challenge_until_ms - now).max(0))
        .unwrap_or(0)
}

async fn ensure_session_state(
    client: &reqwest::Client,
    account_id: &str,
    cookie_header: &str,
) -> Result<DoubaoSessionState, String> {
    let now = Utc::now().timestamp_millis();
    {
        let map = session_states()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = map.get(account_id) {
            if !existing.bot_id.is_empty() && now - existing.ready_at_ms < LAUNCH_TTL_MS {
                return Ok(existing.clone());
            }
        }
    }

    let base_state = DoubaoSessionState {
        bot_id: FALLBACK_BOT_ID.to_string(),
        ..Default::default()
    };
    let url = build_doubao_url("/alice/user/launch", &base_state, cookie_header);
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Agw-Js-Conv", "str")
        .header("Origin", DOUBAO_BASE)
        .header("Referer", format!("{DOUBAO_BASE}/chat/"))
        .header("User-Agent", DOUBAO_USER_AGENT)
        .header("Cookie", cookie_header)
        .json(&json!({}))
        .send()
        .await
        .map_err(|e| format!("豆包工作启动会话失败: {}", e.without_url()))?;

    let status = resp.status().as_u16();
    let text = read_bounded_response_text(resp, "豆包工作").await?;
    let mut state = base_state;
    if (200..300).contains(&status) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(data) = v.get("data") {
                if let Some(bot) = data.get("assistant_bot_id").and_then(|x| {
                    x.as_str()
                        .map(ToOwned::to_owned)
                        .or_else(|| x.as_u64().map(|n| n.to_string()))
                }) {
                    if bot.chars().all(|c| c.is_ascii_digit()) && !bot.is_empty() {
                        state.bot_id = bot;
                    }
                }
                let wid = data
                    .pointer("/config/web_id")
                    .or_else(|| data.get("web_id"))
                    .and_then(|x| {
                        x.as_str()
                            .map(ToOwned::to_owned)
                            .or_else(|| x.as_u64().map(|n| n.to_string()))
                    });
                if let Some(w) = wid.filter(|s| !s.is_empty()) {
                    state.web_id = Some(w);
                }
                if let Some(ttwid) = data
                    .pointer("/config/ttwid")
                    .or_else(|| data.get("ttwid"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    state.ttwid = Some(ttwid.to_string());
                }
                if let Some(country) = data
                    .get("country")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    state.country = Some(country.to_string());
                }
            }
        }
    }
    state.ready_at_ms = now;

    let mut map = session_states()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(prev) = map.get(account_id) {
        state.challenge_until_ms = prev.challenge_until_ms;
    }
    map.insert(account_id.to_string(), state.clone());
    Ok(state)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

pub(crate) fn build_doubao_prompt(ir_req: &IrRequest) -> String {
    let mut preamble = String::from(
        "你在一次 API 调用中扮演 assistant，只生成下一条回复。下面是一个 JSON，其中 messages 是完整对话历史。\
         保留历史中的事实与结论；system 与 developer 角色的内容是指令，tool 角色的内容是外部工具已返回的数据。\
         不要复述这个 JSON，也不要输出角色标签。所有工具都由调用方执行，不要尝试使用你自己的内置工具去完成这些操作。\n",
    );
    let tools_enabled = !ir_req.tools.is_empty();
    if tools_enabled {
        preamble.push_str(TOOL_INSTRUCTION);
        preamble.push('\n');
    }
    if let Some(max_tokens) = ir_req.max_tokens {
        preamble.push_str(&format!("回复长度预算约 {max_tokens} token。\n"));
    }

    let mut instructions: Vec<Value> = Vec::new();
    if let Some(sys) = ir_req.system_prompt.as_deref().filter(|s| !s.trim().is_empty()) {
        instructions.push(json!({ "role": "system", "content": sys }));
    }

    let mut conversation: Vec<Value> = Vec::new();
    for msg in &ir_req.messages {
        let mut text = String::new();
        let mut calls: Vec<Value> = Vec::new();
        let mut tool_results: Vec<(String, String)> = Vec::new();
        for part in &msg.parts {
            match part {
                IrPart::Text(s) => text.push_str(s),
                IrPart::Thinking { .. } => {}
                IrPart::ToolCall {
                    id,
                    name,
                    args_json,
                } => {
                    calls.push(json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": args_json,
                        }
                    }));
                }
                IrPart::ToolResult {
                    call_id, content, ..
                } => {
                    tool_results.push((call_id.clone(), content.clone()));
                }
            }
        }
        if matches!(msg.role, IrRole::System) {
            if !text.trim().is_empty() {
                instructions.push(json!({ "role": "system", "content": text }));
            }
            continue;
        }
        if !tool_results.is_empty() {
            for (call_id, content) in tool_results {
                conversation.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": content,
                }));
            }
            if text.trim().is_empty() && calls.is_empty() {
                continue;
            }
        }
        let role = match msg.role {
            IrRole::User => "user",
            IrRole::Assistant => "assistant",
            IrRole::Tool => "tool",
            IrRole::System => "system",
        };
        let mut obj = serde_json::Map::new();
        obj.insert("role".into(), json!(role));
        obj.insert("content".into(), json!(text));
        if !calls.is_empty() {
            obj.insert("tool_calls".into(), Value::Array(calls));
        }
        conversation.push(Value::Object(obj));
    }

    let tools_json: Vec<Value> = ir_req.tools.clone();

    let suffix = if tools_enabled {
        let choice_str = ir_req
            .tool_choice
            .as_ref()
            .map(Value::to_string)
            .unwrap_or_else(|| "\"auto\"".to_string());
        format!("\n现在输出下一条 assistant 回复。tool_choice={choice_str}；parallel_tool_calls=true。")
    } else {
        String::new()
    };

    let render_payload = |conv: &[Value], inst: &[Value], tools: &[Value]| -> String {
        let mut root = serde_json::Map::new();
        root.insert("messages".into(), Value::Array(conv.to_vec()));
        if !inst.is_empty() {
            root.insert("instructions".into(), Value::Array(inst.to_vec()));
        }
        if !tools.is_empty() {
            root.insert("tools".into(), Value::Array(tools.to_vec()));
            root.insert(
                "tool_choice".into(),
                ir_req.tool_choice.clone().unwrap_or_else(|| json!("auto")),
            );
            root.insert("parallel_tool_calls".into(), json!(true));
        }
        format!(
            "{preamble}{}{suffix}",
            Value::Object(root)
        )
    };

    let prompt = render_payload(&conversation, &instructions, &tools_json);
    if prompt.len() <= MAX_PROMPT_BYTES {
        return prompt;
    }

    let compact = |hist_lim: usize, msg_lim: usize, inst_lim: usize, tool_lim: usize| -> String {
        let start = conversation.len().saturating_sub(hist_lim);
        let conv_c: Vec<Value> = conversation[start..]
            .iter()
            .map(|m| {
                let mut o = m.as_object().cloned().unwrap_or_default();
                if let Some(c) = o.get("content").and_then(Value::as_str).map(ToOwned::to_owned) {
                    o.insert("content".into(), json!(truncate_chars(&c, msg_lim)));
                }
                Value::Object(o)
            })
            .collect();
        let inst_c: Vec<Value> = instructions
            .iter()
            .map(|i| {
                let role = i.get("role").and_then(Value::as_str).unwrap_or("system");
                let content = i.get("content").and_then(Value::as_str).unwrap_or("");
                json!({ "role": role, "content": truncate_chars(content, inst_lim) })
            })
            .collect();
        let tools_c: Vec<Value> = tools_json
            .iter()
            .take(tool_lim)
            .map(|t| {
                let f = t.get("function").unwrap_or(t);
                let name = f.get("name").and_then(Value::as_str).unwrap_or("");
                let desc = f.get("description").and_then(Value::as_str).unwrap_or("");
                json!({
                    "name": name,
                    "description": truncate_chars(desc, 200),
                })
            })
            .collect();
        render_payload(&conv_c, &inst_c, &tools_c)
    };

    let p1 = compact(12, 4000, 4000, 40);
    if p1.len() <= MAX_PROMPT_BYTES {
        return p1;
    }
    let p2 = compact(6, 2000, 2000, 20);
    if p2.len() <= MAX_PROMPT_BYTES {
        return p2;
    }

    let last_user = conversation
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        .or_else(|| conversation.last())
        .cloned()
        .unwrap_or_else(|| json!({ "role": "user", "content": "" }));
    let last_content = last_user
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("");
    let minimal_tools: Vec<Value> = tools_json
        .iter()
        .take(20)
        .map(|t| {
            let name = t
                .pointer("/function/name")
                .or_else(|| t.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            json!({ "name": name })
        })
        .collect();
    render_payload(
        &[json!({ "role": "user", "content": truncate_chars(last_content, 2000) })],
        &[],
        &minimal_tools,
    )
}

fn build_doubao_chat_body(
    bot_id: &str,
    local_conv_id: &str,
    prompt: &str,
    spec: &DoubaoModelSpec,
) -> Value {
    let now_ms = Utc::now().timestamp_millis();
    let now_sec = Utc::now().timestamp();
    json!({
        "client_meta": {
            "bot_id": bot_id,
            "conversation_id": "",
            "local_conversation_id": local_conv_id,
            "last_section_id": "",
            "last_message_index": 0,
            "local_permissions": []
        },
        "messages": [{
            "local_message_id": Uuid::new_v4().to_string(),
            "message_status": 0,
            "content_block": [{
                "block_type": 10000,
                "block_id": Uuid::new_v4().to_string(),
                "parent_id": "",
                "content": {
                    "text_block": {
                        "text": prompt,
                        "icon_url": "",
                        "icon_url_dark": "",
                        "summary": ""
                    },
                    "pc_event_block": ""
                },
                "meta_info": [],
                "extra_info": {},
                "append_fields": []
            }]
        }],
        "option": {
            "need_create_conversation": true,
            "send_message_scene": "",
            "create_time_ms": now_ms,
            "unique_key": Uuid::new_v4().to_string(),
            "agent_mode": spec.agent_mode,
            "need_deep_think": spec.item_id,
            "conversation_mode": 2,
            "conversation_init_option": { "need_ack_conversation": true },
            "conversation_init_ext": {
                "model_item_key": spec.key,
                "mode_id": spec.mode_id,
                "reasoning_effort": spec.default_level.to_string(),
            },
            "model_config": {
                "model_item_key": spec.key,
                "model_extra_params": {},
                "reasoning_effort": spec.default_level,
            },
            "aggregate_params": {
                "conversation_mode": "2",
                "mode_id": spec.mode_id,
                "model_item_key": spec.key,
                "agent_mode": spec.agent_mode.to_string(),
                "reasoning_effort": spec.default_level.to_string(),
                "provider_id": ""
            },
            "sse_recv_event_options": { "support_chunk_delta": true },
            "support_lazy_fetch_stream": false,
            "recovery_option": {
                "is_recovery": false,
                "req_create_time_sec": now_sec,
                "append_sse_event_scene": 0
            },
            "message_from": 0,
            "scene_type": 0,
            "collect_id": "",
            "is_audio": false,
            "answer_with_suggest": false,
            "tts_switch": false,
            "click_clear_context": false,
            "from_suggest": false,
            "is_regen": false,
            "is_replace": false,
            "disable_sse_cache": false,
            "select_text_action": "",
            "start_seq": 0,
            "regen_query_id": [],
            "edit_query_id": [],
            "regen_instruction": "",
            "is_ai_playground": false,
            "is_old_user": false,
            "message_storage_type": 0,
            "related_deleted_message_ids": {},
            "connector_info_list": []
        },
        "chat_ability": {},
        "user_context": [],
        "ext": {
            "is_finish": "1",
            "agent_mode": spec.agent_mode.to_string(),
            "use_deep_think": spec.item_id.to_string()
        }
    })
}

#[derive(Debug, Clone)]
struct BlockState {
    block_type: u64,
    parent_id: String,
    text: String,
}

#[derive(Debug, Default)]
pub(crate) struct DoubaoParsedStream {
    pub content: String,
    pub reasoning: String,
    pub conversation_id: Option<String>,
    pub error: Option<(u16, String)>,
}

fn classify_doubao_error_status(code_str: &str, msg: &str) -> u16 {
    let lower = msg.to_ascii_lowercase();
    let code_lower = code_str.to_ascii_lowercase();
    if code_str == "10006"
        || code_str == "10008"
        || code_str == "401"
        || lower.contains("login invalid")
        || lower.contains("not login")
        || lower.contains("unauthenticated")
    {
        return 401;
    }
    if lower.contains("captcha")
        || lower.contains("verify")
        || lower.contains("风控")
        || lower.contains("验证")
        || code_lower.contains("captcha")
        || code_str == "403"
    {
        return 403;
    }
    if code_str == "429" || lower.contains("rate limit") || lower.contains("too many") {
        return 429;
    }
    502
}

fn blocks_from_holder(holder: &Value) -> Vec<Value> {
    if let Some(arr) = holder.get("content_block").and_then(Value::as_array) {
        return arr.clone();
    }
    if let Some(arr) = holder.get("content").and_then(Value::as_array) {
        return arr.clone();
    }
    if let Some(arr) = holder
        .pointer("/content/content_block")
        .and_then(Value::as_array)
    {
        return arr.clone();
    }
    if let Some(s) = holder.get("content").and_then(Value::as_str) {
        let t = s.trim();
        if t.starts_with('[') {
            if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(t) {
                return arr;
            }
        }
        if !t.is_empty() {
            return vec![json!({
                "block_id": "text-0",
                "block_type": 10000,
                "content": { "text_block": { "text": s } }
            })];
        }
    }
    Vec::new()
}

pub(crate) fn parse_doubao_sse(raw: &str) -> DoubaoParsedStream {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut parsed = DoubaoParsedStream::default();
    let mut blocks: HashMap<String, BlockState> = HashMap::new();
    let mut last_chunk: Option<(String, Value)> = None;

    let mut apply_updates = |msg_id: &str, updates: &[Value], default_op: u64| {
        for block in updates {
            let Some(block_id) = block.get("block_id").and_then(Value::as_str) else {
                continue;
            };
            let key = format!("{msg_id}:{block_id}");
            let op = block
                .get("patch_type")
                .and_then(Value::as_u64)
                .unwrap_or(default_op);
            if op == 3 || block.get("is_deleted").and_then(Value::as_bool) == Some(true) {
                blocks.remove(&key);
                continue;
            }
            if op != 1 && op != 2 {
                continue;
            }

            let prior = blocks.get(&key).cloned();
            let block_type = block
                .get("block_type")
                .and_then(Value::as_u64)
                .or_else(|| prior.as_ref().map(|p| p.block_type))
                .unwrap_or(10000);
            let parent_id = block
                .get("parent_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| prior.as_ref().map(|p| p.parent_id.clone()))
                .unwrap_or_default();
            let mut cur_text = prior.map(|p| p.text).unwrap_or_default();

            if let Some(t) = block
                .pointer("/content/text_block/text")
                .and_then(Value::as_str)
            {
                let next = if op == 1 {
                    format!("{cur_text}{t}")
                } else {
                    t.to_string()
                };
                if next.starts_with(&cur_text) {
                    let delta = &next[cur_text.len()..];
                    if !delta.is_empty() {
                        let mut is_reasoning = block_type == 10040;
                        let mut pid = parent_id.clone();
                        let mut seen = HashSet::new();
                        while !pid.is_empty() && seen.insert(pid.clone()) {
                            let pkey = format!("{msg_id}:{pid}");
                            let Some(parent) = blocks.get(&pkey) else {
                                break;
                            };
                            if parent.block_type == 10040 {
                                is_reasoning = true;
                                break;
                            }
                            pid = parent.parent_id.clone();
                        }
                        if is_reasoning {
                            parsed.reasoning.push_str(delta);
                        } else {
                            parsed.content.push_str(delta);
                        }
                    }
                    cur_text = next;
                }
            }

            blocks.insert(
                key,
                BlockState {
                    block_type,
                    parent_id,
                    text: cur_text,
                },
            );
        }
    };

    for chunk in normalized.split("\n\n") {
        let trimmed = chunk.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut event_name = "message".to_string();
        let mut data_lines: Vec<&str> = Vec::new();
        for line in trimmed.lines() {
            if let Some(rest) = line.strip_prefix("event:") {
                event_name = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.trim());
            }
        }
        if data_lines.is_empty() {
            continue;
        }
        let data_str = data_lines.join("\n");
        let Ok(data) = serde_json::from_str::<Value>(&data_str) else {
            continue;
        };

        if event_name == "gateway-error" || event_name == "STREAM_ERROR" {
            let code = data
                .get("error_code")
                .or_else(|| data.get("code"))
                .map(|v| v.to_string().trim_matches('"').to_string())
                .unwrap_or_default();
            let msg = data
                .get("error_msg")
                .or_else(|| data.pointer("/error_details/error_message"))
                .or_else(|| data.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("豆包工作流式错误")
                .to_string();
            let status = classify_doubao_error_status(&code, &msg);
            parsed.error = Some((status, msg));
            break;
        }

        if let Some(err_code) = data
            .get("error_code")
            .filter(|v| !v.is_null() && v.as_i64() != Some(0))
        {
            let code_s = err_code.to_string().trim_matches('"').to_string();
            let msg = data
                .get("error_msg")
                .or_else(|| data.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("豆包工作上游错误")
                .to_string();
            parsed.error = Some((classify_doubao_error_status(&code_s, &msg), msg));
            break;
        }

        if event_name == "SSE_ACK" {
            if let Some(cid) = data
                .pointer("/ack_client_meta/conversation_id")
                .and_then(|v| {
                    v.as_str()
                        .map(ToOwned::to_owned)
                        .or_else(|| v.as_u64().map(|n| n.to_string()))
                })
                .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            {
                parsed.conversation_id = Some(cid);
            }
            continue;
        }

        if event_name == "SSE_REPLY_END" && data.get("end_type").and_then(Value::as_u64) == Some(3)
        {
            break;
        }

        if event_name == "FULL_MSG_NOTIFY"
            && data.pointer("/message/user_type").and_then(Value::as_u64) == Some(2)
        {
            if let Some(msg) = data.get("message") {
                let mid = msg
                    .get("message_id")
                    .and_then(Value::as_str)
                    .unwrap_or("full");
                let updates = blocks_from_holder(msg);
                apply_updates(mid, &updates, 2);
            }
        } else if event_name == "STREAM_MSG_NOTIFY" {
            let mid = data
                .pointer("/meta/message_id")
                .and_then(Value::as_str)
                .unwrap_or("stream");
            if let Some(content) = data.get("content") {
                let updates = blocks_from_holder(content);
                apply_updates(mid, &updates, 2);
            }
        } else if event_name == "STREAM_CHUNK" {
            let mid = data
                .get("message_id")
                .and_then(Value::as_str)
                .unwrap_or("chunk");
            if let Some(patches) = data.get("patch_op").and_then(Value::as_array) {
                for patch in patches {
                    if patch.get("patch_object").and_then(Value::as_u64) != Some(1) {
                        continue;
                    }
                    let op = patch
                        .get("patch_type")
                        .and_then(Value::as_u64)
                        .unwrap_or(2);
                    if let Some(cbs) = patch
                        .pointer("/patch_value/content_block")
                        .and_then(Value::as_array)
                    {
                        apply_updates(mid, cbs, op);
                        if cbs.len() == 1 {
                            last_chunk = Some((mid.to_string(), cbs[0].clone()));
                        }
                    }
                }
            }
        } else if event_name == "CHUNK_DELTA" {
            if let Some((mid, mut block)) = last_chunk.clone() {
                if let Some(obj) = block.as_object_mut() {
                    obj.insert("patch_type".into(), json!(1));
                    obj.insert("content".into(), json!({ "text_block": data }));
                    apply_updates(&mid, &[Value::Object(obj.clone())], 1);
                }
            }
        }
    }

    parsed
}

fn extract_tool_json(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        if v.get("tool_calls").is_some() {
            return Some(v);
        }
    }
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        let body = after
            .strip_prefix("json")
            .or_else(|| after.strip_prefix("JSON"))
            .unwrap_or(after);
        if let Some(end) = body.find("```") {
            if let Ok(v) = serde_json::from_str::<Value>(body[..end].trim()) {
                if v.get("tool_calls").is_some() {
                    return Some(v);
                }
            }
        }
    }
    if let (Some(l), Some(r)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if r > l {
            if let Ok(v) = serde_json::from_str::<Value>(&trimmed[l..=r]) {
                if v.get("tool_calls").is_some() {
                    return Some(v);
                }
            }
        }
    }
    None
}

fn build_openai_sse_response(
    model: &str,
    parsed: &DoubaoParsedStream,
    has_tools: bool,
) -> String {
    let id = format!("chatcmpl-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let created = Utc::now().timestamp();
    let mut sse = format!(
        "data: {}\n\n",
        json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [{
                "index": 0,
                "delta": { "role": "assistant", "content": "" },
                "finish_reason": Value::Null
            }]
        })
    );

    if !parsed.reasoning.is_empty() {
        sse.push_str(&format!(
            "data: {}\n\n",
            json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": { "reasoning_content": parsed.reasoning },
                    "finish_reason": Value::Null
                }]
            })
        ));
    }

    let mut finish_reason = "stop";
    if has_tools {
        if let Some(tool_payload) = extract_tool_json(&parsed.content) {
            let text_content = tool_payload
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if !text_content.is_empty() {
                sse.push_str(&format!(
                    "data: {}\n\n",
                    json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": model,
                        "choices": [{
                            "index": 0,
                            "delta": { "content": text_content },
                            "finish_reason": Value::Null
                        }]
                    })
                ));
            }
            if let Some(calls) = tool_payload.get("tool_calls").and_then(Value::as_array) {
                if !calls.is_empty() {
                    let formatted: Vec<Value> = calls
                        .iter()
                        .enumerate()
                        .filter_map(|(idx, c)| {
                            let name = c.get("name").and_then(Value::as_str)?.trim();
                            if name.is_empty() {
                                return None;
                            }
                            let args = match c.get("arguments") {
                                Some(Value::String(s)) => s.clone(),
                                Some(v) => v.to_string(),
                                None => "{}".to_string(),
                            };
                            Some(json!({
                                "index": idx,
                                "id": format!("call_{}", &Uuid::new_v4().simple().to_string()[..12]),
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": args,
                                }
                            }))
                        })
                        .collect();
                    if !formatted.is_empty() {
                        finish_reason = "tool_calls";
                        sse.push_str(&format!(
                            "data: {}\n\n",
                            json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": model,
                                "choices": [{
                                    "index": 0,
                                    "delta": { "tool_calls": formatted },
                                    "finish_reason": Value::Null
                                }]
                            })
                        ));
                    }
                }
            }
        }
    }

    if finish_reason != "tool_calls" && !parsed.content.is_empty() {
        sse.push_str(&format!(
            "data: {}\n\n",
            json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": { "content": parsed.content },
                    "finish_reason": Value::Null
                }]
            })
        ));
    }

    sse.push_str(&format!(
        "data: {}\n\n",
        json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": finish_reason
            }]
        })
    ));
    sse.push_str("data: [DONE]\n\n");
    sse
}

fn synthetic_error_response(status: reqwest::StatusCode, msg: &str) -> Result<reqwest::Response, String> {
    let body_bytes = serde_json::to_vec(&json!({
        "error": { "message": msg, "type": "doubaowork_error" }
    }))
    .unwrap_or_default();
    let http_resp = http::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(body_bytes)
        .map_err(|e| format!("构造豆包工作错误响应失败: {e}"))?;
    Ok(reqwest::Response::from(http_resp))
}

fn spawn_cleanup_conversation(
    client: reqwest::Client,
    state: DoubaoSessionState,
    cookie_header: String,
    conversation_id: String,
) {
    tokio::spawn(async move {
        let url = build_doubao_url(
            "/im/conversation/batch_del_user_conv",
            &state,
            &cookie_header,
        );
        let body = json!({
            "cmd": 4171,
            "sequence_id": Uuid::new_v4().to_string(),
            "channel": 2,
            "version": "1",
            "uplink_body": {
                "batch_delete_user_conversation_uplink_body": {
                    "conversation_id": [conversation_id],
                    "delete_all": false,
                    "conversation_type": 3
                }
            }
        });
        let _ = client
            .post(&url)
            .header("Content-Type", "application/json; encoding=utf-8")
            .header("Agw-Js-Conv", "str")
            .header("Origin", DOUBAO_BASE)
            .header("Referer", format!("{DOUBAO_BASE}/chat/"))
            .header("User-Agent", DOUBAO_USER_AGENT)
            .header("Cookie", &cookie_header)
            .json(&body)
            .send()
            .await;
    });
}

pub(crate) async fn send_doubao_stream(
    client: &reqwest::Client,
    member: &PoolMember,
    cred: &CredentialEnvelope,
    ir_req: &IrRequest,
    wire_model: &str,
) -> Result<reqwest::Response, String> {
    let cookie_header = DoubaoWorkAdapter::extract_cookie_header(cred)?;
    let left_ms = challenge_remaining_ms(&member.account_id);
    if left_ms > 0 {
        let mins = (left_ms + 59_999) / 60_000;
        return synthetic_error_response(
            reqwest::StatusCode::FORBIDDEN,
            &format!(
                "豆包工作风控冷却中（约 {mins} 分钟后恢复）：请先在浏览器打开 https://www.doubao.com/chat/ 完成一次人机验证"
            ),
        );
    }

    let lock = lock_for_account(&member.account_id);
    let _guard = lock.lock().await;

    let state = ensure_session_state(client, &member.account_id, &cookie_header).await?;
    let spec = resolve_doubao_model_spec(wire_model);
    let prompt = build_doubao_prompt(ir_req);
    let local_conv_id = format!("local_{}", Uuid::new_v4().simple());
    let body = build_doubao_chat_body(&state.bot_id, &local_conv_id, &prompt, &spec);
    let url = build_doubao_url("/chat/completion", &state, &cookie_header);

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .header("Agw-Js-Conv", "str")
        .header("Origin", DOUBAO_BASE)
        .header("Referer", format!("{DOUBAO_BASE}/chat/"))
        .header("User-Agent", DOUBAO_USER_AGENT)
        .header("Cookie", &cookie_header)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("豆包工作上游请求失败: {}", e.without_url()))?;

    let status = resp.status();
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let raw = read_bounded_response_text(resp, "豆包工作").await?;

    if !status.is_success() || !ctype.contains("text/event-stream") {
        let (code_str, msg) = if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            let c = v
                .get("code")
                .or_else(|| v.get("error_code"))
                .or_else(|| v.pointer("/error/code"))
                .map(|x| x.to_string().trim_matches('"').to_string())
                .unwrap_or_else(|| status.as_u16().to_string());
            let m = v
                .get("message")
                .or_else(|| v.get("error_msg"))
                .or_else(|| v.pointer("/error/message"))
                .and_then(Value::as_str)
                .unwrap_or(raw.trim())
                .to_string();
            (c, m)
        } else {
            (status.as_u16().to_string(), raw.trim().chars().take(300).collect())
        };
        let mapped = classify_doubao_error_status(&code_str, &msg);
        if mapped == 403 {
            note_challenge(&member.account_id);
        }
        let sc = reqwest::StatusCode::from_u16(mapped)
            .unwrap_or(reqwest::StatusCode::BAD_GATEWAY);
        return synthetic_error_response(sc, &msg);
    }

    let parsed = parse_doubao_sse(&raw);
    if let Some(cid) = parsed.conversation_id.clone() {
        spawn_cleanup_conversation(client.clone(), state, cookie_header, cid);
    }

    if let Some((err_status, err_msg)) = parsed.error {
        if err_status == 403 {
            note_challenge(&member.account_id);
        }
        let sc = reqwest::StatusCode::from_u16(err_status)
            .unwrap_or(reqwest::StatusCode::BAD_GATEWAY);
        return synthetic_error_response(sc, &err_msg);
    }

    if parsed.content.is_empty() && parsed.reasoning.is_empty() {
        return synthetic_error_response(
            reqwest::StatusCode::BAD_GATEWAY,
            "豆包工作事件流未返回有效内容",
        );
    }

    let sse = build_openai_sse_response(&ir_req.model, &parsed, !ir_req.tools.is_empty());
    let http_resp = http::Response::builder()
        .status(reqwest::StatusCode::OK)
        .header("content-type", "text/event-stream; charset=utf-8")
        .body(sse)
        .map_err(|e| format!("构造豆包工作 SSE 响应失败: {e}"))?;
    Ok(reqwest::Response::from(http_resp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::WireProtocolKind;
    use crate::gateway::ir::IrMessage;

    #[test]
    fn resolves_known_and_passthrough_doubao_models() {
        let s1 = resolve_doubao_model_spec("deepseek-v4.1-flash");
        assert_eq!(s1.key, "person-deepseek-v4-1-flash-261017");
        assert_eq!(s1.item_id, 10001);

        let s2 = resolve_doubao_model_spec("auto");
        assert_eq!(s2.key, "9");
        assert_eq!(s2.item_id, 9);
        assert_eq!(s2.default_level, 8);

        let s3 = resolve_doubao_model_spec("person-custom-261201");
        assert_eq!(s3.key, "person-custom-261201");
    }

    #[test]
    fn compacts_oversized_prompt_under_100kb() {
        let big = "字".repeat(45_000); // ~135KB UTF-8
        let ir = IrRequest {
            model: "doubaowork/deepseek-v4.1-flash".into(),
            incoming_protocol: WireProtocolKind::OpenaiChat,
            system_prompt: Some(big.clone()),
            messages: vec![
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text(big)],
                },
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text("final question".into())],
                },
            ],
            tools: vec![json!({
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "read",
                    "parameters": { "type": "object" }
                }
            })],
            tool_choice: None,
            stream: true,
            temperature: None,
            max_tokens: None,
        };
        let prompt = build_doubao_prompt(&ir);
        assert!(prompt.len() <= MAX_PROMPT_BYTES);
        assert!(prompt.contains("final question"));
        assert!(prompt.contains("read_file"));
    }

    #[test]
    fn parses_doubao_sse_reasoning_text_ack_and_chunk_delta() {
        let raw = concat!(
            "event: SSE_ACK\ndata: {\"ack_client_meta\":{\"conversation_id\":\"9988776655\"}}\n\n",
            "event: STREAM_CHUNK\ndata: {\"message_id\":\"m1\",\"patch_op\":[{\"patch_object\":1,\"patch_type\":2,\"patch_value\":{\"content_block\":[{\"block_id\":\"b_think\",\"block_type\":10040,\"parent_id\":\"\",\"content\":{\"text_block\":{\"text\":\"思考\"}}}]}}]}\n\n",
            "event: CHUNK_DELTA\ndata: {\"text\":\"中\"}\n\n",
            "event: STREAM_CHUNK\ndata: {\"message_id\":\"m1\",\"patch_op\":[{\"patch_object\":1,\"patch_type\":2,\"patch_value\":{\"content_block\":[{\"block_id\":\"b_text\",\"block_type\":10000,\"parent_id\":\"\",\"content\":{\"text_block\":{\"text\":\"你好\"}}}]}}]}\n\n",
            "event: CHUNK_DELTA\ndata: {\"text\":\"世界\"}\n\n",
            "event: SSE_REPLY_END\ndata: {\"end_type\":3}\n\n",
        );
        let parsed = parse_doubao_sse(raw);
        assert!(parsed.error.is_none());
        assert_eq!(parsed.conversation_id.as_deref(), Some("9988776655"));
        assert_eq!(parsed.reasoning, "思考中");
        assert_eq!(parsed.content, "你好世界");
    }

    #[test]
    fn converts_tool_json_reply_into_openai_tool_calls_sse() {
        let parsed = DoubaoParsedStream {
            content: "```json\n{\"content\":\"\",\"tool_calls\":[{\"name\":\"bash\",\"arguments\":{\"cmd\":\"ls\"}}]}\n```".into(),
            reasoning: String::new(),
            conversation_id: None,
            error: None,
        };
        let sse = build_openai_sse_response("doubaowork/deepseek-v4.1-flash", &parsed, true);
        assert!(sse.contains("\"tool_calls\""));
        assert!(sse.contains("\"name\":\"bash\""));
        assert!(sse.contains("\"finish_reason\":\"tool_calls\""));
    }
}
