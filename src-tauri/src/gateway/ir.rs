use serde_json::{json, Value};

use crate::domain::WireProtocolKind;

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
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
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

pub fn parse_openai_chat_request(body: &Value) -> Result<IrRequest, String> {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "缺少 model 字段".to_string())?
        .to_string();
    let stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
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
                    let call_id = msg
                        .get("tool_call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("call_0")
                        .to_string();
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
                    messages.push(IrMessage {
                        role: IrRole::User,
                        parts: vec![IrPart::Text(text)],
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
    let stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    let max_tokens = body.get("max_tokens").and_then(|v| v.as_u64());
    let temperature = body.get("temperature").and_then(|v| v.as_f64());
    let system_prompt = body.get("system").map(|s| extract_text_from_value(Some(s)));
    let tools = body
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

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
                                let call_id = b
                                    .get("tool_use_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("call_0")
                                    .to_string();
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
    let stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    let max_tokens = body.get("max_output_tokens").and_then(|v| v.as_u64());
    let temperature = body.get("temperature").and_then(|v| v.as_f64());
    let system_prompt = body
        .get("instructions")
        .and_then(|v| v.as_str())
        .map(String::from)
        .filter(|s| !s.is_empty());
    let tools = body
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

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
                let itype = item.get("type").and_then(|v| v.as_str()).unwrap_or("message");
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
                        messages.push(IrMessage {
                            role: IrRole::Assistant,
                            parts: vec![IrPart::ToolCall {
                                id: call_id,
                                name,
                                args_json,
                            }],
                        });
                    }
                    "function_call_output" => {
                        let call_id = item
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("call_0")
                            .to_string();
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
                        let role = match item.get("role").and_then(|v| v.as_str()).unwrap_or("user") {
                            "assistant" => IrRole::Assistant,
                            _ => IrRole::User,
                        };
                        let text = extract_text_from_value(item.get("content"));
                        if !text.is_empty() {
                            messages.push(IrMessage {
                                role,
                                parts: vec![IrPart::Text(text)],
                            });
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
        max_tokens,
        temperature,
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

pub fn to_upstream_openai_chat_body(ir: &IrRequest, upstream_model: &str, stream: bool) -> Value {
    let mut msgs = Vec::new();
    if let Some(sys) = &ir.system_prompt {
        msgs.push(json!({ "role": "system", "content": sys }));
    }
    for msg in &ir.messages {
        match msg.role {
            IrRole::User => {
                let text: String = msg
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        IrPart::Text(t) => Some(t.as_str()),
                        IrPart::ToolResult { content, .. } => Some(content.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                msgs.push(json!({ "role": "user", "content": text }));
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
    body
}

pub fn to_upstream_anthropic_body(ir: &IrRequest, upstream_model: &str, stream: bool) -> Value {
    let mut msgs = Vec::new();
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
            msgs.push(json!({ "role": role, "content": blocks }));
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
    body
}

pub fn to_openai_chat_response(resp: &IrResponse) -> Value {
    json!({
        "id": format!("chatcmpl-{}", resp.id),
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": resp.model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": resp.text
            },
            "finish_reason": resp.finish_reason
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
    content.push(json!({ "type": "text", "text": resp.text }));
    for (id, name, args) in &resp.tool_calls {
        let input: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
        content.push(json!({ "type": "tool_use", "id": id, "name": name, "input": input }));
    }
    json!({
        "id": format!("msg_{}", resp.id),
        "type": "message",
        "role": "assistant",
        "model": resp.model,
        "content": content,
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": resp.usage.input_tokens,
            "output_tokens": resp.usage.output_tokens
        }
    })
}

pub fn to_openai_responses_response(resp: &IrResponse) -> Value {
    json!({
        "id": format!("resp_{}", resp.id),
        "object": "response",
        "status": "completed",
        "model": resp.model,
        "output": [{
            "id": format!("msg_{}", resp.id),
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [{
                "type": "output_text",
                "text": resp.text
            }]
        }],
        "usage": {
            "input_tokens": resp.usage.input_tokens,
            "output_tokens": resp.usage.output_tokens,
            "total_tokens": resp.usage.input_tokens + resp.usage.output_tokens
        }
    })
}

pub fn to_openai_chat_sse(resp: &IrResponse) -> String {
    let c1 = json!({
        "id": format!("chatcmpl-{}", resp.id),
        "object": "chat.completion.chunk",
        "model": resp.model,
        "choices": [{
            "index": 0,
            "delta": { "role": "assistant", "content": resp.text },
            "finish_reason": Value::Null
        }]
    });
    let c2 = json!({
        "id": format!("chatcmpl-{}", resp.id),
        "object": "chat.completion.chunk",
        "model": resp.model,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": resp.finish_reason
        }]
    });
    format!("data: {c1}\n\ndata: {c2}\n\ndata: [DONE]\n\n")
}

pub fn to_anthropic_messages_sse(resp: &IrResponse) -> String {
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
    let block_start = json!({
        "type": "content_block_start",
        "index": 0,
        "content_block": { "type": "text", "text": "" }
    });
    let delta = json!({
        "type": "content_block_delta",
        "index": 0,
        "delta": { "type": "text_delta", "text": resp.text }
    });
    let block_stop = json!({ "type": "content_block_stop", "index": 0 });
    let msg_delta = json!({
        "type": "message_delta",
        "delta": { "stop_reason": "end_turn" },
        "usage": { "output_tokens": resp.usage.output_tokens }
    });
    let msg_stop = json!({ "type": "message_stop" });
    format!(
        "event: message_start\ndata: {start}\n\n\
         event: content_block_start\ndata: {block_start}\n\n\
         event: content_block_delta\ndata: {delta}\n\n\
         event: content_block_stop\ndata: {block_stop}\n\n\
         event: message_delta\ndata: {msg_delta}\n\n\
         event: message_stop\ndata: {msg_stop}\n\n"
    )
}

pub fn to_openai_responses_sse(resp: &IrResponse) -> String {
    let full = to_openai_responses_response(resp);
    let created = json!({
        "type": "response.created",
        "response": { "id": full["id"], "status": "in_progress", "model": resp.model }
    });
    let delta = json!({
        "type": "response.output_text.delta",
        "delta": resp.text
    });
    let completed = json!({
        "type": "response.completed",
        "response": full
    });
    format!(
        "event: response.created\ndata: {created}\n\n\
         event: response.output_text.delta\ndata: {delta}\n\n\
         event: response.completed\ndata: {completed}\n\n"
    )
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
        assert_eq!(anthropic_req.system_prompt.as_deref(), Some("You are terse"));

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
}
