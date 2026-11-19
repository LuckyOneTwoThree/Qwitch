use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use uuid::Uuid;

const MAX_ADAPTER_OUTPUT_BYTES: u64 = 1_048_576; // 1 MiB 上限
const MAX_ADAPTER_TIMEOUT_MS: u64 = 60_000; // 60s 上限

use crate::adapter::{
    AdapterCapabilities, AdapterError, AdapterManifest, ClaimResult, ToolAdapter,
};
use crate::domain::{AuthMethod, CredentialEnvelope, QuotaSnapshot, UnifiedErrorCode};
use crate::platform::PlatformHost;

#[derive(Debug, Clone, Deserialize)]
pub struct ExternalEntry {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExternalCapabilities {
    #[serde(default)]
    pub auth_methods: Vec<AuthMethod>,
    #[serde(default)]
    pub supports_quota_query: bool,
    #[serde(default)]
    pub supports_daily_claim: bool,
    #[serde(default)]
    pub supports_client_switch: bool,
    #[serde(default)]
    pub supports_multi_instance: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExternalManifestFile {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub platforms: Vec<String>,
    pub entry: ExternalEntry,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    pub capabilities: ExternalCapabilities,
}

fn default_timeout_ms() -> u64 {
    15_000
}

#[derive(Debug, Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'static str,
    id: String,
    method: &'a str,
    params: Value,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: Option<String>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    id: Option<Value>,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

pub struct ExternalAdapter {
    manifest: AdapterManifest,
    adapter_dir: PathBuf,
    entry: ExternalEntry,
    timeout_ms: u64,
}

impl ExternalAdapter {
    pub fn from_manifest_file(adapter_dir: PathBuf, raw: ExternalManifestFile) -> Self {
        Self {
            manifest: AdapterManifest {
                id: raw.id,
                name: raw.name,
                version: raw.version,
                is_builtin: false,
                capabilities: AdapterCapabilities {
                    auth_methods: raw.capabilities.auth_methods,
                    supports_quota_query: raw.capabilities.supports_quota_query,
                    supports_daily_claim: raw.capabilities.supports_daily_claim,
                    supports_client_switch: raw.capabilities.supports_client_switch,
                    supports_multi_instance: raw.capabilities.supports_multi_instance,
                },
            },
            adapter_dir,
            entry: raw.entry,
            timeout_ms: raw.timeout_ms.clamp(1000, MAX_ADAPTER_TIMEOUT_MS),
        }
    }

    pub(crate) fn current_platform_tag() -> &'static str {
        #[cfg(target_os = "macos")]
        {
            "macos"
        }
        #[cfg(target_os = "windows")]
        {
            "windows"
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            "linux"
        }
    }

    fn map_error_code(code: Option<&str>) -> UnifiedErrorCode {
        match code {
            Some("AUTH_EXPIRED") => UnifiedErrorCode::AuthExpired,
            Some("RISK_TRIGGERED") => UnifiedErrorCode::RiskTriggered,
            Some("RATE_LIMITED") => UnifiedErrorCode::RateLimited,
            Some("NETWORK_ERROR") => UnifiedErrorCode::NetworkError,
            Some("CLIENT_RUNNING") => UnifiedErrorCode::ClientRunning,
            Some("CLIENT_NOT_FOUND") => UnifiedErrorCode::ClientNotFound,
            Some("UNSUPPORTED_CAPABILITY") => UnifiedErrorCode::UnsupportedCapability,
            Some("INVALID_CREDENTIAL") => UnifiedErrorCode::InvalidCredential,
            _ => UnifiedErrorCode::InternalError,
        }
    }

