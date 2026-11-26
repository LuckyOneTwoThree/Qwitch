//! Qoder COSY 真机探针：从本机 qwitch.db 读 qoder_cn 账户，解密凭据，
//! 用 COSY 协议真实调用 agent_chat_generation，验证签名/编码/解析全链路。
//!
//! ```bash
//! cargo run --example qoder_cosy_probe -- <account_label> "<prompt>"
//! ```

use std::path::PathBuf;

use qwitch_lib::adapter::qoder_cosy;
use qwitch_lib::domain::{CredentialEnvelope, CredentialPayload, WireProtocolKind};
use qwitch_lib::gateway::ir::{IrMessage, IrPart, IrRequest, IrRole};
use qwitch_lib::storage::crypto;
use rusqlite::Connection;

fn master_key() -> [u8; 32] {
    let out = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "dev.qwitch.app",
            "-a",
            "master_key_v1",
            "-w",
        ])
        .output()
        .expect("keychain");
    let hex = String::from_utf8(out.stdout).unwrap();
    let hex = hex.trim();
    let mut key = [0u8; 32];
    for i in 0..32 {
        key[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    key
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let label = args.get(1).cloned().unwrap_or_else(|| "主-01".into());
    let prompt = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "用一句话介绍你自己".into());

    let db = PathBuf::from(std::env::var("HOME").unwrap())
        .join("Library/Application Support/qwitch/qwitch.db");
    let conn = Connection::open(&db).expect("open db");
    let (id, blob): (String, Vec<u8>) = conn
        .query_row(
            "SELECT id, encrypted_credential FROM accounts WHERE tool_id='qoder_cn' AND label=?1 LIMIT 1",
            [&label],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("account row");

    let mk = master_key();
    let aad = crypto::credential_aad("qoder_cn", &id);
    let cred: CredentialEnvelope =
        crypto::decrypt_credential(&mk, &aad, &blob).expect("decrypt cred");

    let (token, machine_id) = match &cred.payload {
        CredentialPayload::Token {
            access_token,
            extra,
            ..
        } => {
            println!(
                "PAYLOAD=Token extra_keys={:?}",
                extra.keys().collect::<Vec<_>>()
            );
            (
                access_token.clone(),
                extra.get("machine_id").cloned().unwrap_or_default(),
            )
        }
        CredentialPayload::LocalSnapshot { files } => {
            use base64::{engine::general_purpose::STANDARD as B, Engine as _};
            println!(
                "PAYLOAD=LocalSnapshot file_keys={:?}",
                files.keys().collect::<Vec<_>>()
            );
            let tok = files
                .get("decrypted_token.txt")
                .and_then(|b| B.decode(b).ok())
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_default();
            let mid = files
                .get("auth.machine-id")
                .and_then(|b| B.decode(b).ok())
                .and_then(|b| String::from_utf8(b).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            (tok, mid)
        }
        _ => (String::new(), String::new()),
    };
    println!("account={label} uid={} machine_id={machine_id}", cred.uid);
    println!("token_len={}", token.len());

    let ir = IrRequest {
        incoming_protocol: WireProtocolKind::OpenaiChat,
        model: "deepseek-v4.1-flash".into(),
        stream: true,
        system_prompt: Some("You are a helpful assistant. Answer briefly.".into()),
        messages: vec![IrMessage {
            role: IrRole::User,
            parts: vec![IrPart::Text(prompt.clone())],
        }],
        tools: vec![],
        tool_choice: None,
        max_tokens: Some(256),
        temperature: None,
    };

    let model_key = "dfmodel";
    let plaintext = qoder_cosy::build_qoder_body(&ir, model_key, "system", model_key, 200_000);
    let wire_body = qoder_cosy::encode_body(&plaintext.to_string());

    let api = "https://gateway.qoder.com.cn";
    let url = format!("{api}{}", qoder_cosy::CHAT_PATH);
    let headers = qoder_cosy::cosy_headers(
        &url,
        &cred.uid,
        cred.display_name.as_deref().unwrap_or(""),
        cred.display_email.as_deref().unwrap_or(""),
        &token,
        &machine_id,
        &wire_body,
        true,
        Some(model_key),
        Some("system"),
    )
    .expect("cosy headers");

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async move {
        let client = reqwest::Client::new();
        let mut req = client
            .post(&url)
            .header("Accept", "text/event-stream")
            .body(wire_body);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let resp = req.send().await.expect("send");
        println!("HTTP {}", resp.status());
        let text = resp.text().await.unwrap_or_default();
        println!("--- raw body (first 3000 chars) ---");
        println!("{}", text.chars().take(3000).collect::<String>());
    });
}
