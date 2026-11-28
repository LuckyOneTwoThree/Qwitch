use serde_json::{json, Value};

use crate::domain::WireProtocolKind;
use crate::plugins::manifest::EgressConstraints;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone)]
pub enum IrPart {
    Text(String),
    Image {
        media_type: String,
        data_base64: Option<String>,
        url: Option<String>,
    },
    Thinking {
        text: String,
        signature: Option<String>,
        sealed_by: Option<WireProtocolKind>,
    },
    ToolCall {
        id: String,
        name: String,
        args_json: String,
    },
    ToolResult {
        call_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone)]
pub struct IrMessage {
    pub role: IrRole,
    pub parts: Vec<IrPart>,
}

#[derive(Debug, Clone)]
pub struct IrRequest {
    pub incoming_protocol: WireProtocolKind,
    pub model: String,
    pub stream: bool,
    pub system_prompt: Option<String>,
    pub messages: Vec<IrMessage>,
    pub tools: Vec<Value>,
    /// 原始 `tool_choice`（协议相关）；跨协议时由上游构造器翻译。
    pub tool_choice: Option<Value>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
}

impl IrRequest {
    pub fn has_images(&self) -> bool {
        self.messages
            .iter()
            .any(|m| m.parts.iter().any(|p| matches!(p, IrPart::Image { .. })))
    }
}

#[derive(Debug, Clone, Default)]
pub struct IrUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct IrResponse {
    pub id: String,
    pub model: String,
    pub text: String,
    pub thinking: Option<String>,
    pub tool_calls: Vec<(String, String, String)>,
    pub finish_reason: String,
    pub usage: IrUsage,
}

pub fn normalize_tool_call_id(id: &str) -> String {
    if id.len() <= 40 {
        return id.to_string();
    }
    let mut hash: u64 = 14695981039346656037;
    for b in id.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("call_{hash:016x}")
}

/// 将任意来源的结束原因映射为 Anthropic `stop_reason`。
fn map_finish_to_anthropic_stop(finish: &str, has_tools: bool) -> &'static str {
    if has_tools {
        return "tool_use";
    }
    match finish {
        "tool_calls" | "tool_use" => "tool_use",
        "length" | "max_tokens" => "max_tokens",
        _ => "end_turn",
    }
}

/// 将任意来源的结束原因映射为 OpenAI `finish_reason`。
fn map_stop_to_openai_finish(stop: &str) -> &'static str {
    match stop {
        "tool_calls" | "tool_use" => "tool_calls",
        "length" | "max_tokens" => "length",
        _ => "stop",
    }
}

pub fn parse_openai_chat_request(body: &Value) -> Result<IrRequest, String> {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "缺少 model 字段".to_string())?
        .to_string();
    let stream = body
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_tokens = body
        .get("max_tokens")
        .or_else(|| body.get("max_completion_tokens"))
        .and_then(|v| v.as_u64());
    let temperature = body.get("temperature").and_then(|v| v.as_f64());
    let tools = body
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let tool_choice = body.get("tool_choice").cloned();

    let mut system_parts = Vec::new();
    let mut messages = Vec::new();
    if let Some(arr) = body.get("messages").and_then(|v| v.as_array()) {
        for msg in arr {
            let role_str = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let text = extract_text_from_value(msg.get("content"));
            match role_str {
                "system" | "developer" => {
                    if !text.is_empty() {
                        system_parts.push(text);
                    }
                }
                "tool" => {
                    let call_id = normalize_tool_call_id(
                        msg.get("tool_call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("call_0"),
                    );
                    messages.push(IrMessage {
                        role: IrRole::Tool,
                        parts: vec![IrPart::ToolResult {
                            call_id,
                            content: text,
                            is_error: false,
                        }],
                    });
                }
                "assistant" => {
                    let mut parts = Vec::new();
                    if !text.is_empty() {
                        parts.push(IrPart::Text(text));
                    }
                    if let Some(tcs) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                        for tc in tcs {
                            let id = normalize_tool_call_id(
                                tc.get("id").and_then(|v| v.as_str()).unwrap_or("call_0"),
                            );
                            let name = tc
                                .pointer("/function/name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("tool")
                                .to_string();
                            let args_json = tc
                                .pointer("/function/arguments")
                                .and_then(|v| v.as_str())
                                .unwrap_or("{}")
                                .to_string();
                            parts.push(IrPart::ToolCall {
                                id,
                                name,
                                args_json,
                            });
                        }
                    }
                    messages.push(IrMessage {
                        role: IrRole::Assistant,
                        parts,
                    });
                }
                _ => {
                    let mut parts = Vec::new();
                    if let Some(Value::Array(blocks)) = msg.get("content") {
                        for b in blocks {
                            match b.get("type").and_then(|v| v.as_str()) {
                                Some("text") => {
                                    if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                                        if !t.is_empty() {
                                            parts.push(IrPart::Text(t.to_string()));
                                        }
                                    }
                                }
                                Some("image_url") => {
                                    if let Some(img) = parse_image_url_part(b.get("image_url")) {
                                        parts.push(img);
                                    }
                                }
                                _ => {}
                            }
                        }
                    } else {
                        parts.push(IrPart::Text(text));
                    }
                    messages.push(IrMessage {
                        role: IrRole::User,
                        parts,
                    });
                }
            }
        }
    }

    Ok(IrRequest {
        incoming_protocol: WireProtocolKind::OpenaiChat,
        model,
        stream,
        system_prompt: (!system_parts.is_empty()).then(|| system_parts.join("\n\n")),
        messages,
        tools,
        tool_choice,
        max_tokens,
        temperature,
    })
}

