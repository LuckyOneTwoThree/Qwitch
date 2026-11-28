use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

use crate::adapter::external::{
    adapter_content_hash, is_valid_entry_args, is_valid_entry_command, resolve_exec_command,
    ALLOWED_PLUGIN_ENV_VARS,
};
use crate::domain::{
    resolve_vendor_wire_model_id, split_scoped_model_id, CredentialEnvelope, PoolMember,
    UpstreamModelOption, WireProtocolKind,
};
use crate::gateway::ir::{
    to_upstream_anthropic_body, to_upstream_openai_chat_body, to_upstream_openai_responses_body,
    IrRequest,
};
use crate::plugins::egress::EgressTransport;
use crate::plugins::manifest::{PluginEntry, PluginManifest};

const MAX_SUBPROCESS_STREAM_BODY: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SubprocessStreamOutput {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub auth_patch: Option<Value>,
}

pub struct SubprocessPluginHost {
    manifest: PluginManifest,
    plugin_dir: PathBuf,
    entry: PluginEntry,
    pinned_hash: String,
}

impl SubprocessPluginHost {
    pub fn new(
        manifest: PluginManifest,
        plugin_dir: PathBuf,
        pinned_hash: String,
    ) -> Result<Self, String> {
        let entry = manifest
            .entry
            .clone()
            .ok_or_else(|| format!("subprocess 插件 {} 缺少 entry", manifest.id))?;
        Ok(Self {
            manifest,
            plugin_dir,
            entry,
            pinned_hash,
        })
    }

    pub fn verify_integrity(&self) -> Result<(), String> {
        if !is_valid_entry_command(&self.plugin_dir, &self.entry.command)
            || !is_valid_entry_args(&self.plugin_dir, &self.entry.command, &self.entry.args)
        {
            return Err(format!(
                "外部插件 {} 入口校验失败，已拒绝执行",
                self.manifest.id
            ));
        }
        let actual_hash = adapter_content_hash(&self.plugin_dir)
            .map_err(|e| format!("外部插件 {} 完整性校验失败: {e}", self.manifest.id))?;
        if !actual_hash.eq_ignore_ascii_case(&self.pinned_hash) {
            return Err(format!(
                "外部插件 {} 内容哈希不一致（已被修改），已拒绝执行",
                self.manifest.id
            ));
        }
        Ok(())
    }

    fn spawn_command(&self) -> Command {
        let exec_cmd = resolve_exec_command(&self.plugin_dir, &self.entry.command);
        let mut cmd = Command::new(exec_cmd);
        cmd.args(&self.entry.args)
            .current_dir(&self.plugin_dir)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for key in ALLOWED_PLUGIN_ENV_VARS {
            if let Some(val) = std::env::var_os(key) {
                cmd.env(key, val);
            }
        }
        crate::platform::hide_console_tokio(&mut cmd);
        cmd
    }

    pub async fn discover_models(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<Vec<UpstreamModelOption>, String> {
        self.verify_integrity()?;
        let req_id = Uuid::new_v4().to_string();
        let req = json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "discover_models",
            "params": { "credential": cred }
        });
        let mut child = self
            .spawn_command()
            .spawn()
            .map_err(|e| format!("启动插件 {} 失败: {e}", self.manifest.id))?;

        let timeout = Duration::from_millis(self.entry.timeout_ms.clamp(1000, 60_000));
        let mut stdin = child.stdin.take();
        let stdout = child.stdout.take();

