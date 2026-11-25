//! Qoder CN 对话私有协议的 COSY 信封与 body 编解码。
//!
//! 协议依据 magpie 的 `opencode-qoder-auth`：
//! - 请求体不是明文 JSON，而是 `encodeBody(JSON)`：先用一套打乱顺序的
//!   自定义 base64 字母表（`$` 补位）编码，再把首尾各三分之一互换；
//! - 鉴权用 `Authorization: Bearer COSY.{payload}.{sig}`，其中
//!   `payload` = base64(JSON{v1, requestId, info, cosyVersion, ideVersion})，
//!   `info` 是账号信息用一把随机 AES-128 密钥（key=iv）加密后的 base64，
//!   该 AES 密钥再用 Qoder 内置 RSA-1024 公钥（PKCS#1 v1.5）包裹放进 `Cosy-Key`；
//!   `sig` = md5(`{payload}\n{Cosy-Key}\n{ts}\n{实际发送的 body}\n{去掉 /algo 前缀的 path}`)。
//! - 会话走 `{api}/algo/api/v2/service/pro/sse/agent_chat_generation?...`，
//!   响应是 SSE，每帧 `data: {statusCodeValue, body:"<OpenAI chunk JSON 字符串>"}`。

use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use md5::{Digest as Md5Digest, Md5};
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use serde_json::{json, Value};
use uuid::Uuid;

/// Qoder 内置的 1024-bit RSA 公钥（来自客户端 cosy 源码），仅用于包裹每次请求的 AES 密钥。
const QODER_RSA_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDA8iMH5c02LilrsERw9t6Pv5Nc
4k6Pz1EaDicBMpdpxKduSZu5OANqUq8er4GM95omAGIOPOh+Nx0spthYA2BqGz+l
6HRkPJ7S236FZz73In/KVuLnwI8JJ2CbuJap8kvheCCZpmAWpb/cPx/3Vr/J6I17
XcW+ML9FoCI6AOvOzwIDAQAB
-----END PUBLIC KEY-----";

/// Qoder 客户端约定的 COSY 版本号（app 0.4.8 / transport 1.1.69）。
pub const COSY_VERSION: &str = "1.1.69";

/// 对话端点（含 query）。
pub const CHAT_PATH: &str =
    "/algo/api/v2/service/pro/sse/agent_chat_generation?FetchKeys=llm_model_result&AgentId=agent_common&Encode=1";

/// 模型列表端点。
pub const MODELS_PATH: &str = "/algo/api/v2/model/list?Encode=1";

const QODER_SYS: &str =
    "You are a Qoder agent. Use the instructions below and the tools available to you to assist the user.";

/// 客户端自定义的 base64 打乱字母表（`$` 作补位）。
const ALPHABET: &[u8; 64] = b"_doRTgHZBKcGVjlvpC,@aFSx#DPuNJme&i*MzLOEn)sUrthbf%Y^w.(kIQyXqWA!";

fn alphabet_val(c: u8) -> Option<u8> {
    ALPHABET.iter().position(|&b| b == c).map(|p| p as u8)
}

fn segment_encode(bytes: &[u8]) -> String {
    let mut s = String::new();
    let mut acc: u32 = 0;
    let mut nb = 0u32;
    for &b in bytes {
        acc = ((acc << 8) | b as u32) & 0xffffff;
        nb += 8;
        while nb >= 6 {
            nb -= 6;
            s.push(ALPHABET[((acc >> nb) & 63) as usize] as char);
        }
    }
    if nb > 0 {
        s.push(ALPHABET[((acc << (6 - nb)) & 63) as usize] as char);
    }
    while s.len() % 4 != 0 {
        s.push('$');
    }
    s
}

fn swap_thirds(s: &str) -> String {
    // 按字节处理：字母表全是 ASCII，swap 只做字节级切片交换
    let bytes = s.as_bytes();
    let t = bytes.len() / 3;
    if t == 0 {
        return s.to_string();
    }
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[bytes.len() - t..]);
    out.extend_from_slice(&bytes[t..bytes.len() - t]);
    out.extend_from_slice(&bytes[..t]);
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// 请求体编码：打乱 base64 后再首尾三分之一互换。
pub fn encode_body(text: &str) -> String {
    swap_thirds(&segment_encode(text.as_bytes()))
}