pub fn parse_anthropic_messages_request(body: &Value) -> Result<IrRequest, String> {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "缺少 model 字段".to_string())?
        .to_string();
    let stream = body
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_tokens = body.get("max_tokens").and_then(|v| v.as_u64());
    let temperature = body.get("temperature").and_then(|v| v.as_f64());
    let system_prompt = body.get("system").map(|s| extract_text_from_value(Some(s)));
    let tools = body
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let tool_choice = body.get("tool_choice").cloned();

    let mut messages = Vec::new();
    if let Some(arr) = body.get("messages").and_then(|v| v.as_array()) {
        for msg in arr {
            let role = match msg.get("role").and_then(|v| v.as_str()).unwrap_or("user") {
                "assistant" => IrRole::Assistant,
                _ => IrRole::User,
            };
            let mut parts = Vec::new();
            match msg.get("content") {
                Some(Value::String(s)) => parts.push(IrPart::Text(s.clone())),
                Some(Value::Array(blocks)) => {
                    for b in blocks {
                        let btype = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        match btype {
                            "text" => {
                                if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                                    parts.push(IrPart::Text(t.to_string()));
                                }
                            }
                            "image" => {
                                if let Some(src) = b.get("source") {
                                    let stype =
                                        src.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                    let media_type = src
                                        .get("media_type")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("image/png")
                                        .to_string();
                                    if stype == "base64" {
                                        if let Some(data) = src.get("data").and_then(|v| v.as_str())
                                        {
                                            parts.push(IrPart::Image {
                                                media_type,
                                                data_base64: Some(data.to_string()),
                                                url: None,
                                            });
                                        }
                                    } else if stype == "url" {
                                        if let Some(url) = src.get("url").and_then(|v| v.as_str()) {
                                            parts.push(IrPart::Image {
                                                media_type,
                                                data_base64: None,
                                                url: Some(url.to_string()),
                                            });
                                        }
                                    }
                                }
                            }
                            "thinking" => {
                                let text = b
                                    .get("thinking")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let sig = b
                                    .get("signature")
                                    .and_then(|v| v.as_str())
                                    .map(String::from);
                                parts.push(IrPart::Thinking {
                                    text,
                                    signature: sig,
                                    sealed_by: Some(WireProtocolKind::AnthropicMessages),
                                });
                            }
                            "tool_use" => {
                                let id = normalize_tool_call_id(
                                    b.get("id").and_then(|v| v.as_str()).unwrap_or("call_0"),
                                );
                                let name = b
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("tool")
                                    .to_string();
                                let args_json = b
                                    .get("input")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "{}".to_string());
                                parts.push(IrPart::ToolCall {
                                    id,
                                    name,
                                    args_json,
                                });
                            }
                            "tool_result" => {
                                let call_id = normalize_tool_call_id(
                                    b.get("tool_use_id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("call_0"),
                                );
                                let content = extract_text_from_value(b.get("content"));
                                let is_error =
                                    b.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                                parts.push(IrPart::ToolResult {
                                    call_id,
                                    content,
                                    is_error,
                                });
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            messages.push(IrMessage { role, parts });
        }
    }

    Ok(IrRequest {
        incoming_protocol: WireProtocolKind::AnthropicMessages,
        model,
        stream,
        system_prompt: system_prompt.filter(|s| !s.is_empty()),
        messages,
        tools,
        tool_choice,
        max_tokens,
        temperature,
    })
}

pub fn parse_openai_responses_request(body: &Value) -> Result<IrRequest, String> {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "缺少 model 字段".to_string())?
        .to_string();
    let stream = body
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_tokens = body.get("max_output_tokens").and_then(|v| v.as_u64());
    let temperature = body.get("temperature").and_then(|v| v.as_f64());
    let mut system_prompt = body
        .get("instructions")
        .and_then(|v| v.as_str())
        .map(String::from)
        .filter(|s| !s.is_empty());
    let tools = body
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let tool_choice = body.get("tool_choice").cloned();

    let mut messages = Vec::new();
    match body.get("input") {
        Some(Value::String(s)) => {
            messages.push(IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text(s.clone())],
            });
        }
        Some(Value::Array(items)) => {
            for item in items {
                let itype = item
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("message");
                match itype {
                    "function_call" => {
                        let call_id = normalize_tool_call_id(
                            item.get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("call_0"),
                        );
                        let name = item
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("tool")
                            .to_string();
                        let args_json = item
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .unwrap_or("{}")
                            .to_string();
                        let part = IrPart::ToolCall {
                            id: call_id,
                            name,
                            args_json,
                        };
                        if let Some(last) =
                            messages.last_mut().filter(|m| m.role == IrRole::Assistant)
                        {
                            last.parts.push(part);
                        } else {
                            messages.push(IrMessage {
                                role: IrRole::Assistant,
                                parts: vec![part],
                            });
                        }
                    }
                    "function_call_output" => {
                        let call_id = normalize_tool_call_id(
                            item.get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("call_0"),
                        );
                        let content = extract_text_from_value(item.get("output"));
                        messages.push(IrMessage {
                            role: IrRole::Tool,
                            parts: vec![IrPart::ToolResult {
                                call_id,
                                content,
                                is_error: false,
                            }],
                        });
                    }
                    _ => {
                        let role_str = item.get("role").and_then(|v| v.as_str()).unwrap_or("user");
                        let text = extract_text_from_value(item.get("content"));
                        if !text.is_empty() {
                            match role_str {
                                "system" | "developer" => {
                                    system_prompt = Some(match system_prompt {
                                        Some(existing) if !existing.is_empty() => {
                                            format!("{existing}\n\n{text}")
                                        }
                                        _ => text,
                                    });
                                }
                                "assistant" => {
                                    let part = IrPart::Text(text);
                                    if let Some(last) =
                                        messages.last_mut().filter(|m| m.role == IrRole::Assistant)
                                    {
                                        last.parts.push(part);
                                    } else {
                                        messages.push(IrMessage {
                                            role: IrRole::Assistant,
                                            parts: vec![part],
                                        });
                                    }
                                }
                                _ => {
                                    let mut parts = Vec::new();
                                    if let Some(Value::Array(blocks)) = item.get("content") {
                                        for b in blocks {
                                            match b.get("type").and_then(|v| v.as_str()) {
                                                Some("input_text") | Some("text") => {
                                                    if let Some(t) =
                                                        b.get("text").and_then(|v| v.as_str())
                                                    {
                                                        if !t.is_empty() {
                                                            parts.push(IrPart::Text(t.to_string()));
                                                        }
                                                    }
                                                }
                                                Some("input_image") => {
                                                    if let Some(img) = parse_image_url_part(
                                                        b.get("image_url").or(Some(b)),
                                                    ) {
                                                        parts.push(img);
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    if parts.is_empty() {
                                        parts.push(IrPart::Text(text));
                                    }
                                    messages.push(IrMessage {
                                        role: IrRole::User,
                                        parts,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }

    Ok(IrRequest {
        incoming_protocol: WireProtocolKind::OpenaiResponses,
        model,
        stream,
        system_prompt,
        messages,
        tools,
        tool_choice,
        max_tokens,
        temperature,
    })
}

fn parse_image_url_part(val: Option<&Value>) -> Option<IrPart> {
    let v = val?;
    let raw_url = match v {
        Value::String(s) => s.as_str(),
        Value::Object(obj) => obj
            .get("url")
            .or_else(|| obj.get("image_url"))
            .and_then(Value::as_str)?,
        _ => return None,
    };
    let trimmed = raw_url.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("data:") {
        if let Some((meta, b64)) = rest.split_once(";base64,") {
            let media_type = if meta.is_empty() {
                "image/png".to_string()
            } else {
                meta.to_string()
            };
            return Some(IrPart::Image {
                media_type,
                data_base64: Some(b64.to_string()),
                url: None,
            });
        }
    }
    Some(IrPart::Image {
        media_type: "image/png".to_string(),
        data_base64: None,
        url: Some(trimmed.to_string()),
    })
}

fn extract_text_from_value(val: Option<&Value>) -> String {
    let Some(v) = val else {
        return String::new();
    };
    match v {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .or_else(|| item.get("content"))
                    .and_then(|t| t.as_str())
                    .map(String::from)
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// 将任意入站协议的工具声明归一化为 `(name, description, parameters)` 三元组。
///
/// 支持三种入站形态：
/// - OpenAI Chat：`{ type:"function", function:{name,description,parameters} }`
/// - OpenAI Responses：`{ type:"function", name, description, parameters }`
/// - Anthropic Messages：`{ name, description, input_schema }`
fn canonical_tools(tools: &[Value]) -> Vec<(String, String, Value)> {
    let mut out = Vec::new();
    for t in tools {
        let func = t.get("function").filter(|v| v.is_object());
        let name = func
            .and_then(|f| f.get("name"))
            .or_else(|| t.get("name"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let Some(name) = name else { continue };
        let description = func
            .and_then(|f| f.get("description"))
            .or_else(|| t.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let parameters = func
            .and_then(|f| f.get("parameters"))
            .or_else(|| t.get("parameters"))
            .or_else(|| t.get("input_schema"))
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
        out.push((name, description, parameters));
    }
    out
}

fn tools_to_openai_chat(tools: &[Value]) -> Vec<Value> {
    canonical_tools(tools)
        .into_iter()
        .map(|(name, description, parameters)| {
            json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": description,
                    "parameters": parameters
                }
            })
        })
        .collect()
}

fn tools_to_anthropic(tools: &[Value]) -> Vec<Value> {
    canonical_tools(tools)
        .into_iter()
        .map(|(name, description, parameters)| {
            json!({
                "name": name,
                "description": description,
                "input_schema": parameters
            })
        })
        .collect()
}

fn tool_choice_to_openai(choice: &Value) -> Option<Value> {
    match choice.get("type").and_then(|v| v.as_str()) {
        Some("auto") => Some(json!("auto")),
        Some("any") => Some(json!("required")),
        Some("none") => Some(json!("none")),
        Some("tool") => choice
            .get("name")
            .and_then(|v| v.as_str())
            .map(|n| json!({ "type": "function", "function": { "name": n } })),
        Some("function") if choice.get("function").is_none() => choice
            .get("name")
            .and_then(|v| v.as_str())
            .map(|n| json!({ "type": "function", "function": { "name": n } })),
        // 已是 OpenAI 形态（"auto"/"none"/{type:"function",...}）或未知 -> 原样透传
        _ => Some(choice.clone()),
    }
}

fn tool_choice_to_anthropic(choice: &Value) -> Option<Value> {
    if let Some(s) = choice.as_str() {
        return match s {
            "auto" => Some(json!({ "type": "auto" })),
            "required" => Some(json!({ "type": "any" })),
            _ => None, // "none" 等 Anthropic 无对应，省略
        };
    }
    if let Some(name) = choice
        .get("function")
        .and_then(|f| f.get("name"))
        .and_then(|v| v.as_str())
    {
        return Some(json!({ "type": "tool", "name": name }));
    }
    if choice.get("type").and_then(|v| v.as_str()) == Some("function") {
        if let Some(name) = choice.get("name").and_then(|v| v.as_str()) {
            return Some(json!({ "type": "tool", "name": name }));
        }
    }
    // 已是 Anthropic 形态
    choice.get("type").map(|_| choice.clone())
}

pub fn to_upstream_openai_chat_body(ir: &IrRequest, upstream_model: &str, stream: bool) -> Value {
    let mut msgs = Vec::new();
    if let Some(sys) = &ir.system_prompt {
        msgs.push(json!({ "role": "system", "content": sys }));
    }
    for msg in &ir.messages {
        match msg.role {
            IrRole::User => {
                // OpenAI 要求工具结果以独立 `role:"tool"` 消息紧跟 assistant tool_calls，
                // 且必须保留 `tool_call_id`；因此先输出工具结果，再输出普通用户文本。
                for p in &msg.parts {
                    if let IrPart::ToolResult {
                        call_id, content, ..
                    } = p
                    {
                        msgs.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": content
                        }));
                    }
                }
                let text: String = msg
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        IrPart::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let images: Vec<Value> = msg
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        IrPart::Image {
                            media_type,
                            data_base64,
                            url,
                        } => {
                            let target_url = if let Some(b64) = data_base64 {
                                format!("data:{media_type};base64,{b64}")
                            } else if let Some(u) = url {
                                u.clone()
                            } else {
                                return None;
                            };
                            Some(json!({
                                "type": "image_url",
                                "image_url": { "url": target_url }
                            }))
                        }
                        _ => None,
                    })
                    .collect();
                if !images.is_empty() {
                    let mut content_arr = Vec::new();
                    if !text.is_empty() {
                        content_arr.push(json!({ "type": "text", "text": text }));
                    }
                    content_arr.extend(images);
                    msgs.push(json!({ "role": "user", "content": content_arr }));
                } else if !text.is_empty() {
                    msgs.push(json!({ "role": "user", "content": text }));
                }
            }
            IrRole::Tool => {
                for p in &msg.parts {
                    if let IrPart::ToolResult {
                        call_id, content, ..
                    } = p
                    {
                        msgs.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": content
                        }));
                    }
                }
            }
            IrRole::Assistant => {
                let text: String = msg
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        IrPart::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let tool_calls: Vec<Value> = msg
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        IrPart::ToolCall {
                            id,
                            name,
                            args_json,
                        } => Some(json!({
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": args_json }
                        })),
                        _ => None,
                    })
                    .collect();
                let mut obj = json!({
                    "role": "assistant",
                    "content": if text.is_empty() && !tool_calls.is_empty() { Value::Null } else { Value::String(text) }
                });
                if !tool_calls.is_empty() {
                    obj.as_object_mut()
                        .unwrap()
                        .insert("tool_calls".into(), Value::Array(tool_calls));
                }
                msgs.push(obj);
            }
            IrRole::System => {}
        }
    }

    let mut body = json!({
        "model": upstream_model,
        "messages": msgs,
        "stream": stream
    });
    if let Some(mt) = ir.max_tokens {
        body["max_tokens"] = json!(mt);
    }
    if let Some(temp) = ir.temperature {
        body["temperature"] = json!(temp);
    }
    let tools = tools_to_openai_chat(&ir.tools);
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
        if let Some(tc) = ir.tool_choice.as_ref().and_then(tool_choice_to_openai) {
            body["tool_choice"] = tc;
        }
    }
    body
}

pub fn to_upstream_anthropic_body(ir: &IrRequest, upstream_model: &str, stream: bool) -> Value {
    let mut msgs: Vec<Value> = Vec::new();
    for msg in &ir.messages {
        let role = match msg.role {
            IrRole::Assistant => "assistant",
            _ => "user",
        };
        let mut blocks = Vec::new();
        for p in &msg.parts {
            match p {
                IrPart::Text(t) if !t.is_empty() => {
                    blocks.push(json!({ "type": "text", "text": t }));
                }
                IrPart::Image {
                    media_type,
                    data_base64,
                    url,
                } => {
                    if let Some(b64) = data_base64 {
                        blocks.push(json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": media_type,
                                "data": b64
                            }
                        }));
                    } else if let Some(u) = url {
                        blocks.push(json!({
                            "type": "image",
                            "source": {
                                "type": "url",
                                "url": u
                            }
                        }));
                    }
                }
                IrPart::Thinking {
                    text,
                    signature,
                    sealed_by,
                } => {
                    // 推理签名隔离：仅同源 Anthropic 协议原样透传 signature，跨源剥离防 400 invalid_encrypted_content
                    if *sealed_by == Some(WireProtocolKind::AnthropicMessages) {
                        if let Some(sig) = signature {
                            blocks.push(json!({
                                "type": "thinking",
                                "thinking": text,
                                "signature": sig
                            }));
                            continue;
                        }
                    }
                    if !text.is_empty() {
                        blocks.push(json!({ "type": "text", "text": format!("<thinking>{text}</thinking>") }));
                    }
                }
                IrPart::ToolCall {
                    id,
                    name,
                    args_json,
                } => {
                    let input: Value =
                        serde_json::from_str(args_json).unwrap_or_else(|_| json!({}));
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": input
                    }));
                }
                IrPart::ToolResult {
                    call_id,
                    content,
                    is_error,
                } => {
                    blocks.push(json!({
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": content,
                        "is_error": is_error
                    }));
                }
                _ => {}
            }
        }
        if !blocks.is_empty() {
            let merged = msgs.last_mut().and_then(|last| {
                if last.get("role").and_then(Value::as_str) == Some(role) {
                    last.get_mut("content").and_then(Value::as_array_mut)
                } else {
                    None
                }
            });
            if let Some(arr) = merged {
                arr.extend(blocks);
            } else {
                msgs.push(json!({ "role": role, "content": blocks }));
            }
        }
    }

    let mut body = json!({
        "model": upstream_model,
        "max_tokens": ir.max_tokens.unwrap_or(4096),
        "messages": msgs,
        "stream": stream
    });
    if let Some(sys) = &ir.system_prompt {
        body["system"] = json!(sys);
    }
    let tools = tools_to_anthropic(&ir.tools);
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
        if let Some(tc) = ir.tool_choice.as_ref().and_then(tool_choice_to_anthropic) {
            body["tool_choice"] = tc;
        }
    }
    body
}