        let result = tokio::time::timeout(timeout, async {
            if let Some(mut s) = stdin.take() {
                let mut line = req.to_string().into_bytes();
                line.push(b'\n');
                let _ = s.write_all(&line).await;
                let _ = s.shutdown().await;
            }
            let Some(out) = stdout else {
                return Err("插件无标准输出".to_string());
            };
            let mut reader = BufReader::new(out).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let parsed: Value =
                    serde_json::from_str(trimmed).map_err(|e| format!("解析插件响应失败: {e}"))?;
                if parsed.get("id").and_then(Value::as_str) != Some(req_id.as_str()) {
                    continue;
                }
                if let Some(err) = parsed.get("error") {
                    let msg = err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("插件返回错误");
                    return Err(msg.to_string());
                }
                let models_val = parsed
                    .get("result")
                    .cloned()
                    .unwrap_or(Value::Array(vec![]));
                let models: Vec<UpstreamModelOption> = serde_json::from_value(models_val)
                    .map_err(|e| format!("解析模型列表失败: {e}"))?;
                return Ok(models);
            }
            Err(format!(
                "插件 {} 未返回 discover_models 响应",
                self.manifest.id
            ))
        })
        .await;

        let _ = child.wait().await;
        match result {
            Ok(inner) => inner,
            Err(_) => {
                let _ = child.start_kill();
                Err(format!("插件 {} discover_models 超时", self.manifest.id))
            }
        }
    }

    pub async fn invoke_stream_raw(
        &self,
        request_id: &str,
        cred: &CredentialEnvelope,
        protocol: WireProtocolKind,
        wire_model: &str,
        body: &Value,
        stream: bool,
    ) -> Result<SubprocessStreamOutput, String> {
        self.verify_integrity()?;
        let req = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "invoke_stream",
            "params": {
                "requestId": request_id,
                "credential": cred,
                "protocol": protocol,
                "wireModel": wire_model,
                "body": body,
                "stream": stream
            }
        });
        let mut child = self
            .spawn_command()
            .spawn()
            .map_err(|e| format!("启动插件 {} 失败: {e}", self.manifest.id))?;

        let timeout = Duration::from_millis(self.entry.timeout_ms.clamp(1000, 120_000));
        let mut stdin = child.stdin.take();
        let stdout = child.stdout.take();

        let res = tokio::time::timeout(timeout, async {
            if let Some(mut s) = stdin.take() {
                let mut line = req.to_string().into_bytes();
                line.push(b'\n');
                let _ = s.write_all(&line).await;
                let _ = s.shutdown().await;
            }
            let Some(out) = stdout else {
                return Err("插件无标准输出".to_string());
            };
            let mut status: u16 = 200;
            let mut headers: HashMap<String, String> = HashMap::new();
            let mut acc_body: Vec<u8> = Vec::new();
            let mut auth_patch: Option<Value> = None;

            let mut reader = BufReader::new(out).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(ev) = serde_json::from_str::<Value>(trimmed) else {
                    continue;
                };
                match ev.get("event").and_then(Value::as_str) {
                    Some("head") => {
                        if let Some(code) = ev.get("status").and_then(Value::as_u64) {
                            status = code as u16;
                        }
                        if let Some(obj) = ev.get("headers").and_then(Value::as_object) {
                            for (k, v) in obj {
                                if let Some(vs) = v.as_str() {
                                    headers.insert(k.to_ascii_lowercase(), vs.to_string());
                                }
                            }
                        }
                    }
                    Some("chunk") => {
                        if let Some(b64) = ev.get("dataBase64").and_then(Value::as_str) {
                            let bytes = BASE64
                                .decode(b64)
                                .map_err(|e| format!("解码插件 chunk 失败: {e}"))?;
                            if acc_body.len() + bytes.len() > MAX_SUBPROCESS_STREAM_BODY {
                                return Err("外部插件流式输出超过 64MB 上限".into());
                            }
                            acc_body.extend_from_slice(&bytes);
                        }
                    }
                    Some("auth_refreshed") => {
                        if let Some(patch) = ev.get("authPatch") {
                            auth_patch = Some(patch.clone());
                        }
                    }
                    Some("error") => {
                        let msg = ev
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("外部插件推理失败");
                        return Err(msg.to_string());
                    }
                    Some("done") => {
                        return Ok(SubprocessStreamOutput {
                            status,
                            headers,
                            body: acc_body,
                            auth_patch,
                        });
                    }
                    _ => {}
                }
            }
            Err(format!("外部插件 {} 未发送 done 结束帧", self.manifest.id))
        })
        .await;

        let _ = child.wait().await;
        match res {
            Ok(inner) => inner,
            Err(_) => {
                let _ = child.start_kill();
                Err(format!("外部插件 {} 执行超时", self.manifest.id))
            }
        }
    }
}

#[async_trait]
impl EgressTransport for SubprocessPluginHost {
    fn id(&self) -> &str {
        &self.manifest.id
    }