    fn validate_envelope_tool_id(
        &self,
        env: CredentialEnvelope,
    ) -> Result<CredentialEnvelope, AdapterError> {
        if env.tool_id != self.manifest.id {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!(
                    "外部适配器 {} 返回的 tool_id ({}) 与插件 ID 不匹配，已拒绝导入",
                    self.manifest.id, env.tool_id
                ),
            });
        }
        Ok(env)
    }

    async fn call_rpc(&self, method: &str, params: Value) -> Result<Value, AdapterError> {
        let req_id = Uuid::new_v4().to_string();
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: req_id.clone(),
            method,
            params,
        };
        let mut req_line = serde_json::to_vec(&req).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("序列化 JSON-RPC 请求失败: {e}"),
        })?;
        req_line.push(b'\n');

        let mut child = Command::new(&self.entry.command)
            .args(&self.entry.args)
            .current_dir(&self.adapter_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("启动外部适配器 {} 失败: {e}", self.manifest.id),
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&req_line).await;
            let _ = stdin.shutdown().await;
        }

        let timeout = Duration::from_millis(self.timeout_ms);
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        let (stdout_bytes, stderr_bytes, exit_status) = tokio::time::timeout(timeout, async {
            let read_stdout = async {
                let mut buf = Vec::new();
                if let Some(stdout) = stdout_pipe {
                    let mut reader = stdout.take(MAX_ADAPTER_OUTPUT_BYTES + 1);
                    reader.read_to_end(&mut buf).await?;
                }
                Ok::<Vec<u8>, std::io::Error>(buf)
            };
            let read_stderr = async {
                let mut buf = Vec::new();
                if let Some(mut stderr) = stderr_pipe {
                    let mut chunk = [0u8; 4096];
                    loop {
                        match stderr.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if buf.len() < 65_536 {
                                    let take = n.min(65_536 - buf.len());
                                    buf.extend_from_slice(&chunk[..take]);
                                }
                            }
                        }
                    }
                }
                buf
            };
            let (out_res, err_buf, wait_res) = tokio::join!(read_stdout, read_stderr, child.wait());
            let out_buf = out_res?;
            let status = wait_res?;
            Ok::<(Vec<u8>, Vec<u8>, std::process::ExitStatus), std::io::Error>((
                out_buf, err_buf, status,
            ))
        })
        .await
        .map_err(|_| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!(
                "外部适配器 {} 执行超时 ({}ms)",
                self.manifest.id, self.timeout_ms
            ),
        })?
        .map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("外部适配器执行异常: {e}"),
        })?;

        if stdout_bytes.len() as u64 > MAX_ADAPTER_OUTPUT_BYTES {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("外部适配器 {} 输出超过 1MB 上限，已中止", self.manifest.id),
            });
        }

        let stderr_snippet = || -> String {
            let text = String::from_utf8_lossy(&stderr_bytes);
            let trimmed = text.trim();
            if trimmed.is_empty() {
                String::new()
            } else {
                let clipped: String = trimmed.chars().take(200).collect();
                format!("（stderr: {clipped}）")
            }
        };

        let stdout_str = String::from_utf8_lossy(&stdout_bytes);
        let Some(last_line) = stdout_str.lines().rev().find(|l| !l.trim().is_empty()) else {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!(
                    "外部适配器 {} 未返回 JSON-RPC 响应 (exit: {}){}",
                    self.manifest.id,
                    exit_status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "signal".to_string()),
                    stderr_snippet()
                ),
            });
        };

        let resp: JsonRpcResponse =
            serde_json::from_str(last_line).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("解析外部适配器响应失败: {e}{}", stderr_snippet()),
            })?;

        if let Some(resp_id) = resp.id.as_ref() {
            let matches = resp_id.as_str() == Some(req_id.as_str());
            if !matches {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: format!("外部适配器 {} 返回的 JSON-RPC id 不匹配", self.manifest.id),
                });
            }
        }

        if let Some(err) = resp.error {
            return Err(AdapterError::Unified {
                code: Self::map_error_code(err.code.as_deref()),
                message: err
                    .message
                    .unwrap_or_else(|| "外部适配器返回错误".to_string()),
            });
        }

        if !exit_status.success() && resp.result.is_none() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!(
                    "外部适配器 {} 异常退出 (code: {:?}){}",
                    self.manifest.id,
                    exit_status.code(),
                    stderr_snippet()
                ),
            });
        }

        Ok(resp.result.unwrap_or(Value::Null))
    }
}

#[async_trait]
impl ToolAdapter for ExternalAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    async fn import_from_local(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let config_dir = host
            .client_config_dir(&self.manifest.id)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let res = self
            .call_rpc(
                "import_local",
                serde_json::json!({ "client_config_dir": config_dir }),
            )
            .await?;
        let env: CredentialEnvelope =
            serde_json::from_value(res).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("外部适配器返回凭据格式无效: {e}"),
            })?;
        self.validate_envelope_tool_id(env)
    }

    async fn parse_manual_token(
        &self,
        raw_input: &str,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let res = self
            .call_rpc(
                "parse_manual_token",
                serde_json::json!({ "raw_input": raw_input }),
            )
            .await?;
        let env: CredentialEnvelope =
            serde_json::from_value(res).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("外部适配器解析 Token 失败: {e}"),
            })?;
        self.validate_envelope_tool_id(env)
    }

    async fn inspect_quota(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<QuotaSnapshot, AdapterError> {
        let res = self
            .call_rpc("inspect_quota", serde_json::json!({ "credential": cred }))
            .await?;
        serde_json::from_value(res).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("外部适配器返回额度格式无效: {e}"),
        })
    }

    async fn claim_daily(
        &self,
        cred: &CredentialEnvelope,
        precheck: Option<&QuotaSnapshot>,
    ) -> Result<ClaimResult, AdapterError> {
        let res = self
            .call_rpc(
                "claim_daily",
                serde_json::json!({ "credential": cred, "precheck": precheck }),
            )
            .await?;
        serde_json::from_value(res).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("外部适配器返回领取结果格式无效: {e}"),
        })
    }

    async fn apply_session(
        &self,
        host: &dyn PlatformHost,
        cred: &CredentialEnvelope,
    ) -> Result<(), AdapterError> {
        let config_dir = host
            .client_config_dir(&self.manifest.id)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        self.call_rpc(
            "apply_session",
            serde_json::json!({
                "credential": cred,
                "client_config_dir": config_dir
            }),
        )
        .await?;
        Ok(())
    }
}