fn tools_to_openai_responses(tools: &[Value]) -> Vec<Value> {
    canonical_tools(tools)
        .into_iter()
        .map(|(name, description, parameters)| {
            json!({
                "type": "function",
                "name": name,
                "description": description,
                "parameters": parameters
            })
        })
        .collect()
}

fn tool_choice_to_openai_responses(choice: &Value) -> Option<Value> {
    if let Some(s) = choice.as_str() {
        return match s {
            "auto" | "required" | "none" => Some(json!(s)),
            _ => None,
        };
    }
    match choice.get("type").and_then(|v| v.as_str()) {
        Some("auto") => Some(json!("auto")),
        Some("any") | Some("required") => Some(json!("required")),
        Some("none") => Some(json!("none")),
        Some("tool") => choice
            .get("name")
            .and_then(|v| v.as_str())
            .map(|n| json!({ "type": "function", "name": n })),
        Some("function") => choice
            .get("function")
            .and_then(|f| f.get("name"))
            .or_else(|| choice.get("name"))
            .and_then(|v| v.as_str())
            .map(|n| json!({ "type": "function", "name": n })),
        _ => Some(choice.clone()),
    }
}

pub fn apply_egress_constraints(
    body: &mut Value,
    protocol: WireProtocolKind,
    constraints: &EgressConstraints,
) {
    if constraints.must_stream {
        body["stream"] = json!(true);
    }
    if protocol == WireProtocolKind::OpenaiResponses {
        if constraints.disallow_store {
            body["store"] = json!(false);
        }
        if constraints.requires_instructions
            && body
                .get("instructions")
                .and_then(Value::as_str)
                .is_none_or(|s| s.trim().is_empty())
        {
            body["instructions"] = json!("You are a helpful coding assistant.");
        }
    }
    if constraints.strip_temperature_on_reasoning {
        if let Some(obj) = body.as_object_mut() {
            obj.remove("temperature");
        }
    }
}