/// 请求体解码（测试与排查用）。
pub fn decode_body(wire: &str) -> String {
    let s = swap_thirds(wire);
    let chars: Vec<u8> = s.bytes().collect();
    let mut out: Vec<u8> = Vec::new();
    let mut g = 0;
    while g + 4 <= chars.len() {
        let vals: Vec<u8> = chars[g..g + 4]
            .iter()
            .filter(|&&c| c != b'$')
            .filter_map(|&c| alphabet_val(c))
            .collect();
        let mut x: u64 = 0;
        for &v in &vals {
            x = x * 64 + v as u64;
        }
        let n = (6 * vals.len()) / 8;
        for k in 0..n {
            let shift = 6 * vals.len() - 8 * (k + 1);
            out.push(((x >> shift) & 255) as u8);
        }
        g += 4;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 账号 blob：AES-128-CBC（key=iv=16 字节随机）加密账号 JSON，AES 密钥用 RSA 公钥包裹。
/// 返回 `(info_base64, wrapped_key_base64)`。
fn user_blob(uid: &str, name: &str, email: &str, access: &str) -> Result<(String, String), String> {
    let raw = json!({
        "uid": uid,
        "aid": "",
        "name": name,
        "email": email,
        "security_oauth_token": access,
    })
    .to_string();
    // 关键：AES-128 的密钥必须是 16 个「十六进制 ASCII 字符」（对齐客户端
    // `Buffer.from(randomUUID().replaceAll("-","").slice(0,16))`），直接用随机
    // 原始字节会被服务端判为 Signature invalid。
    let hex = Uuid::new_v4().simple().to_string();
    let key_bytes = hex.as_bytes()[..16].to_vec();
    let mut key = [0u8; 16];
    key.copy_from_slice(&key_bytes);
    type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
    let cipher = Aes128CbcEnc::new(&key.into(), &key.into());
    let ciphertext = cipher.encrypt_padded_vec_mut::<Pkcs7>(raw.as_bytes());
    let info = BASE64.encode(ciphertext);

    let pubkey = RsaPublicKey::from_public_key_pem(QODER_RSA_PUBLIC_KEY)
        .map_err(|e| format!("解析 Qoder RSA 公钥失败: {e}"))?;
    let mut rng = rand::thread_rng();
    let wrapped = pubkey
        .encrypt(&mut rng, Pkcs1v15Encrypt, &key)
        .map_err(|e| format!("RSA 包裹 AES 密钥失败: {e}"))?;
    Ok((info, BASE64.encode(wrapped)))
}

/// 计算 COSY 请求头。`machine_id` 来自账号快照；`body` 为**实际发送**的 wire body 字符串。
#[allow(clippy::too_many_arguments)]
pub fn cosy_headers(
    url: &str,
    uid: &str,
    name: &str,
    email: &str,
    access: &str,
    machine_id: &str,
    body: &str,
    is_chat: bool,
    model_key: Option<&str>,
    model_source: Option<&str>,
) -> Result<Vec<(String, String)>, String> {
    let (info, wrapped_key) = user_blob(uid, name, email, access)?;
    let request_id = Uuid::new_v4().to_string();
    let payload_json = json!({
        "version": "v1",
        "requestId": request_id,
        "info": info,
        "cosyVersion": COSY_VERSION,
        "ideVersion": "",
    });
    let payload = BASE64.encode(payload_json.to_string().as_bytes());

    // path：取 url 的 pathname，去掉开头 /algo
    let path = {
        let after_scheme = url.splitn(2, "://").nth(1).unwrap_or(url);
        let path = match after_scheme.find('/') {
            Some(i) => &after_scheme[i..],
            None => "/",
        };
        let path = path.split('?').next().unwrap_or(path);
        if let Some(rest) = path.strip_prefix("/algo") {
            rest.to_string()
        } else {
            path.to_string()
        }
    };

    let ts = chrono::Utc::now().timestamp();
    let mut hasher = Md5::new();
    hasher.update(format!("{payload}\n{wrapped_key}\n{ts}\n{body}\n{path}").as_bytes());
    let sig = hex::encode(hasher.finalize());

    let mut headers: Vec<(String, String)> = vec![
        ("Accept".into(), "application/json".into()),
        ("Accept-Encoding".into(), "identity".into()),
        ("Content-Type".into(), "application/json".into()),
        ("Authorization".into(), format!("Bearer COSY.{payload}.{sig}")),
        ("Cosy-Business-Product".into(), "app".into()),
        ("Cosy-Business-Type".into(), "agent".into()),
        ("Cosy-ClientIp".into(), machine_id.to_string()),
        ("Cosy-ClientType".into(), "10".into()),
        ("Cosy-Data-Policy".into(), "disagree".into()),
        ("Cosy-Date".into(), ts.to_string()),
        ("Cosy-Key".into(), wrapped_key),
        ("Cosy-MachineId".into(), machine_id.to_string()),
        ("Cosy-MachineToken".into(), machine_id.to_string()),
        ("Cosy-MachineType".into(), "5".into()),
        ("Cosy-MachineOS".into(), machine_os()),
        ("Cosy-Scene".into(), "app".into()),
        ("Cosy-User".into(), uid.to_string()),
        ("Cosy-Version".into(), COSY_VERSION.into()),
        ("Login-Version".into(), "v2".into()),
    ];

    if is_chat {
        // 对话头：去掉 ClientIp / MachineToken / MachineType，改带 hostname，并附模型键
        headers.retain(|(k, _)| {
            k != "Cosy-ClientIp" && k != "Cosy-MachineToken" && k != "Cosy-MachineType"
        });
        headers.push(("Accept".into(), "text/event-stream".into()));
        headers.push(("Cache-Control".into(), "no-cache".into()));
        if let Some(host) = machine_hostname() {
            headers.push(("Cosy-MachineHostname".into(), host));
        }
        if let Some(k) = model_key {
            headers.push(("X-Model-Key".into(), k.to_string()));
        }
        if let Some(s) = model_source {
            headers.push(("X-Model-Source".into(), s.to_string()));
        }
    }
    Ok(headers)
}

fn machine_os() -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => other,
    };
    let platform = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    format!("{arch}_{platform}")
}