    async fn send_stream(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<reqwest::Response, String> {
        let (code, body_txt) = self
            .call_buffered(member, cred, ir_req, upstream_model)
            .await?;
        let status = reqwest::StatusCode::from_u16(code)
            .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        let content_type = if body_txt.contains("data:") {
            "text/event-stream; charset=utf-8"
        } else {
            "application/json"
        };
        Ok(crate::gateway::synthetic_response(
            status,
            content_type,
            body_txt.into_bytes(),
        ))
    }

    async fn call_buffered(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<(u16, String), String> {
        let (_, bare) = split_scoped_model_id(upstream_model);
        let wire_model = resolve_vendor_wire_model_id(&member.tool_id, bare);
        let profile = self.manifest.egress_profile.as_ref();
        let protocol = profile
            .and_then(|p| p.wire_protocols.first().copied())
            .unwrap_or(WireProtocolKind::OpenaiChat);
        let constraints = profile.map(|p| &p.constraints);
        let stream = constraints.is_some_and(|c| c.must_stream) || ir_req.stream;

        let body = match protocol {
            WireProtocolKind::AnthropicMessages => {
                to_upstream_anthropic_body(ir_req, &wire_model, stream)
            }
            WireProtocolKind::OpenaiResponses => {
                to_upstream_openai_responses_body(ir_req, &wire_model, stream, constraints)
            }
            WireProtocolKind::OpenaiChat => {
                to_upstream_openai_chat_body(ir_req, &wire_model, stream)
            }
        };

        let req_id = Uuid::new_v4().to_string();
        let out = self
            .invoke_stream_raw(&req_id, cred, protocol, &wire_model, &body, stream)
            .await?;
        Ok((out.status, String::from_utf8_lossy(&out.body).to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::CredentialPayload;
    use crate::plugins::manifest::{PluginBrand, PluginEntryMode, PluginKind, PluginTransport};

    fn has_node() -> bool {
        std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn dummy_cred(tool_id: &str) -> CredentialEnvelope {
        CredentialEnvelope {
            tool_id: tool_id.into(),
            uid: "u1".into(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "tok_test".into(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        }
    }

    #[tokio::test]
    async fn subprocess_host_discovers_models_streams_chunks_and_blocks_tampering() {
        if !has_node() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("qwitch_sub_host_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = r#"
import readline from 'node:readline';
const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on('line', (line) => {
  if (!line.trim()) return;
  const req = JSON.parse(line);
  if (req.method === 'discover_models') {
    process.stdout.write(JSON.stringify({
      jsonrpc: '2.0',
      id: req.id,
      result: [{
        id: 'gpt-5.4',
        displayName: 'GPT-5.4',
        vendorWireId: 'gpt-5.4',
        family: 'GPT',
        source: 'live'
      }]
    }) + '\n');
    process.exit(0);
  }
  if (req.method === 'invoke_stream') {
    process.stdout.write(JSON.stringify({ event: 'head', status: 200, headers: { 'content-type': 'text/event-stream' } }) + '\n');
    process.stdout.write(JSON.stringify({ event: 'chunk', dataBase64: Buffer.from('hello ').toString('base64') }) + '\n');
    process.stdout.write(JSON.stringify({ event: 'chunk', dataBase64: Buffer.from('codex').toString('base64') }) + '\n');
    process.stdout.write(JSON.stringify({ event: 'auth_refreshed', authPatch: { accessToken: 'new_tok' } }) + '\n');
    process.stdout.write(JSON.stringify({ event: 'done' }) + '\n');
    process.exit(0);
  }
});
"#;
        std::fs::write(dir.join("plugin.mjs"), script).unwrap();
        let manifest = PluginManifest {
            schema_version: 2,
            id: "codex_sub_test".into(),
            name: "Codex Test".into(),
            version: "1.0.0".into(),
            kind: PluginKind::Egress,
            transport: PluginTransport::Subprocess,
            platforms: Vec::new(),
            brand: PluginBrand::default(),
            entry: Some(PluginEntry {
                command: "node".into(),
                args: vec!["plugin.mjs".into()],
                mode: PluginEntryMode::Oneshot,
                timeout_ms: 10_000,
            }),
            egress_profile: None,
            ingress_profile: None,
        };
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();

        let hash = adapter_content_hash(&dir).unwrap();
        let host = SubprocessPluginHost::new(manifest, dir.clone(), hash).unwrap();
        let cred = dummy_cred("codex_sub_test");

        let models = host.discover_models(&cred).await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-5.4");

        let out = host
            .invoke_stream_raw(
                "r1",
                &cred,
                WireProtocolKind::OpenaiResponses,
                "gpt-5.4",
                &json!({"input":"hi"}),
                true,
            )
            .await
            .unwrap();
        assert_eq!(out.status, 200);
        assert_eq!(String::from_utf8_lossy(&out.body), "hello codex");
        assert_eq!(out.auth_patch.unwrap()["accessToken"], "new_tok");

        // 篡改脚本后必须拒绝执行
        std::fs::write(dir.join("plugin.mjs"), "console.log('pwned');").unwrap();
        let tampered_err = host.discover_models(&cred).await.unwrap_err();
        assert!(tampered_err.contains("哈希不一致"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