pub fn to_upstream_openai_responses_body(
    ir: &IrRequest,
    upstream_model: &str,
    stream: bool,
    constraints: Option<&EgressConstraints>,
) -> Value {
    let mut input: Vec<Value> = Vec::new();
    for msg in &ir.messages {
        match msg.role {
            IrRole::User => {
                for p in &msg.parts {
                    if let IrPart::ToolResult {
                        call_id, content, ..
                    } = p
                    {
                        input.push(json!({
                            "type": "function_call_output",
                            "call_id": call_id,
                            "output": content
                        }));
                    }
                }
                let mut content_blocks: Vec<Value> = Vec::new();
                for p in &msg.parts {
                    match p {
                        IrPart::Text(t) if !t.is_empty() => {
                            content_blocks.push(json!({ "type": "input_text", "text": t }));
                        }
                        IrPart::Image {
                            media_type,
                            data_base64,
                            url,
                        } => {
                            let target_url = if let Some(b64) = data_base64 {
                                Some(format!("data:{media_type};base64,{b64}"))
                            } else {
                                url.clone()
                            };
                            if let Some(u) = target_url {
                                content_blocks.push(json!({
                                    "type": "input_image",
                                    "image_url": u
                                }));
                            }
                        }
                        _ => {}
                    }
                }
                if !content_blocks.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "user",
                        "content": content_blocks
                    }));
                }
            }
            IrRole::Tool => {
                for p in &msg.parts {
                    if let IrPart::ToolResult {
                        call_id, content, ..
                    } = p
                    {
                        input.push(json!({
                            "type": "function_call_output",
                            "call_id": call_id,
                            "output": content
                        }));
                    }
                }
            }
            IrRole::Assistant => {
                let mut text_blocks: Vec<Value> = Vec::new();
                let mut fc_items: Vec<Value> = Vec::new();
                for p in &msg.parts {
                    match p {
                        IrPart::Text(t) if !t.is_empty() => {
                            text_blocks.push(json!({ "type": "output_text", "text": t }));
                        }
                        IrPart::ToolCall {
                            id,
                            name,
                            args_json,
                        } => {
                            fc_items.push(json!({
                                "type": "function_call",
                                "call_id": id,
                                "name": name,
                                "arguments": args_json
                            }));
                        }
                        _ => {}
                    }
                }
                if !text_blocks.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": text_blocks
                    }));
                }
                input.extend(fc_items);
            }
            IrRole::System => {}
        }
    }

    let mut body = json!({
        "model": upstream_model,
        "input": input,
        "stream": stream
    });
    if let Some(sys) = &ir.system_prompt {
        if !sys.trim().is_empty() {
            body["instructions"] = json!(sys);
        }
    }
    if let Some(mt) = ir.max_tokens {
        body["max_output_tokens"] = json!(mt);
    }
    if let Some(temp) = ir.temperature {
        body["temperature"] = json!(temp);
    }
    let tools = tools_to_openai_responses(&ir.tools);
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
        if let Some(tc) = ir
            .tool_choice
            .as_ref()
            .and_then(tool_choice_to_openai_responses)
        {
            body["tool_choice"] = tc;
        }
    }
    if let Some(c) = constraints {
        apply_egress_constraints(&mut body, WireProtocolKind::OpenaiResponses, c);
    }
    body
}