fn machine_hostname() -> Option<String> {
    let name = hostname_string()?;
    let v = name.trim().to_string();
    if v.is_empty() {
        return None;
    }
    let printable = v.bytes().all(|b| (0x20..=0x7e).contains(&b));
    if printable {
        Some(cap_hostname(&v))
    } else {
        Some(format!("unknown-{}", short_hash(&v)))
    }
}

fn cap_hostname(v: &str) -> String {
    if v.len() <= 96 {
        return v.to_string();
    }
    let head = v[..96 - 8 - 1].trim_end_matches(['-', ' ']);
    if head.is_empty() {
        format!("unknown-{}", short_hash(v))
    } else {
        format!("{head}-{}", short_hash(v))
    }
}

fn short_hash(v: &str) -> String {
    use sha2::{Digest as ShaDigest, Sha256};
    let mut h = Sha256::new();
    h.update(v.as_bytes());
    hex::encode(h.finalize())[..8].to_string()
}

fn hostname_string() -> Option<String> {
    // 优先环境变量（跨平台稳定），回退 /etc/hostname；取不到则不带头
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.trim().is_empty() {
            return Some(h);
        }
    }
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        if !h.trim().is_empty() {
            return Some(h);
        }
    }
    None
}

/// 把 IR 的普通对话消息转成 Qoder 的 `messages` 结构（system 单独处理）。
/// Qoder 接受 content 为「块数组」：文本 `{type:text,text}`。
pub fn build_qoder_messages(
    system: Option<&str>,
    messages: &[crate::gateway::ir::IrMessage],
) -> Value {
    use crate::gateway::ir::{IrPart, IrRole};

    // 系统提示：合并系统消息 + 前置 Qoder 身份行
    let mut sys_parts: Vec<String> = Vec::new();
    if let Some(s) = system {
        if !s.trim().is_empty() {
            sys_parts.push(s.to_string());
        }
    }
    for m in messages {
        if m.role == IrRole::System {
            let t: String = m
                .parts
                .iter()
                .filter_map(|p| match p {
                    IrPart::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !t.trim().is_empty() {
                sys_parts.push(t);
            }
        }
    }
    let sys_text = {
        let extra = sys_parts.join("\n\n");
        if extra.is_empty() {
            QODER_SYS.to_string()
        } else {
            format!("{QODER_SYS}\n\n{extra}")
        }
    };

    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        match m.role {
            IrRole::System => {}
            IrRole::User => {
                let mut blocks: Vec<Value> = Vec::new();
                for p in &m.parts {
                    match p {
                        IrPart::Text(t) if !t.is_empty() => {
                            blocks.push(json!({ "type": "text", "text": t }))
                        }
                        IrPart::ToolResult { content, .. } if !content.is_empty() => {
                            blocks.push(json!({ "type": "text", "text": content }))
                        }
                        _ => {}
                    }
                }
                if !blocks.is_empty() {
                    out.push(json!({ "role": "user", "content": blocks }));
                }
            }
            IrRole::Assistant => {
                let text: String = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        IrPart::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let tool_calls: Vec<Value> = m
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
                if text.is_empty() && tool_calls.is_empty() {
                    continue;
                }
                let mut msg = json!({ "role": "assistant", "content": text });
                if !tool_calls.is_empty() {
                    msg["tool_calls"] = json!(tool_calls);
                }
                out.push(msg);
            }
            IrRole::Tool => {
                for p in &m.parts {
                    if let IrPart::ToolResult {
                        call_id, content, ..
                    } = p
                    {
                        out.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": content
                        }));
                    }
                }
            }
        }
    }

    json!({
        "system_block": { "type": "text", "text": sys_text },
        "messages": out,
    })
}