/// 扫描 <qwitch_data_dir>/adapters/<adapter_id>/manifest.json（仅解析清单，不执行插件）
pub fn scan_external_manifests(data_dir: &Path) -> Vec<(PathBuf, ExternalManifestFile)> {
    let mut out = Vec::new();
    let adapters_root = data_dir.join("adapters");
    let _ = std::fs::create_dir_all(&adapters_root);

    let Ok(entries) = std::fs::read_dir(&adapters_root) else {
        return out;
    };

    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    for dir in dirs {
        let manifest_path = dir.join("manifest.json");
        if !manifest_path.is_file() {
            continue;
        }
        let Ok(bytes) = std::fs::read(&manifest_path) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_slice::<ExternalManifestFile>(&bytes) else {
            continue;
        };
        out.push((dir, parsed));
    }
    out
}

/// 加载适配当前平台的外部适配器（信任门禁由调用方 create_all_adapters 负责）
pub fn discover_external_adapters(data_dir: &Path) -> HashMap<String, Arc<dyn ToolAdapter>> {
    let current_os = ExternalAdapter::current_platform_tag();
    scan_external_manifests(data_dir)
        .into_iter()
        .filter(|(_, m)| m.platforms.is_empty() || m.platforms.iter().any(|p| p == current_os))
        .map(|(dir, m)| {
            let id = m.id.clone();
            let adapter: Arc<dyn ToolAdapter> =
                Arc::new(ExternalAdapter::from_manifest_file(dir, m));
            (id, adapter)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ext(id: &str, command: &str, args: Vec<String>) -> ExternalAdapter {
        ExternalAdapter::from_manifest_file(
            std::env::temp_dir(),
            ExternalManifestFile {
                id: id.to_string(),
                name: format!("Ext {id}"),
                version: "0.1.0".to_string(),
                platforms: vec![],
                entry: ExternalEntry {
                    command: command.to_string(),
                    args,
                },
                timeout_ms: 5000,
                capabilities: ExternalCapabilities {
                    auth_methods: vec![AuthMethod::TokenPaste],
                    supports_quota_query: true,
                    supports_daily_claim: false,
                    supports_client_switch: false,
                    supports_multi_instance: false,
                },
            },
        )
    }

    #[test]
    fn external_adapter_rejects_spoofed_tool_id() {
        let adapter = make_ext("my_plugin", "echo", vec![]);
        let spoofed = CredentialEnvelope {
            tool_id: "trae_cn".to_string(),
            uid: "u_1".to_string(),
            display_name: None,
            display_email: None,
            payload: crate::domain::CredentialPayload::Token {
                access_token: "tok".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };
        assert!(adapter.validate_envelope_tool_id(spoofed).is_err());

        let valid = CredentialEnvelope {
            tool_id: "my_plugin".to_string(),
            uid: "u_1".to_string(),
            display_name: None,
            display_email: None,
            payload: crate::domain::CredentialPayload::Token {
                access_token: "tok".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };
        assert!(adapter.validate_envelope_tool_id(valid).is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn call_rpc_handles_large_stderr_without_deadlock_and_checks_id() {
        // 1. 超过 100KB 的 stderr 与合法 JSON-RPC stdout 并发读取不死锁
        let script = r#"
            read req
            i=0
            while [ "$i" -lt 2000 ]; do
              printf 'stderr noise line padding 0123456789abcdefghijklmnopqrstuvwxyz\n' >&2
              i=$((i + 1))
            done
            id=$(printf '%s' "$req" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
            printf '{"jsonrpc":"2.0","id":"%s","result":{"ok":true}}\n' "$id"
        "#;
        let adapter = make_ext("my_plugin", "/bin/sh", vec!["-c".into(), script.into()]);
        let res = adapter
            .call_rpc("ping", serde_json::json!({}))
            .await
            .expect("large stderr must not deadlock");
        assert_eq!(res.get("ok").and_then(|v| v.as_bool()), Some(true));

        // 2. JSON-RPC id 不匹配拒绝
        let bad_id = make_ext(
            "my_plugin",
            "/bin/sh",
            vec![
                "-c".into(),
                r#"read _; printf '{"jsonrpc":"2.0","id":"wrong-id","result":{}}\n'"#.into(),
            ],
        );
        let err = bad_id
            .call_rpc("ping", serde_json::json!({}))
            .await
            .expect_err("id mismatch must fail");
        assert!(err.to_string().contains("id 不匹配"));

        // 3. 外部插件尝试返回内置 tool_id 时被 parse_manual_token 拦截
        let spoof = make_ext(
            "my_plugin",
            "/bin/sh",
            vec![
                "-c".into(),
                r#"read req; id=$(printf '%s' "$req" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p'); printf '{"jsonrpc":"2.0","id":"%s","result":{"tool_id":"trae_cn","uid":"u1","display_email":null,"payload":{"kind":"token","access_token":"t","refresh_token":null,"expires_at":null,"extra":{}}}}\n' "$id""#.into(),
            ],
        );
        let spoof_err = spoof
            .parse_manual_token("t")
            .await
            .expect_err("spoofed tool_id must fail");
        assert!(spoof_err.to_string().contains("tool_id"));
    }
}