pub fn parse_openai_responses_json(parsed: &Value, model: &str) -> Option<IrResponse> {
    let is_responses = parsed.get("object").and_then(Value::as_str) == Some("response")
        || parsed.get("output").and_then(Value::as_array).is_some();
    if !is_responses {
        return None;
    }

    let mut text = String::new();
    let mut thinking = String::new();
    let mut tool_calls: Vec<(String, String, String)> = Vec::new();

    if let Some(output) = parsed.get("output").and_then(Value::as_array) {
        for item in output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    if let Some(content) = item.get("content").and_then(Value::as_array) {
                        for block in content {
                            if matches!(
                                block.get("type").and_then(Value::as_str),
                                Some("output_text") | Some("text")
                            ) {
                                if let Some(s) = block.get("text").and_then(Value::as_str) {
                                    text.push_str(s);
                                }
                            }
                        }
                    }
                }
                Some("reasoning") => {
                    if let Some(summary) = item.get("summary").and_then(Value::as_array) {
                        for s in summary {
                            if let Some(t) = s.get("text").and_then(Value::as_str) {
                                thinking.push_str(t);
                            }
                        }
                    }
                }
                Some("function_call") => {
                    if let Some(name) = item.get("name").and_then(Value::as_str) {
                        let call_id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or("call_0")
                            .to_string();
                        let args = item
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}")
                            .to_string();
                        tool_calls.push((call_id, name.to_string(), args));
                    }
                }
                _ => {}
            }
        }
    }

    let status = parsed
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed");
    let finish_reason = if !tool_calls.is_empty() {
        "tool_calls".to_string()
    } else if status == "incomplete" {
        "max_tokens".to_string()
    } else {
        "stop".to_string()
    };

    let usage = IrUsage {
        input_tokens: parsed
            .pointer("/usage/input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: parsed
            .pointer("/usage/output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    };

    Some(IrResponse {
        id: parsed
            .get("id")
            .and_then(Value::as_str)
            .map(|s| s.trim_start_matches("resp_").to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string()[..12].to_string()),
        model: model.to_string(),
        text,
        thinking: if thinking.is_empty() {
            None
        } else {
            Some(thinking)
        },
        tool_calls,
        finish_reason,
        usage,
    })
}

pub fn parse_openai_responses_sse(raw: &str, model: &str) -> Option<IrResponse> {
    if !raw.contains("response.output_text")
        && !raw.contains("response.completed")
        && !raw.contains("response.output_item.")
        && !raw.contains("response.created")
    {
        return None;
    }

    let mut text = String::new();
    let mut thinking = String::new();
    let mut tool_calls_map: std::collections::BTreeMap<usize, (String, String, String)> =
        std::collections::BTreeMap::new();
    let mut completed_resp: Option<IrResponse> = None;

    for line in raw.lines() {
        let trimmed = line.trim();
        let Some(raw_data) = trimmed.strip_prefix("data:") else {
            continue;
        };
        let data = raw_data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match ev.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(d) = ev.get("delta").and_then(Value::as_str) {
                    text.push_str(d);
                }
            }
            Some("response.reasoning_summary_text.delta") => {
                if let Some(d) = ev.get("delta").and_then(Value::as_str) {
                    thinking.push_str(d);
                }
            }
            Some("response.output_item.added") => {
                let idx = ev.get("output_index").and_then(Value::as_u64).unwrap_or(0) as usize;
                if ev.pointer("/item/type").and_then(Value::as_str) == Some("function_call") {
                    let call_id = ev
                        .pointer("/item/call_id")
                        .or_else(|| ev.pointer("/item/id"))
                        .and_then(Value::as_str)
                        .unwrap_or("call_0")
                        .to_string();
                    let name = ev
                        .pointer("/item/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    tool_calls_map.insert(idx, (call_id, name, String::new()));
                }
            }
            Some("response.function_call_arguments.delta") => {
                let idx = ev.get("output_index").and_then(Value::as_u64).unwrap_or(0) as usize;
                if let Some(d) = ev.get("delta").and_then(Value::as_str) {
                    let entry = tool_calls_map
                        .entry(idx)
                        .or_insert_with(|| (format!("call_{idx}"), String::new(), String::new()));
                    entry.2.push_str(d);
                }
            }
            Some("response.output_item.done") => {
                let idx = ev.get("output_index").and_then(Value::as_u64).unwrap_or(0) as usize;
                if ev.pointer("/item/type").and_then(Value::as_str) == Some("function_call") {
                    let call_id = ev
                        .pointer("/item/call_id")
                        .or_else(|| ev.pointer("/item/id"))
                        .and_then(Value::as_str)
                        .unwrap_or("call_0")
                        .to_string();
                    let name = ev
                        .pointer("/item/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let args = ev
                        .pointer("/item/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}")
                        .to_string();
                    tool_calls_map.insert(idx, (call_id, name, args));
                }
            }
            Some("response.completed") => {
                if let Some(resp_obj) = ev.get("response") {
                    completed_resp = parse_openai_responses_json(resp_obj, model);
                }
            }
            _ => {}
        }
    }

    if let Some(mut comp) = completed_resp {
        if comp.text.is_empty() && !text.is_empty() {
            comp.text = text;
        }
        if comp.thinking.is_none() && !thinking.is_empty() {
            comp.thinking = Some(thinking);
        }
        if comp.tool_calls.is_empty() && !tool_calls_map.is_empty() {
            comp.tool_calls = tool_calls_map
                .into_values()
                .filter(|(_, n, _)| !n.is_empty())
                .collect();
            if !comp.tool_calls.is_empty() {
                comp.finish_reason = "tool_calls".to_string();
            }
        }
        return Some(comp);
    }

    let tool_calls: Vec<(String, String, String)> = tool_calls_map
        .into_values()
        .filter(|(_, n, _)| !n.is_empty())
        .collect();
    let finish_reason = if tool_calls.is_empty() {
        "stop".to_string()
    } else {
        "tool_calls".to_string()
    };

    Some(IrResponse {
        id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
        model: model.to_string(),
        text,
        thinking: if thinking.is_empty() {
            None
        } else {
            Some(thinking)
        },
        tool_calls,
        finish_reason,
        usage: IrUsage::default(),
    })
}

pub fn to_openai_chat_response(resp: &IrResponse) -> Value {
    let has_tools = !resp.tool_calls.is_empty();
    let content = if resp.text.is_empty() && has_tools {
        Value::Null
    } else {
        json!(resp.text)
    };
    let mut message = serde_json::Map::new();
    message.insert("role".into(), json!("assistant"));
    message.insert("content".into(), content);
    if let Some(thinking) = &resp.thinking {
        if !thinking.is_empty() {
            message.insert("reasoning_content".into(), json!(thinking));
        }
    }
    if has_tools {
        let tc_arr: Vec<Value> = resp
            .tool_calls
            .iter()
            .map(|(id, name, args)| {
                json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": args
                    }
                })
            })
            .collect();
        message.insert("tool_calls".into(), Value::Array(tc_arr));
    }
    let finish_reason = if has_tools {
        "tool_calls"
    } else {
        map_stop_to_openai_finish(&resp.finish_reason)
    };
    json!({
        "id": format!("chatcmpl-{}", resp.id),
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": resp.model,
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": finish_reason
        }],
        "usage": {
            "prompt_tokens": resp.usage.input_tokens,
            "completion_tokens": resp.usage.output_tokens,
            "total_tokens": resp.usage.input_tokens + resp.usage.output_tokens
        }
    })
}

pub fn to_anthropic_messages_response(resp: &IrResponse) -> Value {
    let mut content = Vec::new();
    if let Some(thinking) = &resp.thinking {
        if !thinking.is_empty() {
            content.push(json!({ "type": "thinking", "thinking": thinking }));
        }
    }
    if !resp.text.is_empty() || (resp.tool_calls.is_empty() && content.is_empty()) {
        content.push(json!({ "type": "text", "text": resp.text }));
    }
    for (id, name, args) in &resp.tool_calls {
        let input: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
        content.push(json!({ "type": "tool_use", "id": id, "name": name, "input": input }));
    }
    let stop_reason =
        map_finish_to_anthropic_stop(&resp.finish_reason, !resp.tool_calls.is_empty());
    json!({
        "id": format!("msg_{}", resp.id),
        "type": "message",
        "role": "assistant",
        "model": resp.model,
        "content": content,
        "stop_reason": stop_reason,
        "usage": {
            "input_tokens": resp.usage.input_tokens,
            "output_tokens": resp.usage.output_tokens
        }
    })
}

pub fn to_openai_responses_response(resp: &IrResponse) -> Value {
    let mut output = Vec::new();
    if !resp.text.is_empty() || resp.tool_calls.is_empty() {
        output.push(json!({
            "id": format!("msg_{}", resp.id),
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [{
                "type": "output_text",
                "text": resp.text,
                "annotations": []
            }]
        }));
    }
    for (idx, (call_id, name, args)) in resp.tool_calls.iter().enumerate() {
        output.push(json!({
            "id": format!("fc_{idx}_{}", resp.id),
            "type": "function_call",
            "status": "completed",
            "call_id": call_id,
            "name": name,
            "arguments": args
        }));
    }
    json!({
        "id": format!("resp_{}", resp.id),
        "object": "response",
        "status": "completed",
        "model": resp.model,
        "output": output,
        "usage": {
            "input_tokens": resp.usage.input_tokens,
            "output_tokens": resp.usage.output_tokens,
            "total_tokens": resp.usage.input_tokens + resp.usage.output_tokens
        }
    })
}