/// 生成 `agent_chat_generation` 的明文 body（之后再 encodeBody）。
#[allow(clippy::too_many_arguments)]
pub fn build_qoder_body(
    ir: &crate::gateway::ir::IrRequest,
    model_key: &str,
    model_source: &str,
    display_name: &str,
    max_input_tokens: u64,
) -> Value {
    let built = build_qoder_messages(ir.system_prompt.as_deref(), &ir.messages);
    let sys_block = built["system_block"].clone();
    let user_msgs = built["messages"].as_array().cloned().unwrap_or_default();

    let mut messages: Vec<Value> = Vec::new();
    messages.push(json!({ "role": "system", "content": [sys_block.clone()] }));
    messages.extend(user_msgs);

    let request_id = Uuid::new_v4().to_string();
    let session_id = Uuid::new_v4().to_string();
    let task_id = Uuid::new_v4().to_string();

    let max_tokens = ir.max_tokens.unwrap_or(32000);
    let parameters = json!({
        "enable_thinking": false,
        "max_tokens": max_tokens,
    });

    let mut body = json!({
        "request_id": request_id,
        "request_set_id": task_id,
        "chat_record_id": request_id,
        "session_id": session_id,
        "parameters": parameters,
        "business": {
            "product": "app",
            "version": COSY_VERSION,
            "type": "agent",
            "id": task_id,
            "name": "",
            "begin_at": chrono::Utc::now().timestamp_millis(),
            "stage": "start",
            "model_key": model_key,
        },
        "agent_id": "agent_common",
        "task_id": "common",
        "session_type": "app",
        "model_config": {
            "key": model_key,
            "display_name": display_name,
            "model": "",
            "format": "openai",
            "is_vl": false,
            "is_reasoning": false,
            "api_key": "",
            "url": "",
            "source": model_source,
            "max_input_tokens": max_input_tokens,
        },
        "system": [sys_block],
        "messages": messages,
    });

    if !ir.tools.is_empty() {
        let tools: Vec<Value> = ir
            .tools
            .iter()
            .filter_map(|t| {
                let f = t.get("function")?;
                let name = f.get("name")?.as_str()?;
                Some(json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": f.get("description").and_then(Value::as_str).unwrap_or(""),
                        "parameters": f.get("parameters").cloned().unwrap_or(json!({"type":"object","properties":{}})),
                    }
                }))
            })
            .collect();
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_codec_roundtrips() {
        let text = r#"{"hello":"世界","n":123}"#;
        let wire = encode_body(text);
        assert_ne!(wire, text);
        assert_eq!(decode_body(&wire), text);
    }

    #[test]
    fn segment_encode_matches_manual_alphabet() {
        // 空输入 -> 空串
        assert_eq!(segment_encode(b""), "");
        // 基本可逆：三字节 -> 四字符
        let s = segment_encode(b"abc");
        assert_eq!(s.len(), 4);
        assert!(s.bytes().all(|b| ALPHABET.contains(&b) || b == b'$'));
    }

    #[test]
    fn swap_thirds_inverts_itself_logic() {
        let s = "012345678";
        let once = swap_thirds(s);
        assert_eq!(once.len(), s.len());
    }

    #[test]
    fn user_blob_produces_base64_key_and_info() {
        let (info, key) = user_blob("3704729780097880", "tester", "t@e.com", "tok").unwrap();
        assert!(!info.is_empty());
        assert!(!key.is_empty());
        // RSA-1024 密文 = 128 字节 -> base64 长度 172
        assert_eq!(key.len(), 172);
    }

    #[test]
    fn cosy_headers_have_authorization_and_cosy_key() {
        let h = cosy_headers(
            "https://gateway.qoder.com.cn/algo/api/v2/service/pro/sse/agent_chat_generation?Encode=1",
            "uid1",
            "name",
            "e@x.com",
            "access",
            "machine-1",
            "{\"a\":1}",
            true,
            Some("deepseek-v4.1-flash"),
            Some("system"),
        )
        .unwrap();
        let get = |k: &str| h.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
        let auth = get("Authorization").expect("must have Authorization");
        assert!(auth.starts_with("Bearer COSY."));
        assert_eq!(auth.matches('.').count(), 2);
        assert!(get("Cosy-Key").is_some());
        assert_eq!(get("Cosy-Version").as_deref(), Some(COSY_VERSION));
        assert_eq!(get("X-Model-Key").as_deref(), Some("deepseek-v4.1-flash"));
        // 对话头不应含 ClientIp
        assert!(get("Cosy-ClientIp").is_none());
    }

    #[test]
    fn qoder_body_has_required_fields() {
        use crate::domain::WireProtocolKind;
        use crate::gateway::ir::{IrMessage, IrPart, IrRequest, IrRole};
        let ir = IrRequest {
            incoming_protocol: WireProtocolKind::OpenaiChat,
            model: "deepseek-v4.1-flash".into(),
            stream: true,
            system_prompt: Some("you are helpful".into()),
            messages: vec![IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text("hi".into())],
            }],
            tools: vec![],
            tool_choice: None,
            max_tokens: Some(1024),
            temperature: None,
        };
        let b = build_qoder_body(&ir, "deepseek-v4.1-flash", "system", "DeepSeek", 200000);
        assert_eq!(b["agent_id"], "agent_common");
        assert_eq!(b["model_config"]["key"], "deepseek-v4.1-flash");
        assert_eq!(b["business"]["model_key"], "deepseek-v4.1-flash");
        assert_eq!(b["parameters"]["max_tokens"], 1024);
        assert!(b["messages"].as_array().unwrap().len() >= 2);
    }
}

#[cfg(test)]
mod codec_vectors {
    use super::*;
    #[test]
    fn matches_node_reference() {
        assert_eq!(encode_body(r#"{"hello":"世界","n":123}"#), "BOI*lMTYVkw$Bh,ILhmFMoBrmYKnDS%ruYBy");
        assert_eq!(encode_body("abc"), "MSK#");
    }
}
