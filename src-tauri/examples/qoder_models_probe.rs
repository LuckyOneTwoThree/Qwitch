//! Qoder CN 模型目录探针：读本机 qwitch.db 的 qoder_cn 凭据（主密钥取自 Keychain），
//! 解密后调用模型列表接口，验证目录探测链路。
//!
//! ```bash
//! cargo run --example qoder_models_probe -- <account_label>
//! ```

use base64::{engine::general_purpose::STANDARD as B, Engine as _};
use qwitch_lib::adapter::qoder_cosy;
use qwitch_lib::domain::{CredentialEnvelope, CredentialPayload};
use qwitch_lib::storage::crypto;
use rusqlite::Connection;
use std::path::PathBuf;
fn main() {
    let label = std::env::args().nth(1).unwrap_or("主-01".into());
    let db = PathBuf::from(std::env::var("HOME").unwrap())
        .join("Library/Application Support/qwitch/qwitch.db");
    let conn = Connection::open(&db).unwrap();
    let (id,blob):(String,Vec<u8>)=conn.query_row("SELECT id,encrypted_credential FROM accounts WHERE tool_id='qoder_cn' AND label=?1 LIMIT 1",[&label],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
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
        .unwrap();
    let hex = String::from_utf8(out.stdout).unwrap();
    let hex = hex.trim();
    let mut mk = [0u8; 32];
    for i in 0..32 {
        mk[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    let aad = crypto::credential_aad("qoder_cn", &id);
    let cred: CredentialEnvelope = crypto::decrypt_credential(&mk, &aad, &blob).unwrap();
    let (token, machine_id) = match &cred.payload {
        CredentialPayload::LocalSnapshot { files } => {
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
    println!("token_len={} machine_id={}", token.len(), machine_id);
    let api = "https://gateway.qoder.com.cn";
    let url = format!("{api}{}", qoder_cosy::MODELS_PATH);
    let headers = qoder_cosy::cosy_headers(
        &url,
        &cred.uid,
        cred.display_name.as_deref().unwrap_or(""),
        cred.display_email.as_deref().unwrap_or(""),
        &token,
        &machine_id,
        "",
        false,
        None,
        None,
    )
    .unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async move {
        let client = reqwest::Client::new();
        let mut req = client.get(&url);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let resp = req.send().await.unwrap();
        println!("HTTP {}", resp.status());
        let t = resp.text().await.unwrap_or_default();
        println!("{}", t.chars().take(800).collect::<String>());
    });
}