pub fn to_openai_chat_sse(resp: &IrResponse) -> String {
    let id = format!("chatcmpl-{}", resp.id);
    let mut out = String::new();
    let mut sent_role = false;

    if let Some(thinking) = &resp.thinking {
        if !thinking.is_empty() {
            let chunk = json!({
                "id": &id,
                "object": "chat.completion.chunk",
                "model": resp.model,
                "choices": [{
                    "index": 0,
                    "delta": { "role": "assistant", "reasoning_content": thinking },
                    "finish_reason": Value::Null
                }]
            });
            out.push_str(&format!("data: {chunk}\n\n"));
            sent_role = true;
        }
    }

    if !resp.text.is_empty() || resp.tool_calls.is_empty() {
        let mut delta = serde_json::Map::new();
        if !sent_role {
            delta.insert("role".into(), json!("assistant"));
            sent_role = true;
        }
        delta.insert("content".into(), json!(resp.text));
        let chunk = json!({
            "id": &id,
            "object": "chat.completion.chunk",
            "model": resp.model,
            "choices": [{
                "index": 0,
                "delta": Value::Object(delta),
                "finish_reason": Value::Null
            }]
        });
        out.push_str(&format!("data: {chunk}\n\n"));
    }

    if !resp.tool_calls.is_empty() {
        let tc_arr: Vec<Value> = resp
            .tool_calls
            .iter()
            .enumerate()
            .map(|(idx, (tc_id, name, args))| {
                json!({
                    "index": idx,
                    "id": tc_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": args
                    }
                })
            })
            .collect();
        let mut delta = serde_json::Map::new();
        if !sent_role {
            delta.insert("role".into(), json!("assistant"));
        }
        delta.insert("tool_calls".into(), Value::Array(tc_arr));
        let chunk = json!({
            "id": &id,
            "object": "chat.completion.chunk",
            "model": resp.model,
            "choices": [{
                "index": 0,
                "delta": Value::Object(delta),
                "finish_reason": Value::Null
            }]
        });
        out.push_str(&format!("data: {chunk}\n\n"));
    }

    let finish_reason = if resp.tool_calls.is_empty() {
        map_stop_to_openai_finish(&resp.finish_reason)
    } else {
        "tool_calls"
    };
    let final_chunk = json!({
        "id": &id,
        "object": "chat.completion.chunk",
        "model": resp.model,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": finish_reason
        }],
        "usage": {
            "prompt_tokens": resp.usage.input_tokens,
            "completion_tokens": resp.usage.output_tokens,
            "total_tokens": resp.usage.input_tokens + resp.usage.output_tokens
        }
    });
    out.push_str(&format!("data: {final_chunk}\n\ndata: [DONE]\n\n"));
    out
}

fn push_named_sse(out: &mut String, event_name: &str, payload: Value) {
    out.push_str("event: ");
    out.push_str(event_name);
    out.push_str("\ndata: ");
    out.push_str(&payload.to_string());
    out.push_str("\n\n");
}

pub fn to_anthropic_messages_sse(resp: &IrResponse) -> String {
    let mut out = String::new();
    let start = json!({
        "type": "message_start",
        "message": {
            "id": format!("msg_{}", resp.id),
            "type": "message",
            "role": "assistant",
            "model": resp.model,
            "content": [],
            "usage": { "input_tokens": resp.usage.input_tokens, "output_tokens": 0 }
        }
    });
    push_named_sse(&mut out, "message_start", start);

    let mut block_idx = 0usize;
    if let Some(thinking) = &resp.thinking {
        if !thinking.is_empty() {
            push_named_sse(
                &mut out,
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": block_idx,
                    "content_block": { "type": "thinking", "thinking": "" }
                }),
            );
            push_named_sse(
                &mut out,
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": block_idx,
                    "delta": { "type": "thinking_delta", "thinking": thinking }
                }),
            );
            push_named_sse(
                &mut out,
                "content_block_stop",
                json!({ "type": "content_block_stop", "index": block_idx }),
            );
            block_idx += 1;
        }
    }

    if !resp.text.is_empty() || (resp.tool_calls.is_empty() && block_idx == 0) {
        push_named_sse(
            &mut out,
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": block_idx,
                "content_block": { "type": "text", "text": "" }
            }),
        );
        if !resp.text.is_empty() {
            push_named_sse(
                &mut out,
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": block_idx,
                    "delta": { "type": "text_delta", "text": resp.text }
                }),
            );
        }
        push_named_sse(
            &mut out,
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": block_idx }),
        );
        block_idx += 1;
    }

    for (id, name, args) in &resp.tool_calls {
        push_named_sse(
            &mut out,
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": block_idx,
                "content_block": { "type": "tool_use", "id": id, "name": name, "input": {} }
            }),
        );
        if !args.is_empty() {
            push_named_sse(
                &mut out,
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": block_idx,
                    "delta": { "type": "input_json_delta", "partial_json": args }
                }),
            );
        }
        push_named_sse(
            &mut out,
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": block_idx }),
        );
        block_idx += 1;
    }

    let stop_reason =
        map_finish_to_anthropic_stop(&resp.finish_reason, !resp.tool_calls.is_empty());
    push_named_sse(
        &mut out,
        "message_delta",
        json!({
            "type": "message_delta",
            "delta": { "stop_reason": stop_reason },
            "usage": { "output_tokens": resp.usage.output_tokens }
        }),
    );
    push_named_sse(&mut out, "message_stop", json!({ "type": "message_stop" }));
    out
}

