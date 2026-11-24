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
        resp.finish_reason.as_str()
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
    let stop_reason = if resp.tool_calls.is_empty() {
        "end_turn"
    } else {
        "tool_use"
    };
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
        resp.finish_reason.as_str()
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

    let stop_reason = if resp.tool_calls.is_empty() {
        "end_turn"
    } else {
        "tool_use"
    };
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
        assert_eq!(chat_json["choices"][0]["message"]["reasoning_content"], "hmm");
        assert_eq!(chat_json["choices"][0]["message"]["tool_calls"][0]["id"], "toolu_1");

        let chat_sse = to_openai_chat_sse(&resp);
        assert!(chat_sse.contains("\"reasoning_content\":\"hmm\""));
        assert!(chat_sse.contains("\"tool_calls\""));
        assert!(chat_sse.contains("\"index\":0"));
        assert!(chat_sse.contains("\"finish_reason\":\"tool_calls\""));
        assert!(chat_sse.contains("\"usage\""));
    }
}