pub fn to_openai_responses_sse(resp: &IrResponse) -> String {
    let full = to_openai_responses_response(resp);
    let mut out = String::new();
    let in_progress_resp = json!({
        "id": full["id"],
        "object": "response",
        "status": "in_progress",
        "model": resp.model,
        "output": []
    });
    push_named_sse(
        &mut out,
        "response.created",
        json!({
            "type": "response.created",
            "response": in_progress_resp
        }),
    );
    push_named_sse(
        &mut out,
        "response.in_progress",
        json!({
            "type": "response.in_progress",
            "response": in_progress_resp
        }),
    );

    let mut output_index = 0usize;
    if !resp.text.is_empty() || resp.tool_calls.is_empty() {
        let msg_id = format!("msg_{}", resp.id);
        push_named_sse(
            &mut out,
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "output_index": output_index,
                "item": {
                    "id": &msg_id,
                    "type": "message",
                    "status": "in_progress",
                    "role": "assistant",
                    "content": []
                }
            }),
        );
        push_named_sse(
            &mut out,
            "response.content_part.added",
            json!({
                "type": "response.content_part.added",
                "item_id": &msg_id,
                "output_index": output_index,
                "content_index": 0,
                "part": { "type": "output_text", "text": "", "annotations": [] }
            }),
        );
        if !resp.text.is_empty() {
            push_named_sse(
                &mut out,
                "response.output_text.delta",
                json!({
                    "type": "response.output_text.delta",
                    "item_id": &msg_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "delta": resp.text
                }),
            );
        }
        push_named_sse(
            &mut out,
            "response.output_text.done",
            json!({
                "type": "response.output_text.done",
                "item_id": &msg_id,
                "output_index": output_index,
                "content_index": 0,
                "text": resp.text
            }),
        );
        push_named_sse(
            &mut out,
            "response.content_part.done",
            json!({
                "type": "response.content_part.done",
                "item_id": &msg_id,
                "output_index": output_index,
                "content_index": 0,
                "part": { "type": "output_text", "text": resp.text, "annotations": [] }
            }),
        );
        push_named_sse(
            &mut out,
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": {
                    "id": &msg_id,
                    "type": "message",
                    "status": "completed",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": resp.text, "annotations": [] }]
                }
            }),
        );
        output_index += 1;
    }

    for (idx, (call_id, name, args)) in resp.tool_calls.iter().enumerate() {
        let fc_id = format!("fc_{idx}_{}", resp.id);
        push_named_sse(
            &mut out,
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "output_index": output_index,
                "item": {
                    "id": &fc_id,
                    "type": "function_call",
                    "status": "in_progress",
                    "call_id": call_id,
                    "name": name,
                    "arguments": ""
                }
            }),
        );
        if !args.is_empty() {
            push_named_sse(
                &mut out,
                "response.function_call_arguments.delta",
                json!({
                    "type": "response.function_call_arguments.delta",
                    "item_id": &fc_id,
                    "output_index": output_index,
                    "delta": args
                }),
            );
        }
        push_named_sse(
            &mut out,
            "response.function_call_arguments.done",
            json!({
                "type": "response.function_call_arguments.done",
                "item_id": &fc_id,
                "output_index": output_index,
                "arguments": args
            }),
        );
        push_named_sse(
            &mut out,
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": {
                    "id": &fc_id,
                    "type": "function_call",
                    "status": "completed",
                    "call_id": call_id,
                    "name": name,
                    "arguments": args
                }
            }),
        );
        output_index += 1;
    }

    push_named_sse(
        &mut out,
        "response.completed",
        json!({
            "type": "response.completed",
            "response": full
        }),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_three_protocols_and_strips_cross_protocol_thinking_signatures() {
        let anthropic_req = parse_anthropic_messages_request(&json!({
            "model": "claude-sonnet-4.6",
            "system": "You are terse",
            "messages": [{
                "role": "assistant",
                "content": [
                    { "type": "thinking", "thinking": "plan", "signature": "sig_secret" },
                    { "type": "text", "text": "hello" }
                ]
            }]
        }))
        .unwrap();
        assert_eq!(
            anthropic_req.system_prompt.as_deref(),
            Some("You are terse")
        );

        let chat_req = parse_openai_chat_request(&json!({
            "model": "deepseek-v4",
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();
        let up_anth = to_upstream_anthropic_body(&chat_req, "MiniMax-M2.5", false);
        assert_eq!(up_anth["model"], "MiniMax-M2.5");

        let resp_req = parse_openai_responses_request(&json!({
            "model": "coding-main/claude-sonnet-4.6",
            "instructions": "Be brief",
            "input": "ping"
        }))
        .unwrap();
        assert_eq!(resp_req.system_prompt.as_deref(), Some("Be brief"));
        assert_eq!(resp_req.messages.len(), 1);
    }

    #[test]
    fn forwards_tools_and_tool_choice_across_protocols() {
        // Anthropic 入站 -> OpenAI Chat 上游
        let anth = parse_anthropic_messages_request(&json!({
            "model": "claude-sonnet-4.6",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": "list files" }],
            "tools": [{ "name": "read_file", "description": "read a file",
                        "input_schema": { "type": "object", "properties": { "path": { "type": "string" } } } }],
            "tool_choice": { "type": "tool", "name": "read_file" }
        }))
        .unwrap();
        let up_chat = to_upstream_openai_chat_body(&anth, "gpt-x", false);
        assert_eq!(up_chat["tools"][0]["type"], "function");
        assert_eq!(up_chat["tools"][0]["function"]["name"], "read_file");
        assert_eq!(
            up_chat["tools"][0]["function"]["parameters"]["type"],
            "object"
        );
        assert_eq!(up_chat["tool_choice"]["function"]["name"], "read_file");

        // OpenAI Chat 入站 -> Anthropic 上游
        let chat = parse_openai_chat_request(&json!({
            "model": "deepseek-v4",
            "messages": [{ "role": "user", "content": "go" }],
            "tools": [{ "type": "function", "function": { "name": "run", "description": "run cmd",
                        "parameters": { "type": "object", "properties": {} } } }],
            "tool_choice": "required"
        }))
        .unwrap();
        let up_anth = to_upstream_anthropic_body(&chat, "MiniMax-M2.5", false);
        assert_eq!(up_anth["tools"][0]["name"], "run");
        assert_eq!(up_anth["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(up_anth["tool_choice"]["type"], "any");
    }

    #[test]
    fn openai_tool_result_keeps_call_id() {
        let anth = parse_anthropic_messages_request(&json!({
            "model": "claude",
            "messages": [
                { "role": "assistant", "content": [{ "type": "tool_use", "id": "call_1", "name": "read_file", "input": {} }] },
                { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "call_1", "content": "file body" }] }
            ]
        }))
        .unwrap();
        let up = to_upstream_openai_chat_body(&anth, "gpt", false);
        let msgs = up["messages"].as_array().unwrap();
        let tool_msg = msgs
            .iter()
            .find(|m| m["role"] == "tool")
            .expect("tool message present");
        assert_eq!(tool_msg["tool_call_id"], "call_1");
        assert_eq!(tool_msg["content"], "file body");
    }

    #[test]
    fn maps_finish_reasons_across_protocols() {
        let resp = IrResponse {
            id: "r".into(),
            model: "m".into(),
            text: "partial".into(),
            thinking: None,
            tool_calls: Vec::new(),
            finish_reason: "max_tokens".into(),
            usage: IrUsage {
                input_tokens: 1,
                output_tokens: 2,
            },
        };
        assert_eq!(
            to_anthropic_messages_response(&resp)["stop_reason"],
            "max_tokens"
        );
        assert_eq!(
            to_openai_chat_response(&resp)["choices"][0]["finish_reason"],
            "length"
        );
    }

    #[test]
    fn tool_call_and_result_ids_stay_consistent_for_long_ids() {
        let long_id = format!("call_{}", "x".repeat(80));
        // Responses: function_call 与 function_call_output 的 call_id 必须归一化一致
        let req = parse_openai_responses_request(&json!({
            "model": "m",
            "input": [
                {"type": "function_call", "call_id": long_id, "name": "read_file", "arguments": "{}"},
                {"type": "function_call_output", "call_id": long_id, "output": "body"}
            ]
        }))
        .unwrap();
        let tc_id = match &req.messages[0].parts[0] {
            IrPart::ToolCall { id, .. } => id.clone(),
            _ => panic!("expect tool call"),
        };
        let tr_id = match &req.messages[1].parts[0] {
            IrPart::ToolResult { call_id, .. } => call_id.clone(),
            _ => panic!("expect tool result"),
        };
        assert_eq!(tc_id, tr_id);
        assert_eq!(tc_id, normalize_tool_call_id(&long_id));

        // Anthropic: tool_use 与 tool_result 的 id 必须归一化一致
        let req2 = parse_anthropic_messages_request(&json!({
            "model": "m",
            "messages": [
                {"role": "assistant", "content": [{"type": "tool_use", "id": long_id, "name": "read_file", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": long_id, "content": "body"}]}
            ]
        }))
        .unwrap();
        let a = match &req2.messages[0].parts[0] {
            IrPart::ToolCall { id, .. } => id.clone(),
            _ => panic!("expect tool use"),
        };
        let b = match &req2.messages[1].parts[0] {
            IrPart::ToolResult { call_id, .. } => call_id.clone(),
            _ => panic!("expect tool result"),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn responses_sse_and_json_emit_full_lifecycle_and_function_calls() {
        let resp = IrResponse {
            id: "r1".into(),
            model: "hy4flash-free".into(),
            text: "Checking file".into(),
            thinking: None,
            tool_calls: vec![(
                "call_1".into(),
                "read_file".into(),
                "{\"path\":\"README.md\"}".into(),
            )],
            finish_reason: "tool_calls".into(),
            usage: IrUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        };

        let json_out = to_openai_responses_response(&resp);
        let output = json_out["output"].as_array().unwrap();
        assert_eq!(output.len(), 2);
        assert_eq!(output[0]["type"], "message");
        assert_eq!(output[0]["content"][0]["type"], "output_text");
        assert_eq!(output[0]["content"][0]["text"], "Checking file");
        assert_eq!(output[1]["type"], "function_call");
        assert_eq!(output[1]["call_id"], "call_1");
        assert_eq!(output[1]["name"], "read_file");

        let sse = to_openai_responses_sse(&resp);
        let p_created = sse.find("event: response.created").unwrap();
        let p_in_prog = sse.find("event: response.in_progress").unwrap();
        let p_item_add = sse.find("event: response.output_item.added").unwrap();
        let p_part_add = sse.find("event: response.content_part.added").unwrap();
        let p_txt_delta = sse.find("event: response.output_text.delta").unwrap();
        let p_txt_done = sse.find("event: response.output_text.done").unwrap();
        let p_part_done = sse.find("event: response.content_part.done").unwrap();
        let p_item_done = sse.find("event: response.output_item.done").unwrap();
        let p_fc_delta = sse
            .find("event: response.function_call_arguments.delta")
            .unwrap();
        let p_fc_done = sse
            .find("event: response.function_call_arguments.done")
            .unwrap();
        let p_completed = sse.find("event: response.completed").unwrap();

        assert!(p_created < p_in_prog);
        assert!(p_in_prog < p_item_add);
        assert!(p_item_add < p_part_add);
        assert!(p_part_add < p_txt_delta);
        assert!(p_txt_delta < p_txt_done);
        assert!(p_txt_done < p_part_done);
        assert!(p_part_done < p_item_done);
        assert!(p_item_done < p_fc_delta);
        assert!(p_fc_delta < p_fc_done);
        assert!(p_fc_done < p_completed);
    }

    #[test]
    fn anthropic_and_openai_chat_preserve_thinking_and_tool_calls() {
        let resp = IrResponse {
            id: "a1".into(),
            model: "deepseek-v4".into(),
            text: "Using tool".into(),
            thinking: Some("hmm".into()),
            tool_calls: vec![("toolu_1".into(), "bash".into(), "{\"cmd\":\"ls\"}".into())],
            finish_reason: "tool_calls".into(),
            usage: IrUsage {
                input_tokens: 12,
                output_tokens: 8,
            },
        };

        let anth_json = to_anthropic_messages_response(&resp);
        assert_eq!(anth_json["stop_reason"], "tool_use");
        assert_eq!(anth_json["content"][0]["type"], "thinking");
        assert_eq!(anth_json["content"][1]["type"], "text");
        assert_eq!(anth_json["content"][2]["type"], "tool_use");
        assert_eq!(anth_json["content"][2]["id"], "toolu_1");

        let anth_sse = to_anthropic_messages_sse(&resp);
        assert!(anth_sse.contains("thinking_delta"));
        assert!(anth_sse.contains("text_delta"));
        assert!(anth_sse.contains("\"type\":\"tool_use\""));
        assert!(anth_sse.contains("input_json_delta"));
        assert!(anth_sse.contains("\"stop_reason\":\"tool_use\""));

        let chat_json = to_openai_chat_response(&resp);
        assert_eq!(chat_json["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(
            chat_json["choices"][0]["message"]["reasoning_content"],
            "hmm"
        );
        assert_eq!(
            chat_json["choices"][0]["message"]["tool_calls"][0]["id"],
            "toolu_1"
        );

        let chat_sse = to_openai_chat_sse(&resp);
        assert!(chat_sse.contains("\"reasoning_content\":\"hmm\""));
        assert!(chat_sse.contains("\"tool_calls\""));
        assert!(chat_sse.contains("\"index\":0"));
        assert!(chat_sse.contains("\"finish_reason\":\"tool_calls\""));
        assert!(chat_sse.contains("\"usage\""));
    }

    #[test]
    fn parallel_tool_results_merge_and_responses_items_normalize() {
        let req = json!({
            "model": "claude-sonnet-4-6",
            "tools": [{ "type": "function", "name": "read_file", "parameters": { "type": "object" } }],
            "tool_choice": { "type": "function", "name": "read_file" },
            "input": [
                { "role": "developer", "content": "Be concise." },
                { "role": "user", "content": "Read both" },
                { "role": "assistant", "content": "Reading..." },
                { "type": "function_call", "call_id": "c1", "name": "read_file", "arguments": "{\"p\":\"a\"}" },
                { "type": "function_call", "call_id": "c2", "name": "read_file", "arguments": "{\"p\":\"b\"}" },
                { "type": "function_call_output", "call_id": "c1", "output": "ok1" },
                { "type": "function_call_output", "call_id": "c2", "output": "ok2" }
            ]
        });
        let ir = parse_openai_responses_request(&req).expect("parse responses");
        assert_eq!(ir.system_prompt.as_deref(), Some("Be concise."));
        assert_eq!(ir.messages.len(), 4); // user, assistant (text + 2 calls), tool c1, tool c2
        assert_eq!(ir.messages[1].role, IrRole::Assistant);
        assert_eq!(ir.messages[1].parts.len(), 3);

        let anth = to_upstream_anthropic_body(&ir, "claude-sonnet-4-6", false);
        assert_eq!(anth["tool_choice"]["type"], "tool");
        assert_eq!(anth["tool_choice"]["name"], "read_file");
        let msgs = anth["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3); // user -> assistant -> single merged user with 2 tool_results
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"].as_array().unwrap().len(), 2);

        let openai = to_upstream_openai_chat_body(&ir, "gpt-5", false);
        assert_eq!(openai["tool_choice"]["type"], "function");
        assert_eq!(openai["tool_choice"]["function"]["name"], "read_file");
    }

    #[test]
    fn openai_chat_with_tools_and_image_encodes_to_responses_with_constraints() {
        let chat = parse_openai_chat_request(&json!({
            "model": "gpt-5.4",
            "temperature": 0.7,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Inspect this screenshot" },
                        { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } }
                    ]
                }
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "read file",
                    "parameters": { "type": "object", "properties": {} }
                }
            }],
            "tool_choice": "required"
        }))
        .unwrap();
        assert!(chat.has_images());

        let constraints = EgressConstraints {
            must_stream: true,
            requires_instructions: true,
            disallow_store: true,
            strip_temperature_on_reasoning: true,
            ..Default::default()
        };
        let body = to_upstream_openai_responses_body(&chat, "gpt-5.4", false, Some(&constraints));
        assert_eq!(body["model"], "gpt-5.4");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert!(body["instructions"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(body.get("temperature").is_none());
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "read_file");
        assert_eq!(body["tool_choice"], "required");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][0]["content"][1]["type"], "input_image");
        assert_eq!(
            body["input"][0]["content"][1]["image_url"],
            "data:image/png;base64,AAAA"
        );
    }

    #[test]
    fn parses_responses_json_and_sse_back_into_chat_and_anthropic() {
        let resp_json = json!({
            "id": "resp_abc123",
            "object": "response",
            "status": "completed",
            "output": [
                { "type": "reasoning", "summary": [{ "type": "summary_text", "text": "thinking..." }] },
                { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "Done." }] },
                { "type": "function_call", "call_id": "call_99", "name": "bash", "arguments": "{\"cmd\":\"pwd\"}" }
            ],
            "usage": { "input_tokens": 15, "output_tokens": 9 }
        });
        let ir = parse_openai_responses_json(&resp_json, "gpt-5.4").expect("responses json");
        assert_eq!(ir.text, "Done.");
        assert_eq!(ir.thinking.as_deref(), Some("thinking..."));
        assert_eq!(ir.tool_calls.len(), 1);
        assert_eq!(ir.tool_calls[0].0, "call_99");

        let chat = to_openai_chat_response(&ir);
        assert_eq!(chat["choices"][0]["message"]["content"], "Done.");
        assert_eq!(
            chat["choices"][0]["message"]["tool_calls"][0]["id"],
            "call_99"
        );

        let anth = to_anthropic_messages_response(&ir);
        assert_eq!(anth["stop_reason"], "tool_use");

        let sse_raw = to_openai_responses_sse(&ir);
        let ir_from_sse = parse_openai_responses_sse(&sse_raw, "gpt-5.4").expect("responses sse");
        assert_eq!(ir_from_sse.text, "Done.");
        assert_eq!(ir_from_sse.tool_calls.len(), 1);
        assert_eq!(ir_from_sse.tool_calls[0].1, "bash");
    }
}
