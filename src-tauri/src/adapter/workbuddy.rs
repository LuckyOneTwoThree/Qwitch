use async_trait::async_trait;
use base64::{
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD as BASE64_URL},
    Engine as _,
};
use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::adapter::common::{map_http_status, shared_http_client};
use crate::adapter::{
    AdapterCapabilities, AdapterError, AdapterManifest, ClaimResult, ToolAdapter,
};
use crate::domain::{
    AuthMethod, ClaimStatus, CredentialEnvelope, CredentialPayload, QuotaBucket, QuotaSnapshot,
    UnifiedErrorCode,
};
use crate::platform::PlatformHost;

const DEFAULT_ENDPOINT: &str = "https://copilot.tencent.com";
const WEB_AUTH_ORIGIN: &str = "https://www.workbuddy.cn";
const AUTH_FILENAME: &str = "workbuddy-desktop.info";
const DECRYPTED_TOKEN_FILE: &str = "decrypted_token.txt";
/// accessToken 距离过期不足 10 分钟时提前续期
const REFRESH_SKEW_MS: i64 = 10 * 60 * 1000;

const AUTH_HELPER_JS: &str = r#"'use strict';
const crypto = require('crypto');
const inputLimit = 65536;
const failure = reason => { throw {reason}; };
const object = x => x !== null && typeof x === 'object' && !Array.isArray(x);
function base64(value, length) {
  if (typeof value !== 'string' || value.length > inputLimit) failure('INVALID_FORMAT');
  const bytes = Buffer.from(value, 'base64');
  if (bytes.toString('base64') !== value || (length !== undefined && bytes.length !== length)) failure('INVALID_FORMAT');
  return bytes;
}
function utf8(bytes) {
  const text = bytes.toString('utf8');
  if (!Buffer.from(text, 'utf8').equals(bytes)) failure('INVALID_FORMAT');
  return text;
}
function decode(value) {
  if (!object(value) || Object.keys(value).sort().join(',') !== '$wbEncrypted,envelope' || value.$wbEncrypted !== 1)
    failure('UNSUPPORTED_ENVELOPE');
  let envelope;
  try { envelope = JSON.parse(utf8(base64(value.envelope))); } catch (e) { failure(e.reason || 'INVALID_FORMAT'); }
  if (!object(envelope) || !Number.isInteger(envelope.suite) || envelope.suite !== 1) failure('UNSUPPORTED_ENVELOPE');
  if (Object.keys(envelope).sort().join(',') !== 'authTag,ciphertext,keyId,nonce,suite' ||
      typeof envelope.keyId !== 'string' || !/^[0-9a-f]{16}$/.test(envelope.keyId)) failure('INVALID_FORMAT');
  return {
    keyId: envelope.keyId,
    nonce: base64(envelope.nonce, 12),
    tag: base64(envelope.authTag, 16),
    ciphertext: base64(envelope.ciphertext),
  };
}
function nativeStorage() {
  try {
    const storage = process._linkedBinding('electron_browser_workbuddy_storage');
    if (typeof storage.loggerGet !== 'function') failure('RUNTIME_UNAVAILABLE');
    return storage;
  } catch (_) { failure('RUNTIME_UNAVAILABLE'); }
}
function decrypt(envelope) {
  let payload;
  try { payload = JSON.parse(nativeStorage().loggerGet()); } catch (_) { failure('RUNTIME_UNAVAILABLE'); }
  let key, plaintext;
  try {
    if (!object(payload) || payload.version !== 1) failure('RUNTIME_UNAVAILABLE');
    const secret = base64(payload.atRestSecretKey, 32);
    if (secret.every(b => b === 0)) failure('RUNTIME_UNAVAILABLE');
    secret.fill(0);
    key = crypto.createHash('sha256').update(payload.atRestSecretKey, 'utf8').digest();
    payload = null;
    if (crypto.createHash('sha256').update(key).digest('hex').slice(0, 16) !== envelope.keyId) failure('KEY_MISMATCH');
    const lp = s => {
      const bytes = Buffer.from(s, 'utf8');
      const len = Buffer.alloc(4); len.writeUInt32BE(bytes.length);
      return Buffer.concat([len, bytes]);
    };
    const aad = Buffer.concat([Buffer.from('WB-AAD\0', 'ascii'), Buffer.from([1]), lp('WBEV1'), lp('sym-v1'), Buffer.from([0,0,0,1]), lp(envelope.keyId), Buffer.from([2,0,0])]);
    try {
      const cipher = crypto.createDecipheriv('aes-256-gcm', key, envelope.nonce, {authTagLength: 16});
      cipher.setAAD(aad); cipher.setAuthTag(envelope.tag);
      plaintext = Buffer.concat([cipher.update(envelope.ciphertext), cipher.final()]);
    } catch (_) { failure('DECRYPT_FAILED'); }
    return utf8(plaintext);
  } finally {
    if (key) key.fill(0);
    if (plaintext) plaintext.fill(0);
  }
}
let chunks = [], size = 0, replied = false;
function reply(v) { if (replied) return; replied = true; process.stdout.write(JSON.stringify({version: 1, ...v}), () => process.exit(v.ok ? 0 : 1)); }
process.stdin.on('data', c => { size += c.length; if (size > inputLimit) reply({ok: false, reason: 'INVALID_FORMAT'}); else chunks.push(c); });
process.stdin.on('end', () => {
  try {
    const req = JSON.parse(utf8(Buffer.concat(chunks)));
    if (!object(req) || req.version !== 1) failure('HELPER_PROTOCOL');
    if (req.operation === 'decrypt') {
      reply({ok: true, accessToken: decrypt(decode(req.value))});
    } else if (req.operation === 'decrypt_batch' && object(req.fields)) {
      const values = {};
      for (const [k, v] of Object.entries(req.fields)) {
        if (typeof v === 'string') {
          values[k] = v;
        } else if (object(v) && v.$wbEncrypted === 1) {
          try { values[k] = decrypt(decode(v)); } catch (_) {}
        }
      }
      reply({ok: true, values});
    } else {
      failure('HELPER_PROTOCOL');
    }
  } catch (e) {
    reply({ok: false, reason: e.reason || 'HELPER_PROTOCOL'});
  }
});"#;

fn decode_jwt_claims(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?.trim();
    if payload.is_empty() || parts.next().is_none() {
        return None;
    }
    let unpadded = payload.trim_end_matches('=');
    let bytes = BASE64_URL.decode(unpadded).ok()?;
    serde_json::from_slice(&bytes).ok().filter(Value::is_object)
}

pub struct WorkBuddyAdapter {
    manifest: AdapterManifest,
    http: reqwest::Client,
    /// 注入的平台宿主。为 None 时回退到 `NativePlatformHost`。
    ///
    /// 历史实现直接在 `inspect_quota` / `claim_daily` 内自造
    /// `NativePlatformHost`，导致多实例 profile 导入使用的 `ScopedPlatformHost`
    /// 无法影响这两条路径（读取的是真实本机配置而非隔离 profile）。
    host: Option<Arc<dyn PlatformHost>>,
    #[cfg(test)]
    api_base_override: Option<String>,
}

struct WorkBuddyRefreshContext {
    endpoint: String,
    refresh_token: String,
    expires_at_ms: Option<i64>,
    domain: String,
    enterprise_id: Option<String>,
}

impl WorkBuddyAdapter {
    /// 注入平台宿主（用于多实例 profile 等隔离场景）。
    pub fn with_host(host: Arc<dyn PlatformHost>) -> Self {
        let mut adapter = Self::new();
        adapter.host = Some(host);
        adapter
    }

    #[cfg(test)]
    pub(crate) fn with_api_base(base: impl Into<String>) -> Self {
        let mut adapter = Self::new();
        adapter.api_base_override = Some(base.into().trim_end_matches('/').to_string());
        adapter
    }

    /// 优先使用注入的宿主，否则回退到本机实现。
    fn host_or_native(&self) -> Arc<dyn PlatformHost> {
        self.host
            .clone()
            .unwrap_or_else(|| Arc::new(crate::platform::NativePlatformHost))
    }

    pub fn new() -> Self {
        let http = shared_http_client().clone();

        Self {
            host: None,
            #[cfg(test)]
            api_base_override: None,
            manifest: AdapterManifest {
                id: "workbuddy".to_string(),
                name: "WorkBuddy".to_string(),
                version: "0.1.0".to_string(),
                is_builtin: true,
                capabilities: AdapterCapabilities {
                    auth_methods: vec![
                        AuthMethod::LocalImport,
                        AuthMethod::WebviewOauth,
                        AuthMethod::TokenPaste,
                    ],
                    supports_quota_query: true,
                    supports_daily_claim: true,
                    supports_client_switch: true,
                    supports_multi_instance: false,
                },
            },
            http,
        }
    }

    fn parse_session_json(raw_bytes: &[u8]) -> Result<Value, AdapterError> {
        let val: Value = serde_json::from_slice(raw_bytes).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: format!("WorkBuddy 凭据不是合法 JSON: {e}"),
        })?;

        let uid = val
            .pointer("/account/uid")
            .and_then(crate::adapter::common::json_uid_to_string);
        let access_token = val.pointer("/auth/accessToken");

        if uid.is_none() || access_token.is_none() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "WorkBuddy 会话缺少 account.uid 或 auth.accessToken".into(),
            });
        }
        Ok(val)
    }

    fn resolve_runtime_binary(host: &dyn PlatformHost) -> Option<PathBuf> {
        let detected = host.detect_client_executable("workbuddy")?;
        #[cfg(target_os = "macos")]
        {
            if detected.extension().and_then(|e| e.to_str()) == Some("app") || detected.is_dir() {
                let macos_dir = detected.join("Contents/MacOS");
                // 仅接受已知的官方可执行文件名。
                //
                // 历史实现在此有「取 Contents/MacOS 下第一个普通文件」的兜底，
                // 会把同一目录内的任意二进制（可能来自自定义客户端路径，或被
                // 替换/植入的文件）当作 Electron 运行时执行，并经由 stdin 喂入
                // 明文凭据。该兜底已移除：解析失败一律返回 None，由调用方报
                // CLIENT_NOT_FOUND 并提示用户在设置中修正客户端路径。
                for name in ["WorkBuddy", "Electron"] {
                    let candidate = macos_dir.join(name);
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
                return None;
            }
        }
        if detected.is_file() {
            Some(detected)
        } else {
            None
        }
    }

    pub fn validate_endpoint(raw_url: &str) -> Result<String, AdapterError> {
        let trimmed = raw_url.trim();
        let parsed = reqwest::Url::parse(trimmed).map_err(|_| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "WorkBuddy 端点 URL 格式无效".into(),
        })?;
        if parsed.scheme() != "https"
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.port().is_some_and(|p| p != 443)
            || !matches!(parsed.path(), "" | "/")
        {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "WorkBuddy 端点必须为无路径与查询参数的标准 HTTPS 地址".into(),
            });
        }
        let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
        let allowed = host == "copilot.tencent.com"
            || host == "www.workbuddy.cn"
            || host == "workbuddy.cn"
            || host.ends_with(".workbuddy.cn")
            || host.ends_with(".copilot.tencent.com");
        if !allowed {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "WorkBuddy 端点不在受信任的官方 HTTPS 白名单内".into(),
            });
        }
        Ok(trimmed.trim_end_matches('/').to_string())
    }

    async fn decrypt_sym_v1_batch(
        runtime_bin: &Path,
        fields: &serde_json::Map<String, Value>,
    ) -> Result<HashMap<String, String>, AdapterError> {
        let req_json = serde_json::json!({
            "version": 1,
            "operation": "decrypt_batch",
            "fields": fields,
        });
        let req_bytes = serde_json::to_vec(&req_json).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: e.to_string(),
        })?;

        let mut cmd = Command::new(runtime_bin);
        cmd.args(["-e", AUTH_HELPER_JS])
            .env("ELECTRON_RUN_AS_NODE", "1")
            .env_remove("NODE_OPTIONS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(target_os = "windows")]
        cmd.creation_flags(0x0800_0000);

        let mut child = cmd.spawn().map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::ClientNotFound,
            message: format!("无法启动 WorkBuddy 运行时解密凭据: {e}"),
        })?;

        let mut stdin_pipe = child.stdin.take();
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        let stdout_bytes = match tokio::time::timeout(Duration::from_secs(10), async {
            let write_stdin = async {
                if let Some(mut stdin) = stdin_pipe.take() {
                    let _ = stdin.write_all(&req_bytes).await;
                    let _ = stdin.shutdown().await;
                }
            };
            let read_stdout = async {
                let mut buf = Vec::new();
                if let Some(stdout) = stdout_pipe {
                    let mut reader = stdout.take(256 * 1024);
                    reader.read_to_end(&mut buf).await?;
                }
                Ok::<Vec<u8>, std::io::Error>(buf)
            };
            let read_stderr = async {
                let mut chunk = [0u8; 4096];
                if let Some(mut stderr) = stderr_pipe {
                    while let Ok(n) = stderr.read(&mut chunk).await {
                        if n == 0 {
                            break;
                        }
                    }
                }
            };
            let (_, out_res, _, wait_res) =
                tokio::join!(write_stdin, read_stdout, read_stderr, child.wait());
            let out_buf = out_res?;
            let _status = wait_res?;
            Ok::<Vec<u8>, std::io::Error>(out_buf)
        })
        .await
        {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(e)) => {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: format!("等待 WorkBuddy 解密助手失败: {e}"),
                });
            }
            Err(_) => {
                let _ = child.start_kill();
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: "WorkBuddy 解密助手执行超时".into(),
                });
            }
        };

        let reply: Value =
            serde_json::from_slice(&stdout_bytes).map_err(|_| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "WorkBuddy 解密助手返回无效输出".into(),
            })?;

        if reply.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            if let Some(obj) = reply.get("values").and_then(|v| v.as_object()) {
                let mut out = HashMap::new();
                for (k, v) in obj {
                    if let Some(s) = v.as_str() {
                        out.insert(k.clone(), s.to_string());
                    }
                }
                return Ok(out);
            }
        }

        let reason = reply
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("DECRYPT_FAILED");
        Err(AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: format!("WorkBuddy 加密凭据解密失败 ({reason})"),
        })
    }

    async fn decrypt_sym_v1(
        runtime_bin: &Path,
        encrypted_val: &Value,
    ) -> Result<String, AdapterError> {
        let mut fields = serde_json::Map::new();
        fields.insert("accessToken".to_string(), encrypted_val.clone());
        let mut out = Self::decrypt_sym_v1_batch(runtime_bin, &fields).await?;
        out.remove("accessToken")
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "WorkBuddy 加密凭据解密失败 (DECRYPT_FAILED)".into(),
            })
    }

    fn parse_claim_response_body(
        status: reqwest::StatusCode,
        raw_text: &str,
    ) -> Result<Value, AdapterError> {
        if status.is_success() && raw_text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(raw_text).map_err(|e| {
            if !status.is_success() {
                map_http_status(status, "WorkBuddy 每日签到")
            } else {
                AdapterError::Unified {
                    code: UnifiedErrorCode::NetworkError,
                    message: format!("解析 WorkBuddy 签到响应失败: {e}"),
                }
            }
        })
    }

    /// 已签幂等契约（ADAPTER_SPEC）：2xx null body 直接算已签；
    /// 400/409 必须携带 code 10001 或已签文案；其余状态码一律不算
    fn checkin_claim_idempotent(status: u16, body: &Value) -> bool {
        let ok_http = (200..300).contains(&status);
        let idempotent_http = status == 400 || status == 409;
        if body.is_null() {
            return ok_http;
        }
        if !(ok_http || idempotent_http) {
            return false;
        }
        if body.get("code").and_then(|v| v.as_i64()) == Some(10001) {
            return true;
        }
        let msg = body
            .get("msg")
            .or_else(|| body.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        crate::adapter::common::is_idempotent_claimed_phrase(msg)
    }

    fn format_display_identity(
        nickname: Option<&str>,
        phone: Option<&str>,
        email: Option<&str>,
    ) -> (Option<String>, Option<String>) {
        let clean_nick = nickname.map(str::trim).filter(|s| !s.is_empty());
        let clean_phone = phone.map(str::trim).filter(|s| !s.is_empty());
        let clean_email = email.map(str::trim).filter(|s| !s.is_empty());

        let display_name = match (clean_nick, clean_phone) {
            (Some(n), Some(p)) if n.chars().count() <= 1 || n == "~" => Some(format!(
                "WorkBuddy ({})",
                crate::storage::crypto::mask_phone(p)
            )),
            (Some(n), _) => Some(n.to_string()),
            (None, Some(p)) => Some(format!(
                "WorkBuddy ({})",
                crate::storage::crypto::mask_phone(p)
            )),
            (None, None) => None,
        };
        let display_email = clean_email
            .map(|s| s.to_string())
            .or_else(|| clean_phone.map(|s| s.to_string()));
        (display_name, display_email)
    }

    pub async fn create_oauth_session() -> Result<(String, String), String> {
        let http = shared_http_client();

        let url = format!("{WEB_AUTH_ORIGIN}/v2/plugin/auth/state?platform=workbuddy");
        let resp = http
            .post(&url)
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/json")
            .header(USER_AGENT, "WorkBuddy")
            .header("X-No-Authorization", "true")
            .header("X-No-User-Id", "true")
            .header("X-No-Enterprise-Id", "true")
            .header("X-No-Department-Info", "true")
            .body("{}")
            .send()
            .await
            .map_err(|e| format!("请求 WorkBuddy 授权状态失败: {}", e.without_url()))?;

        if !resp.status().is_success() {
            return Err(format!(
                "WorkBuddy 授权状态接口返回 HTTP {}",
                resp.status().as_u16()
            ));
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("解析 WorkBuddy 授权状态失败: {e}"))?;
        let data = dig_data(&body);
        let state = data
            .get("state")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| "WorkBuddy 授权状态缺少 state".to_string())?
            .to_string();
        let auth_url = data
            .get("authUrl")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{WEB_AUTH_ORIGIN}/login?platform=workbuddy&state={state}"));

        Ok((state, auth_url))
    }

    pub async fn poll_oauth_result(
        state_token: &str,
    ) -> Result<Option<CredentialEnvelope>, String> {
        let http = shared_http_client();

        let token_url = reqwest::Url::parse_with_params(
            &format!("{WEB_AUTH_ORIGIN}/v2/plugin/auth/token"),
            &[("state", state_token)],
        )
        .map_err(|e| format!("构造 WorkBuddy Token 轮询 URL 失败: {e}"))?;
        let resp = http
            .get(token_url)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, "WorkBuddy")
            .header("X-No-Authorization", "true")
            .header("X-No-User-Id", "true")
            .header("X-No-Enterprise-Id", "true")
            .header("X-No-Department-Info", "true")
            .send()
            .await
            .map_err(|e| format!("轮询 WorkBuddy Token 失败: {}", e.without_url()))?;

        let status = resp.status();
        if !status.is_success() {
            if matches!(status.as_u16(), 401 | 403 | 429) || status.is_server_error() {
                return Err(format!(
                    "轮询 WorkBuddy Token 异常：HTTP {}",
                    status.as_u16()
                ));
            }
            return Ok(None);
        }

        let body: Value = resp.json().await.unwrap_or(Value::Null);
        let code = body.get("code").and_then(|v| v.as_i64());
        if code == Some(11217) {
            return Ok(None);
        }

        let token_obj = dig_data(&body);
        let Some(access_token) = token_obj
            .get("accessToken")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        else {
            return Ok(None);
        };

        let mut account_val = Value::Null;
        let acc_url = reqwest::Url::parse_with_params(
            &format!("{WEB_AUTH_ORIGIN}/v2/plugin/login/account"),
            &[("state", state_token)],
        )
        .map_err(|e| format!("构造 WorkBuddy 账号查询 URL 失败: {e}"))?;
        for _ in 0..4 {
            if let Ok(acc_resp) = http
                .get(acc_url.clone())
                .header(ACCEPT, "application/json")
                .header(USER_AGENT, "WorkBuddy")
                .header(AUTHORIZATION, format!("Bearer {access_token}"))
                .header("X-No-User-Id", "true")
                .header("X-No-Enterprise-Id", "true")
                .header("X-No-Department-Info", "true")
                .send()
                .await
            {
                if acc_resp.status().is_success() {
                    let acc_body: Value = acc_resp.json().await.unwrap_or(Value::Null);
                    if acc_body.get("code").and_then(|v| v.as_i64()) == Some(12151) {
                        tokio::time::sleep(Duration::from_millis(450)).await;
                        continue;
                    }
                    let d = dig_data(&acc_body);
                    if d.get("uid")
                        .and_then(crate::adapter::common::json_uid_to_string)
                        .is_some()
                    {
                        account_val = d.clone();
                        break;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(350)).await;
        }

        if account_val.is_null() {
            // 兜底仅允许唯一账号列表；多账号场景禁止取第一个以免绑定错账号
            if let Ok(cons_resp) = http
                .get(format!("{WEB_AUTH_ORIGIN}/console/accounts"))
                .header(ACCEPT, "application/json")
                .header(USER_AGENT, "WorkBuddy")
                .header(AUTHORIZATION, format!("Bearer {access_token}"))
                .send()
                .await
            {
                if cons_resp.status().is_success() {
                    if let Ok(cons_body) = cons_resp.json::<Value>().await {
                        let accounts = cons_body
                            .pointer("/data/accounts")
                            .or_else(|| cons_body.pointer("/accounts"))
                            .and_then(|v| v.as_array());
                        if let Some(list) = accounts {
                            if list.len() == 1 {
                                account_val = list[0].clone();
                            }
                        }
                    }
                }
            }
        }

        if account_val.is_null() {
            return Err(
                "WorkBuddy 授权完成但无法唯一确认账号身份（接口未返回或存在多个账号），请重试或改用本机导入"
                    .to_string(),
            );
        }

        let uid = account_val
            .get("uid")
            .and_then(crate::adapter::common::json_uid_to_string)
            .ok_or_else(|| "WorkBuddy 授权成功但未获取到账号 UID".to_string())?;

        let nick = account_val
            .get("nickname")
            .or_else(|| account_val.get("nickName"))
            .and_then(|v| v.as_str());
        let phone = account_val
            .get("phoneNumber")
            .or_else(|| account_val.get("phone_number"))
            .and_then(|v| v.as_str());
        let email = account_val.get("email").and_then(|v| v.as_str());
        let (display_name, display_email) = Self::format_display_identity(nick, phone, email);

        let mut auth_json = token_obj.clone();
        if let Some(obj) = auth_json.as_object_mut() {
            obj.entry("endpoint".to_string())
                .or_insert_with(|| Value::String(DEFAULT_ENDPOINT.to_string()));
        }

        let session_obj = serde_json::json!({
            "account": account_val,
            "auth": auth_json,
            "accounts": [account_val],
            "allAccounts": [account_val]
        });
        let sess_bytes = serde_json::to_vec_pretty(&session_obj)
            .map_err(|e| format!("序列化 WorkBuddy 会话失败: {e}"))?;

        let mut files = HashMap::new();
        files.insert(AUTH_FILENAME.to_string(), BASE64.encode(sess_bytes));
        files.insert(
            DECRYPTED_TOKEN_FILE.to_string(),
            BASE64.encode(access_token.as_bytes()),
        );
        if let Some(rt) = token_obj
            .get("refreshToken")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            files.insert(
                "decrypted_refresh_token.txt".to_string(),
                BASE64.encode(rt.as_bytes()),
            );
        }

        Ok(Some(CredentialEnvelope {
            tool_id: "workbuddy".to_string(),
            uid,
            display_name,
            display_email,
            payload: CredentialPayload::LocalSnapshot { files },
        }))
    }

    async fn extract_api_context(
        cred: &CredentialEnvelope,
        host: Option<&dyn PlatformHost>,
    ) -> Result<(String, String, HeaderMap), AdapterError> {
        match &cred.payload {
            CredentialPayload::LocalSnapshot { files } => {
                let b64 = files
                    .get(AUTH_FILENAME)
                    .ok_or_else(|| AdapterError::Unified {
                        code: UnifiedErrorCode::InvalidCredential,
                        message: "快照中缺少 workbuddy-desktop.info".into(),
                    })?;
                let raw = BASE64.decode(b64).map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: format!("快照 Base64 解码失败: {e}"),
                })?;
                let sess = Self::parse_session_json(&raw)?;
                let uid = sess
                    .pointer("/account/uid")
                    .and_then(crate::adapter::common::json_uid_to_string)
                    .unwrap_or_else(|| cred.uid.clone());
                let raw_endpoint = sess
                    .pointer("/auth/endpoint")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(DEFAULT_ENDPOINT);
                let endpoint = Self::validate_endpoint(raw_endpoint)?;

                let cached_plain_token = files
                    .get(DECRYPTED_TOKEN_FILE)
                    .and_then(|b64| BASE64.decode(b64).ok())
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());

                let token_val = &sess["auth"]["accessToken"];
                let access_token = if let Some(s) = token_val.as_str() {
                    s.to_string()
                } else if let Some(tok) = cached_plain_token {
                    tok
                } else if token_val.get("$wbEncrypted").and_then(|v| v.as_i64()) == Some(1) {
                    let h = host.ok_or_else(|| AdapterError::Unified {
                        code: UnifiedErrorCode::InvalidCredential,
                        message: "加密凭据需要本机 WorkBuddy 运行时解密".into(),
                    })?;
                    let bin =
                        Self::resolve_runtime_binary(h).ok_or_else(|| AdapterError::Unified {
                            code: UnifiedErrorCode::ClientNotFound,
                            message: "未找到本机 WorkBuddy 客户端，无法解密 sym-v1 凭据".into(),
                        })?;
                    Self::decrypt_sym_v1(&bin, token_val).await?
                } else {
                    return Err(AdapterError::Unified {
                        code: UnifiedErrorCode::InvalidCredential,
                        message: "不支持的 WorkBuddy 凭据格式".into(),
                    });
                };

                let headers = Self::build_headers(
                    &access_token,
                    &uid,
                    sess.pointer("/account/enterpriseId")
                        .and_then(|v| v.as_str()),
                    sess.pointer("/auth/domain").and_then(|v| v.as_str()),
                )?;
                Ok((endpoint, uid, headers))
            }
            CredentialPayload::Token {
                access_token,
                extra,
                ..
            } => {
                let raw_endpoint = extra
                    .get("endpoint")
                    .map(|s| s.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(DEFAULT_ENDPOINT);
                let endpoint = Self::validate_endpoint(raw_endpoint)?;
                let headers = Self::build_headers(
                    access_token,
                    &cred.uid,
                    extra.get("enterprise_id").map(|s| s.as_str()),
                    extra.get("domain").map(|s| s.as_str()),
                )?;
                Ok((endpoint, cred.uid.clone(), headers))
            }
            CredentialPayload::Cookies { .. } => Err(AdapterError::Unified {
                code: UnifiedErrorCode::UnsupportedCapability,
                message: "WorkBuddy 不支持 Cookie 凭据模式".into(),
            }),
        }
    }

    fn build_headers(
        access_token: &str,
        uid: &str,
        enterprise_id: Option<&str>,
        domain: Option<&str>,
    ) -> Result<HeaderMap, AdapterError> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(USER_AGENT, HeaderValue::from_static("WorkBuddy"));
        let auth = HeaderValue::from_str(&format!("Bearer {access_token}")).map_err(|_| {
            AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "Token 包含非法请求头字符".into(),
            }
        })?;
        headers.insert(AUTHORIZATION, auth);
        if !uid.trim().is_empty() && !CredentialEnvelope::is_fallback_uid(uid) {
            if let Ok(v) = HeaderValue::from_str(uid) {
                headers.insert("X-User-Id", v);
            }
        }
        if let Some(eid) = enterprise_id.filter(|s| !s.is_empty()) {
            if let Ok(v) = HeaderValue::from_str(eid) {
                headers.insert("X-Enterprise-Id", v.clone());
                headers.insert("X-Tenant-Id", v);
            }
        }
        if let Some(dom) = domain.filter(|s| !s.is_empty()) {
            if let Ok(v) = HeaderValue::from_str(dom) {
                headers.insert("X-Domain", v);
            }
        }
        Ok(headers)
    }

    fn parse_checkin_status_from_body(
        body: &Value,
    ) -> Result<(ClaimStatus, f64, i64), AdapterError> {
        if let Some(code) = body.get("code").and_then(|v| v.as_i64()) {
            if code != 0 {
                let msg = body
                    .get("msg")
                    .or_else(|| body.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("查询签到状态失败");
                let lower = msg.to_lowercase();
                if code == 401
                    || lower.contains("unauthorized")
                    || lower.contains("token")
                    || lower.contains("expired")
                {
                    return Err(AdapterError::Unified {
                        code: UnifiedErrorCode::AuthExpired,
                        message: format!("WorkBuddy 登录态已失效 ({code}): {msg}"),
                    });
                }
                if code == 403 {
                    return Err(AdapterError::Unified {
                        code: UnifiedErrorCode::RiskTriggered,
                        message: format!("WorkBuddy 拒绝访问 ({code}): {msg}"),
                    });
                }
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::NetworkError,
                    message: format!("查询 WorkBuddy 签到状态异常 ({code}): {msg}"),
                });
            }
        }

        let data = dig_data(body);
        let claim_status = match data.get("today_checked_in") {
            Some(v) if v.as_bool() == Some(true) || v.as_i64() == Some(1) => ClaimStatus::Claimed,
            Some(v) if v.as_bool() == Some(false) || v.as_i64() == Some(0) => {
                ClaimStatus::Available
            }
            _ => ClaimStatus::Unknown,
        };
        let checkin_total_credits = parse_json_f64(data.get("total_credits")).unwrap_or(0.0);
        let streak_days = data
            .get("streak_days")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        Ok((claim_status, checkin_total_credits, streak_days))
    }

    async fn fetch_quota_and_checkin(
        &self,
        endpoint: &str,
        headers: &HeaderMap,
    ) -> Result<QuotaSnapshot, AdapterError> {
        let checkin_url = format!("{endpoint}/v2/billing/meter/checkin-activity-status");
        let resp = self
            .http
            .post(&checkin_url)
            .headers(headers.clone())
            .body("{}")
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("查询 WorkBuddy 签到状态失败: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            return Err(map_http_status(status, "查询 WorkBuddy 状态"));
        }

        let body: Value = resp.json().await.map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("解析 WorkBuddy 状态响应失败: {e}"),
        })?;

        if let Some(code) = body.get("code").and_then(|v| v.as_i64()) {
            if let Some(err) =
                crate::adapter::common::map_business_code(code, "查询 WorkBuddy 状态")
            {
                return Err(err);
            }
        }

        let (claim_status, checkin_total_credits, streak_days) =
            Self::parse_checkin_status_from_body(&body)?;

        let mut plan_name: Option<String> = None;
        let mut buckets: Vec<QuotaBucket> = Vec::new();

        let summary_url = format!("{endpoint}/billing/meter/get-user-resource-summary");
        if let Ok(sum_resp) = self
            .http
            .post(&summary_url)
            .headers(headers.clone())
            .body("{}")
            .send()
            .await
        {
            let sum_status = sum_resp.status();
            if matches!(sum_status.as_u16(), 401 | 403 | 429) {
                return Err(map_http_status(sum_status, "查询 WorkBuddy 额度汇总"));
            }
            if sum_status.is_success() {
                if let Ok(sum_body) = sum_resp.json::<Value>().await {
                    if let Some(code) = sum_body.get("code").and_then(|v| v.as_i64()) {
                        if let Some(err) = crate::adapter::common::map_business_code(
                            code,
                            "查询 WorkBuddy 额度汇总",
                        ) {
                            return Err(err);
                        }
                    }
                    let sdata = dig_data(&sum_body);
                    plan_name = sdata
                        .get("SubscriptionPackageName")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());

                    let mut cycle_total = 0.0_f64;
                    let mut cycle_remain = 0.0_f64;
                    let mut cycle_used = 0.0_f64;

                    if let Some(pkgs) = sdata.get("Packages").and_then(|v| v.as_array()) {
                        for pkg in pkgs {
                            cycle_total +=
                                parse_json_f64(pkg.get("CycleTotalCapacity")).unwrap_or(0.0);
                            cycle_remain +=
                                parse_json_f64(pkg.get("CycleRemainCapacity")).unwrap_or(0.0);
                            cycle_used +=
                                parse_json_f64(pkg.get("CycleUsedCapacity")).unwrap_or(0.0);
                        }
                    }

                    if let Some(total_credits_str) = parse_json_f64(sdata.get("TotalCredits")) {
                        if cycle_remain == 0.0 && total_credits_str > 0.0 {
                            cycle_remain = total_credits_str;
                        }
                        if cycle_total < cycle_remain {
                            cycle_total = cycle_remain + cycle_used;
                        }
                    }

                    if cycle_total > 0.0 || cycle_remain > 0.0 {
                        buckets.push(QuotaBucket {
                            key: "credits".into(),
                            label: "可用积分".into(),
                            used: cycle_used,
                            total: cycle_total.max(cycle_remain),
                            remaining: cycle_remain,
                            unit: "积分".into(),
                            reset_at: None,
                        });
                    }
                }
            }
        }

        if buckets.is_empty() || checkin_total_credits > 0.0 {
            let label = if streak_days > 0 {
                format!("签到累计 (连签 {streak_days} 天)")
            } else {
                "签到累计积分".to_string()
            };
            buckets.push(QuotaBucket {
                key: "checkin_credits".into(),
                label,
                used: 0.0,
                total: checkin_total_credits,
                remaining: checkin_total_credits,
                unit: "积分".into(),
                reset_at: None,
            });
        }

        Ok(QuotaSnapshot {
            plan_name,
            claim_status,
            buckets,
            fetched_at: Utc::now().to_rfc3339(),
        })
    }

    fn jwt_exp_ms(claims: &Value) -> Option<i64> {
        let exp = claims.get("exp").and_then(|v| v.as_i64())?;
        Some(if exp < 10_000_000_000 {
            exp.saturating_mul(1000)
        } else {
            exp
        })
    }

    fn extract_refresh_context(cred: &CredentialEnvelope) -> Option<WorkBuddyRefreshContext> {
        match &cred.payload {
            CredentialPayload::Token {
                access_token,
                refresh_token,
                expires_at,
                extra,
            } => {
                let rt = refresh_token
                    .as_ref()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())?;
                let raw_ep = extra
                    .get("endpoint")
                    .map(String::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(DEFAULT_ENDPOINT);
                let endpoint =
                    Self::validate_endpoint(raw_ep).unwrap_or_else(|_| DEFAULT_ENDPOINT.into());
                let domain = extra
                    .get("domain")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        reqwest::Url::parse(&endpoint)
                            .ok()
                            .and_then(|u| u.host_str().map(str::to_string))
                    })
                    .unwrap_or_else(|| "copilot.tencent.com".to_string());
                let exp_ms = expires_at
                    .map(|e| {
                        if e < 10_000_000_000 {
                            e.saturating_mul(1000)
                        } else {
                            e
                        }
                    })
                    .or_else(|| decode_jwt_claims(access_token).and_then(|c| Self::jwt_exp_ms(&c)));
                let enterprise_id = extra
                    .get("enterprise_id")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                Some(WorkBuddyRefreshContext {
                    endpoint,
                    refresh_token: rt,
                    expires_at_ms: exp_ms,
                    domain,
                    enterprise_id,
                })
            }
            CredentialPayload::LocalSnapshot { files } => {
                let rt = files
                    .get("decrypted_refresh_token.txt")
                    .and_then(|b64| BASE64.decode(b64).ok())
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())?;
                let exp_ms = files
                    .get(DECRYPTED_TOKEN_FILE)
                    .and_then(|b64| BASE64.decode(b64).ok())
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .and_then(|tok| decode_jwt_claims(tok.trim()))
                    .and_then(|c| Self::jwt_exp_ms(&c));
                let sess = files
                    .get(AUTH_FILENAME)
                    .and_then(|b64| BASE64.decode(b64).ok())
                    .and_then(|raw| Self::parse_session_json(&raw).ok());
                let raw_ep = sess
                    .as_ref()
                    .and_then(|s| s.pointer("/auth/endpoint").and_then(|v| v.as_str()))
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(DEFAULT_ENDPOINT);
                let endpoint =
                    Self::validate_endpoint(raw_ep).unwrap_or_else(|_| DEFAULT_ENDPOINT.into());
                let domain = sess
                    .as_ref()
                    .and_then(|s| s.pointer("/auth/domain").and_then(|v| v.as_str()))
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        reqwest::Url::parse(&endpoint)
                            .ok()
                            .and_then(|u| u.host_str().map(str::to_string))
                    })
                    .unwrap_or_else(|| "copilot.tencent.com".to_string());
                let enterprise_id = sess
                    .as_ref()
                    .and_then(|s| s.pointer("/account/enterpriseId").and_then(|v| v.as_str()))
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                Some(WorkBuddyRefreshContext {
                    endpoint,
                    refresh_token: rt,
                    expires_at_ms: exp_ms,
                    domain,
                    enterprise_id,
                })
            }
            CredentialPayload::Cookies { .. } => None,
        }
    }

    async fn refresh_credential_inner(
        &self,
        cred: &CredentialEnvelope,
        force: bool,
    ) -> Result<Option<CredentialEnvelope>, AdapterError> {
        let Some(WorkBuddyRefreshContext {
            endpoint,
            refresh_token: rt,
            expires_at_ms: exp_ms,
            domain,
            enterprise_id,
        }) = Self::extract_refresh_context(cred)
        else {
            return Ok(None);
        };
        let now_ms = Utc::now().timestamp_millis();
        if !force {
            let Some(exp) = exp_ms else {
                return Ok(None);
            };
            if exp - now_ms >= REFRESH_SKEW_MS {
                return Ok(None);
            }
        }

        #[cfg(test)]
        let base = self.api_base_override.as_deref().unwrap_or(&endpoint);
        #[cfg(not(test))]
        let base = endpoint.as_str();
        let url = format!("{base}/v2/plugin/auth/token/refresh");

        let mut req = self
            .http
            .post(&url)
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/json")
            .header(USER_AGENT, "WorkBuddy")
            .header("X-Refresh-Token", rt.as_str())
            .header("X-Auth-Refresh-Source", "plugin")
            .header("X-Domain", domain.as_str());
        if let Some(eid) = enterprise_id.as_deref() {
            req = req
                .header("X-Enterprise-Id", eid)
                .header("X-Tenant-Id", eid);
        }
        let resp = req
            .body("{}")
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("刷新 WorkBuddy 登录态失败: {}", e.without_url()),
            })?;

        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                message: "WorkBuddy 刷新令牌已失效，请重新登录或导入本机登录态".into(),
            });
        }
        if !status.is_success() {
            return Err(map_http_status(status, "刷新 WorkBuddy 登录态"));
        }

        let body: Value = resp.json().await.map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("解析 WorkBuddy 刷新响应失败: {e}"),
        })?;
        if let Some(code) = body.get("code").and_then(|v| v.as_i64()) {
            if code != 0 {
                let msg = body
                    .get("msg")
                    .or_else(|| body.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("WorkBuddy 刷新被拒绝");
                if matches!(code, 401 | 403 | 10002 | 10003)
                    || msg.to_ascii_lowercase().contains("expired")
                    || msg.to_ascii_lowercase().contains("invalid")
                {
                    return Err(AdapterError::Unified {
                        code: UnifiedErrorCode::AuthExpired,
                        message: format!("WorkBuddy 登录态已失效: {msg}"),
                    });
                }
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::NetworkError,
                    message: format!("刷新 WorkBuddy 登录态失败: {msg}"),
                });
            }
        }

        let data = dig_data(&body);
        let access_token = data
            .get("accessToken")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "WorkBuddy 刷新响应缺少 accessToken".into(),
            })?
            .to_string();
        let new_refresh = data
            .get("refreshToken")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(rt.as_str())
            .to_string();
        let new_expires_at = data
            .get("expiresIn")
            .and_then(|v| v.as_i64())
            .filter(|&s| s > 0)
            .map(|s| now_ms.saturating_add(s.saturating_mul(1000)))
            .or_else(|| decode_jwt_claims(&access_token).and_then(|c| Self::jwt_exp_ms(&c)));

        let mut updated = cred.clone();
        match &mut updated.payload {
            CredentialPayload::Token {
                access_token: at,
                refresh_token: rt_slot,
                expires_at,
                ..
            } => {
                *at = access_token;
                *rt_slot = Some(new_refresh);
                *expires_at = new_expires_at;
            }
            CredentialPayload::LocalSnapshot { files } => {
                files.insert(
                    DECRYPTED_TOKEN_FILE.to_string(),
                    BASE64.encode(access_token.as_bytes()),
                );
                files.insert(
                    "decrypted_refresh_token.txt".to_string(),
                    BASE64.encode(new_refresh.as_bytes()),
                );
                if let Some(b64) = files.get(AUTH_FILENAME).cloned() {
                    if let Ok(raw) = BASE64.decode(b64) {
                        if let Ok(mut sess) = Self::parse_session_json(&raw) {
                            if let Some(auth_obj) =
                                sess.get_mut("auth").and_then(|v| v.as_object_mut())
                            {
                                if auth_obj.get("accessToken").is_some_and(|v| v.is_string()) {
                                    auth_obj.insert(
                                        "accessToken".into(),
                                        Value::String(access_token.clone()),
                                    );
                                }
                                if auth_obj.get("refreshToken").is_some_and(|v| v.is_string()) {
                                    auth_obj.insert(
                                        "refreshToken".into(),
                                        Value::String(new_refresh.clone()),
                                    );
                                }
                                if let Ok(bytes) = serde_json::to_vec(&sess) {
                                    files.insert(AUTH_FILENAME.to_string(), BASE64.encode(bytes));
                                }
                            }
                        }
                    }
                }
            }
            CredentialPayload::Cookies { .. } => return Ok(None),
        }
        Ok(Some(updated))
    }
}

impl Default for WorkBuddyAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_json_f64(val: Option<&Value>) -> Option<f64> {
    let v = val?;
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

fn dig_data(val: &Value) -> &Value {
    for k in ["data", "result", "resp", "response"] {
        if let Some(inner) = val.get(k).filter(|v| v.is_object()) {
            return inner;
        }
    }
    val
}

#[async_trait]
impl ToolAdapter for WorkBuddyAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    fn detect_local_active_uid(&self, host: &dyn PlatformHost) -> Option<String> {
        let auth_dir = host.client_config_dir("workbuddy").ok()?;
        let bytes = std::fs::read(auth_dir.join(AUTH_FILENAME)).ok()?;
        let sess = Self::parse_session_json(&bytes).ok()?;
        let uid = sess
            .pointer("/account/uid")
            .and_then(crate::adapter::common::json_uid_to_string)?;
        if CredentialEnvelope::is_fallback_uid(&uid) {
            None
        } else {
            Some(uid)
        }
    }

    async fn import_from_local(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let auth_dir = host
            .client_config_dir("workbuddy")
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: e.to_string(),
            })?;
        let auth_file = auth_dir.join(AUTH_FILENAME);
        if !auth_file.is_file() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::ClientNotFound,
                message: format!("未找到 WorkBuddy 本机登录凭据: {}", auth_file.display()),
            });
        }

        let mut raw_bytes = None;
        for attempt in 0..3 {
            match std::fs::read(&auth_file) {
                Ok(bytes) => {
                    raw_bytes = Some(bytes);
                    break;
                }
                Err(_) if attempt < 2 => {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                Err(e) => {
                    return Err(AdapterError::Unified {
                        code: UnifiedErrorCode::InternalError,
                        message: format!("读取 WorkBuddy 凭据文件失败: {e}"),
                    });
                }
            }
        }

        let bytes = raw_bytes.unwrap_or_default();
        let sess = Self::parse_session_json(&bytes)?;
        let uid = sess
            .pointer("/account/uid")
            .and_then(crate::adapter::common::json_uid_to_string)
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "WorkBuddy 会话缺少有效的 account.uid".into(),
            })?;

        let mut files = HashMap::new();
        files.insert(AUTH_FILENAME.to_string(), BASE64.encode(&bytes));

        let mut decrypted_fields = HashMap::new();
        if let Some(bin) = Self::resolve_runtime_binary(host) {
            let mut req_fields = serde_json::Map::new();
            if let Some(v) = sess.pointer("/auth/accessToken") {
                req_fields.insert("accessToken".to_string(), v.clone());
            }
            if let Some(v) = sess.pointer("/auth/refreshToken") {
                req_fields.insert("refreshToken".to_string(), v.clone());
            }
            if let Some(v) = sess
                .pointer("/account/nickname")
                .or_else(|| sess.pointer("/account/nickName"))
                .or_else(|| sess.pointer("/account/name"))
            {
                req_fields.insert("nickname".to_string(), v.clone());
            }
            if let Some(v) = sess
                .pointer("/account/phoneNumber")
                .or_else(|| sess.pointer("/account/phone_number"))
            {
                req_fields.insert("phoneNumber".to_string(), v.clone());
            }
            if let Some(v) = sess.pointer("/account/email") {
                req_fields.insert("email".to_string(), v.clone());
            }
            if let Ok(vals) = Self::decrypt_sym_v1_batch(&bin, &req_fields).await {
                decrypted_fields = vals;
            }
        }

        if let Some(tok) = decrypted_fields
            .get("accessToken")
            .map(String::as_str)
            .or_else(|| sess.pointer("/auth/accessToken").and_then(|v| v.as_str()))
            .filter(|s| !s.trim().is_empty())
        {
            files.insert(
                DECRYPTED_TOKEN_FILE.to_string(),
                BASE64.encode(tok.as_bytes()),
            );
        }
        if let Some(rt) = decrypted_fields
            .get("refreshToken")
            .filter(|s| !s.trim().is_empty())
        {
            files.insert(
                "decrypted_refresh_token.txt".to_string(),
                BASE64.encode(rt.as_bytes()),
            );
        }

        let plain_nick = decrypted_fields
            .get("nickname")
            .map(String::as_str)
            .or_else(|| {
                sess.pointer("/account/nickName")
                    .or_else(|| sess.pointer("/account/nickname"))
                    .or_else(|| sess.pointer("/account/name"))
                    .or_else(|| sess.pointer("/account/username"))
                    .and_then(|v| v.as_str())
            });
        let plain_phone = decrypted_fields
            .get("phoneNumber")
            .map(String::as_str)
            .or_else(|| {
                sess.pointer("/account/phoneNumber")
                    .and_then(|v| v.as_str())
            });
        let plain_email = decrypted_fields
            .get("email")
            .map(String::as_str)
            .or_else(|| sess.pointer("/account/email").and_then(|v| v.as_str()));

        let (name, email) = Self::format_display_identity(plain_nick, plain_phone, plain_email);

        Ok(CredentialEnvelope {
            tool_id: "workbuddy".to_string(),
            uid,
            display_name: name,
            display_email: email,
            payload: CredentialPayload::LocalSnapshot { files },
        })
    }

    async fn parse_manual_token(
        &self,
        raw_input: &str,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let trimmed = raw_input.trim();
        if trimmed.is_empty() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "输入凭据不能为空".into(),
            });
        }

        if trimmed.starts_with('{') {
            if let Ok(sess) = Self::parse_session_json(trimmed.as_bytes()) {
                let uid = sess
                    .pointer("/account/uid")
                    .and_then(crate::adapter::common::json_uid_to_string)
                    .ok_or_else(|| AdapterError::Unified {
                        code: UnifiedErrorCode::InvalidCredential,
                        message: "WorkBuddy 会话缺少有效的 account.uid".into(),
                    })?;
                let nick = sess
                    .pointer("/account/nickName")
                    .or_else(|| sess.pointer("/account/nickname"))
                    .or_else(|| sess.pointer("/account/name"))
                    .and_then(|v| v.as_str());
                let phone = sess
                    .pointer("/account/phoneNumber")
                    .or_else(|| sess.pointer("/account/phone_number"))
                    .and_then(|v| v.as_str());
                let email = sess.pointer("/account/email").and_then(|v| v.as_str());
                let (name, masked_contact) = Self::format_display_identity(nick, phone, email);

                let mut files = HashMap::new();
                files.insert(AUTH_FILENAME.to_string(), BASE64.encode(trimmed.as_bytes()));
                if let Some(tok) = sess
                    .pointer("/auth/accessToken")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    files.insert(
                        DECRYPTED_TOKEN_FILE.to_string(),
                        BASE64.encode(tok.as_bytes()),
                    );
                }
                if let Some(rt) = sess
                    .pointer("/auth/refreshToken")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    files.insert(
                        "decrypted_refresh_token.txt".to_string(),
                        BASE64.encode(rt.as_bytes()),
                    );
                }
                return Ok(CredentialEnvelope {
                    tool_id: "workbuddy".to_string(),
                    uid,
                    display_name: name,
                    display_email: masked_contact,
                    payload: CredentialPayload::LocalSnapshot { files },
                });
            }

            let val: Value = serde_json::from_str(trimmed).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("WorkBuddy JSON 格式无效: {e}"),
            })?;
            let token = val
                .get("token")
                .or_else(|| val.get("accessToken"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "JSON 中缺少 token/accessToken 字段".into(),
                })?;
            let jwt_claims = decode_jwt_claims(token);
            let uid = val
                .get("userId")
                .or_else(|| val.get("uid"))
                .and_then(crate::adapter::common::json_uid_to_string)
                .or_else(|| {
                    jwt_claims.as_ref().and_then(|c| {
                        c.get("userId")
                            .or_else(|| c.get("uid"))
                            .or_else(|| c.get("user_id"))
                            .or_else(|| c.get("sub"))
                            .and_then(crate::adapter::common::json_uid_to_string)
                    })
                })
                .unwrap_or_else(|| {
                    crate::adapter::common::hash_fallback_uid("wb_oauth", token.as_bytes())
                });
            let email = val
                .get("email")
                .and_then(|v| v.as_str())
                .or_else(|| {
                    jwt_claims
                        .as_ref()
                        .and_then(|c| c.get("email").and_then(|v| v.as_str()))
                })
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let name = val
                .get("name")
                .or_else(|| val.get("nickName"))
                .or_else(|| val.get("nickname"))
                .and_then(|v| v.as_str())
                .or_else(|| {
                    jwt_claims.as_ref().and_then(|c| {
                        c.get("name")
                            .or_else(|| c.get("nickname"))
                            .or_else(|| c.get("nickName"))
                            .and_then(|v| v.as_str())
                    })
                })
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let mut extra = HashMap::new();
            if let Some(ent) = val
                .get("enterpriseId")
                .or_else(|| val.get("enterprise_id"))
                .or_else(|| val.get("tenantId"))
                .and_then(crate::adapter::common::json_uid_to_string)
                .or_else(|| {
                    jwt_claims.as_ref().and_then(|c| {
                        c.get("enterpriseId")
                            .or_else(|| c.get("enterprise_id"))
                            .or_else(|| c.get("tenantId"))
                            .and_then(crate::adapter::common::json_uid_to_string)
                    })
                })
            {
                extra.insert("enterprise_id".to_string(), ent);
            }
            if let Some(dom) = val
                .get("domain")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                extra.insert("domain".to_string(), dom.to_string());
            }
            if let Some(ep) = val
                .get("endpoint")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                extra.insert("endpoint".to_string(), Self::validate_endpoint(ep)?);
            }
            return Ok(CredentialEnvelope {
                tool_id: "workbuddy".to_string(),
                uid,
                display_name: name,
                display_email: email,
                payload: CredentialPayload::Token {
                    access_token: token.to_string(),
                    refresh_token: val
                        .get("refreshToken")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    expires_at: jwt_claims.as_ref().and_then(Self::jwt_exp_ms),
                    extra,
                },
            });
        }

        let clean_token = trimmed.strip_prefix("Bearer ").unwrap_or(trimmed).trim();
        if clean_token.is_empty() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "输入凭据不能为空".into(),
            });
        }

        let jwt_claims = decode_jwt_claims(clean_token);
        let jwt_uid = jwt_claims.as_ref().and_then(|c| {
            c.get("userId")
                .or_else(|| c.get("uid"))
                .or_else(|| c.get("user_id"))
                .or_else(|| c.get("sub"))
                .and_then(crate::adapter::common::json_uid_to_string)
        });
        let jwt_email = jwt_claims
            .as_ref()
            .and_then(|c| c.get("email").and_then(|v| v.as_str()))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let jwt_name = jwt_claims
            .as_ref()
            .and_then(|c| {
                c.get("name")
                    .or_else(|| c.get("nickname"))
                    .or_else(|| c.get("nickName"))
                    .and_then(|v| v.as_str())
            })
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let jwt_enterprise = jwt_claims.as_ref().and_then(|c| {
            c.get("enterpriseId")
                .or_else(|| c.get("enterprise_id"))
                .or_else(|| c.get("tenantId"))
                .and_then(crate::adapter::common::json_uid_to_string)
        });

        #[cfg(not(test))]
        let fallback_acc: Option<Value> = if jwt_uid.is_none() {
            match shared_http_client()
                .get(format!("{WEB_AUTH_ORIGIN}/console/accounts"))
                .timeout(Duration::from_secs(3))
                .header(ACCEPT, "application/json")
                .header(USER_AGENT, "WorkBuddy")
                .header(AUTHORIZATION, format!("Bearer {clean_token}"))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    resp.json::<Value>().await.ok().and_then(|cons_body| {
                        cons_body
                            .pointer("/data/accounts")
                            .or_else(|| cons_body.pointer("/accounts"))
                            .and_then(|v| v.as_array())
                            .filter(|list| list.len() == 1)
                            .map(|list| list[0].clone())
                    })
                }
                _ => None,
            }
        } else {
            None
        };
        #[cfg(test)]
        let fallback_acc: Option<Value> = None;

        let uid = jwt_uid.or_else(|| {
            fallback_acc
                .as_ref()
                .and_then(|acc| acc.get("uid"))
                .and_then(crate::adapter::common::json_uid_to_string)
        });
        let email = jwt_email.or_else(|| {
            fallback_acc
                .as_ref()
                .and_then(|acc| acc.get("email").and_then(|v| v.as_str()))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        });
        let name = jwt_name.or_else(|| {
            fallback_acc
                .as_ref()
                .and_then(|acc| {
                    acc.get("nickname")
                        .or_else(|| acc.get("nickName"))
                        .and_then(|v| v.as_str())
                })
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        });
        let mut extra = HashMap::new();
        if let Some(ent) = jwt_enterprise.or_else(|| {
            fallback_acc
                .as_ref()
                .and_then(|acc| acc.get("enterpriseId").or_else(|| acc.get("tenantId")))
                .and_then(crate::adapter::common::json_uid_to_string)
        }) {
            extra.insert("enterprise_id".to_string(), ent);
        }

        Ok(CredentialEnvelope {
            tool_id: "workbuddy".to_string(),
            uid: uid.unwrap_or_else(|| {
                crate::adapter::common::hash_fallback_uid("wb_manual", clean_token.as_bytes())
            }),
            display_name: name,
            display_email: email,
            payload: CredentialPayload::Token {
                access_token: clean_token.to_string(),
                refresh_token: None,
                expires_at: jwt_claims.as_ref().and_then(Self::jwt_exp_ms),
                extra,
            },
        })
    }

    async fn inspect_quota(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<QuotaSnapshot, AdapterError> {
        let host = self.host_or_native();
        let (endpoint, _uid, headers) = Self::extract_api_context(cred, Some(&*host)).await?;
        self.fetch_quota_and_checkin(&endpoint, &headers).await
    }

    async fn claim_daily(
        &self,
        cred: &CredentialEnvelope,
        precheck: Option<&QuotaSnapshot>,
    ) -> Result<ClaimResult, AdapterError> {
        let host = self.host_or_native();
        let (endpoint, _uid, headers) = Self::extract_api_context(cred, Some(&*host)).await?;

        // 复用调度器前置快照，避免重复查状态
        let current = match precheck {
            Some(q) => q.clone(),
            None => self.fetch_quota_and_checkin(&endpoint, &headers).await?,
        };
        if current.claim_status == ClaimStatus::Claimed {
            return Ok(ClaimResult {
                claimed: false,
                reward_summary: "今日已签过".into(),
                updated_quota: Some(current),
            });
        }
        if current.claim_status != ClaimStatus::Available {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: "WorkBuddy 签到状态未知，为避免重复请求已中止领取".into(),
            });
        }

        let url = format!("{endpoint}/v2/billing/meter/daily-checkin");
        let resp = self
            .http
            .post(&url)
            .headers(headers.clone())
            .body("{}")
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("WorkBuddy 每日签到请求失败: {e}"),
            })?;

        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(map_http_status(status, "WorkBuddy 每日签到"));
        }

        let raw_text = resp.text().await.map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("读取 WorkBuddy 签到响应失败: {e}"),
        })?;
        // 契约（ADAPTER_SPEC）：仅 2xx 下空响应体或字面 "null" 按 null 处理；
        // 非 JSON（如 HTML/WAF 错误页）必须失败而非误判"已签"
        let body = Self::parse_claim_response_body(status, &raw_text)?;

        let code = body.get("code").and_then(|v| v.as_i64());

        // 业务码优先：401/403/429 无论 HTTP 状态如何都必须映射为可熔断错误
        if let Some(c) = code {
            if let Some(err) = crate::adapter::common::map_business_code(c, "WorkBuddy 每日签到")
            {
                return Err(err);
            }
        }

        // 幂等已签识别必须先于通用 HTTP 错误：HTTP 400 + code 10001 是契约内的"今日已签"
        if Self::checkin_claim_idempotent(status.as_u16(), &body) {
            let mut refreshed = self
                .fetch_quota_and_checkin(&endpoint, &headers)
                .await
                .unwrap_or_else(|_| QuotaSnapshot {
                    plan_name: current.plan_name.clone(),
                    claim_status: ClaimStatus::Claimed,
                    buckets: current.buckets.clone(),
                    fetched_at: Utc::now().to_rfc3339(),
                });
            refreshed.claim_status = ClaimStatus::Claimed;
            return Ok(ClaimResult {
                claimed: false,
                reward_summary: "今日已签过".into(),
                updated_quota: Some(refreshed),
            });
        }

        if !status.is_success() {
            return Err(map_http_status(status, "WorkBuddy 每日签到"));
        }

        if let Some(c) = code.filter(|&c| c != 0) {
            let msg = body
                .get("msg")
                .or_else(|| body.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("签到未成功");
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("WorkBuddy 每日签到返回异常 ({c}): {msg}"),
            });
        }

        let data = dig_data(&body);
        let credit_opt = data.get("credit").and_then(|v| v.as_i64());
        if code != Some(0) && credit_opt.is_none() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: "WorkBuddy 每日签到响应缺少有效结果字段，请刷新核实状态".into(),
            });
        }
        let mut refreshed = self
            .fetch_quota_and_checkin(&endpoint, &headers)
            .await
            .unwrap_or_else(|_| QuotaSnapshot {
                plan_name: current.plan_name.clone(),
                claim_status: ClaimStatus::Claimed,
                buckets: current.buckets.clone(),
                fetched_at: Utc::now().to_rfc3339(),
            });
        if matches!(
            refreshed.claim_status,
            ClaimStatus::Unknown | ClaimStatus::Unsupported
        ) {
            refreshed.claim_status = ClaimStatus::Claimed;
        }
        let reward_summary = match credit_opt {
            Some(c) => format!("成功领取 {c} 积分"),
            None => "每日签到已完成".to_string(),
        };

        Ok(ClaimResult {
            claimed: true,
            reward_summary,
            updated_quota: Some(refreshed),
        })
    }

    async fn refresh_credential(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<Option<CredentialEnvelope>, AdapterError> {
        self.refresh_credential_inner(cred, false).await
    }

    async fn force_refresh_credential(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<Option<CredentialEnvelope>, AdapterError> {
        self.refresh_credential_inner(cred, true).await
    }

    async fn fetch_account_profile(
        &self,
        cred: &CredentialEnvelope,
    ) -> Option<(Option<String>, Option<String>)> {
        let (endpoint, uid, headers) = Self::extract_api_context(cred, None).await.ok()?;
        let resp = self
            .http
            .get(format!("{endpoint}/console/accounts"))
            .headers(headers)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body: Value = resp.json().await.ok()?;
        let list = body
            .pointer("/data/accounts")
            .or_else(|| body.pointer("/accounts"))
            .and_then(|v| v.as_array())?;
        let target = list
            .iter()
            .find(|acc| {
                acc.get("uid")
                    .and_then(crate::adapter::common::json_uid_to_string)
                    .as_deref()
                    == Some(uid.as_str())
            })
            .or_else(|| (list.len() == 1).then(|| &list[0]))?;
        let nick = target
            .get("nickname")
            .or_else(|| target.get("nickName"))
            .and_then(|v| v.as_str());
        let phone = target
            .get("phoneNumber")
            .or_else(|| target.get("phone_number"))
            .and_then(|v| v.as_str());
        let email = target.get("email").and_then(|v| v.as_str());
        let (display_name, display_email) = Self::format_display_identity(nick, phone, email);
        (display_name.is_some() || display_email.is_some()).then_some((display_name, display_email))
    }

    async fn apply_session(
        &self,
        host: &dyn PlatformHost,
        cred: &CredentialEnvelope,
    ) -> Result<(), AdapterError> {
        let auth_dir = host
            .client_config_dir("workbuddy")
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: e.to_string(),
            })?;
        std::fs::create_dir_all(&auth_dir).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("创建 WorkBuddy 配置目录失败: {e}"),
        })?;
        let target = auth_dir.join(AUTH_FILENAME);

        let bytes: Vec<u8> = match &cred.payload {
            CredentialPayload::LocalSnapshot { files } => {
                let b64 = files
                    .get(AUTH_FILENAME)
                    .ok_or_else(|| AdapterError::Unified {
                        code: UnifiedErrorCode::InvalidCredential,
                        message: "快照中缺少 workbuddy-desktop.info".into(),
                    })?;
                BASE64.decode(b64).map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: format!("快照解码失败: {e}"),
                })?
            }
            CredentialPayload::Token {
                access_token,
                refresh_token,
                extra,
                ..
            } => {
                let raw_endpoint = extra
                    .get("endpoint")
                    .map(|s| s.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(DEFAULT_ENDPOINT);
                let endpoint = Self::validate_endpoint(raw_endpoint)?;

                // 不得原地修改上一账号的 workbuddy-desktop.info，必须全新构建目标账号会话，
                // 避免残留旧账号的 enterpriseId / refreshToken / accounts 导致串号
                let mut account_map = serde_json::Map::new();
                account_map.insert("uid".to_string(), Value::String(cred.uid.clone()));
                if let Some(email) = cred.display_email.as_ref().filter(|s| !s.trim().is_empty()) {
                    account_map.insert("email".to_string(), Value::String(email.clone()));
                }
                if let Some(name) = cred.display_name.as_ref().filter(|s| !s.trim().is_empty()) {
                    account_map.insert("nickName".to_string(), Value::String(name.clone()));
                }
                let enterprise_id = extra
                    .get("enterprise_id")
                    .or_else(|| extra.get("enterpriseId"))
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty());
                if let Some(eid) = enterprise_id {
                    account_map.insert("enterpriseId".to_string(), Value::String(eid.to_string()));
                }
                let account_val = Value::Object(account_map);

                let mut auth_map = serde_json::Map::new();
                auth_map.insert(
                    "accessToken".to_string(),
                    Value::String(access_token.clone()),
                );
                auth_map.insert("endpoint".to_string(), Value::String(endpoint));
                if let Some(eid) = enterprise_id {
                    auth_map.insert("enterpriseId".to_string(), Value::String(eid.to_string()));
                }
                if let Some(domain) = extra
                    .get("domain")
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                {
                    auth_map.insert("domain".to_string(), Value::String(domain.to_string()));
                }
                if let Some(rt) = refresh_token.as_ref().filter(|s| !s.trim().is_empty()) {
                    auth_map.insert("refreshToken".to_string(), Value::String(rt.clone()));
                }

                let root = serde_json::json!({
                    "account": account_val.clone(),
                    "auth": Value::Object(auth_map),
                    "accounts": [account_val.clone()],
                    "allAccounts": [account_val]
                });

                serde_json::to_vec_pretty(&root).map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: format!("序列化 WorkBuddy 会话失败: {e}"),
                })?
            }
            CredentialPayload::Cookies { .. } => {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::UnsupportedCapability,
                    message: "WorkBuddy 不支持 Cookie 凭据模式".into(),
                });
            }
        };

        crate::adapter::write_private_file_atomic(&target, &bytes)?;

        Ok(())
    }

    async fn backup_session(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<crate::adapter::SessionBackup, AdapterError> {
        let auth_dir = host
            .client_config_dir("workbuddy")
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: e.to_string(),
            })?;
        crate::adapter::backup_files(&[auth_dir.join(AUTH_FILENAME)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_endpoint_allows_official_https_and_rejects_untrusted() {
        assert_eq!(
            WorkBuddyAdapter::validate_endpoint("https://copilot.tencent.com/").unwrap(),
            "https://copilot.tencent.com"
        );
        assert_eq!(
            WorkBuddyAdapter::validate_endpoint("https://www.workbuddy.cn").unwrap(),
            "https://www.workbuddy.cn"
        );
        assert!(WorkBuddyAdapter::validate_endpoint("http://copilot.tencent.com").is_err());
        assert!(WorkBuddyAdapter::validate_endpoint("https://evil.example.com").is_err());
        assert!(WorkBuddyAdapter::validate_endpoint("https://evil.tencent.com").is_err());
        assert!(WorkBuddyAdapter::validate_endpoint("https://copilot.tencent.com:8443").is_err());
        assert!(WorkBuddyAdapter::validate_endpoint("https://copilot.tencent.com/extra").is_err());
        assert!(WorkBuddyAdapter::validate_endpoint("https://copilot.tencent.com?x=1").is_err());
    }

    #[test]
    fn checkin_parser_is_fail_closed_when_fields_missing_or_error_code() {
        let missing = serde_json::json!({ "code": 0, "data": { "total_credits": 500 } });
        let (cs, credits, _) = WorkBuddyAdapter::parse_checkin_status_from_body(&missing).unwrap();
        assert_eq!(cs, ClaimStatus::Unknown);
        assert_eq!(credits, 500.0);

        let avail = serde_json::json!({ "code": 0, "data": { "today_checked_in": false } });
        let (cs2, _, _) = WorkBuddyAdapter::parse_checkin_status_from_body(&avail).unwrap();
        assert_eq!(cs2, ClaimStatus::Available);

        let claimed = serde_json::json!({ "code": 0, "data": { "today_checked_in": true } });
        let (cs3, _, _) = WorkBuddyAdapter::parse_checkin_status_from_body(&claimed).unwrap();
        assert_eq!(cs3, ClaimStatus::Claimed);

        let err_body = serde_json::json!({ "code": 401, "msg": "token expired" });
        assert!(matches!(
            WorkBuddyAdapter::parse_checkin_status_from_body(&err_body),
            Err(AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                ..
            })
        ));
    }

    #[test]
    fn checkin_claim_idempotent_contract() {
        // HTTP 400 + code 10001 = 今日已签（不得当作错误）
        let body = serde_json::json!({ "code": 10001, "msg": "already checked in" });
        assert!(WorkBuddyAdapter::checkin_claim_idempotent(400, &body));
        assert!(WorkBuddyAdapter::checkin_claim_idempotent(409, &body));
        // null body（含空响应）= 已签幂等仅限 2xx；400/409 空 body 不得吞错
        assert!(WorkBuddyAdapter::checkin_claim_idempotent(
            200,
            &Value::Null
        ));
        assert!(!WorkBuddyAdapter::checkin_claim_idempotent(
            400,
            &Value::Null
        ));
        assert!(!WorkBuddyAdapter::checkin_claim_idempotent(
            409,
            &Value::Null
        ));
        // 2xx + 已签文案
        let msg_body = serde_json::json!({ "code": 0, "msg": "今日已签到" });
        assert!(WorkBuddyAdapter::checkin_claim_idempotent(200, &msg_body));
        // 正常成功响应不是幂等跳过
        let success = serde_json::json!({ "code": 0, "data": { "credit": 100 } });
        assert!(!WorkBuddyAdapter::checkin_claim_idempotent(200, &success));
        // 500/403 等不得被文案启发式吞掉
        let err = serde_json::json!({ "code": 403, "msg": "already claimed" });
        assert!(!WorkBuddyAdapter::checkin_claim_idempotent(500, &err));
        assert!(!WorkBuddyAdapter::checkin_claim_idempotent(
            403,
            &Value::Null
        ));
        // 5xx 携带 10001 也不得当作已签
        let server_err = serde_json::json!({ "code": 10001 });
        assert!(!WorkBuddyAdapter::checkin_claim_idempotent(
            500,
            &server_err
        ));
    }

    struct TempDirHost {
        root: PathBuf,
    }

    #[async_trait]
    impl PlatformHost for TempDirHost {
        fn qwitch_data_dir(&self) -> Result<PathBuf, crate::platform::HostError> {
            Ok(self.root.clone())
        }
        fn client_config_dir(&self, _tool_id: &str) -> Result<PathBuf, crate::platform::HostError> {
            Ok(self.root.clone())
        }
        fn detect_client_executable(&self, _tool_id: &str) -> Option<PathBuf> {
            None
        }
        fn is_client_running(&self, _tool_id: &str) -> Result<bool, crate::platform::HostError> {
            Ok(false)
        }
        async fn terminate_client_gracefully(
            &self,
            _tool_id: &str,
            _timeout_ms: u64,
        ) -> Result<(), crate::platform::HostError> {
            Ok(())
        }
        fn launch_client(
            &self,
            _executable: &Path,
            _args: &[String],
        ) -> Result<(), crate::platform::HostError> {
            Ok(())
        }
        fn get_or_create_master_key(
            &self,
            _expectation: &crate::platform::KeyExpectation,
        ) -> Result<[u8; 32], crate::platform::HostError> {
            Ok([0u8; 32])
        }
        fn set_launch_at_login(&self, _enabled: bool) -> Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn numeric_uid_hashed_jwt_plaintext_session_and_clean_apply_session() {
        let adapter = WorkBuddyAdapter::new();
        assert!(adapter
            .manifest()
            .capabilities
            .auth_methods
            .contains(&AuthMethod::TokenPaste));

        // 1. 数字型 UID 解析（local_import + parse_manual_token）
        let dir = std::env::temp_dir().join(format!("qwitch_wb_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let sess_json = serde_json::json!({
            "account": { "uid": 100200300, "phoneNumber": "13812345678" },
            "auth": { "accessToken": "plain-wb-token-xyz", "refreshToken": "plain-wb-rt" }
        });
        std::fs::write(
            dir.join(AUTH_FILENAME),
            serde_json::to_vec(&sess_json).unwrap(),
        )
        .unwrap();
        let host = TempDirHost { root: dir.clone() };
        let imported = adapter
            .import_from_local(&host)
            .await
            .expect("local import");
        assert_eq!(imported.uid, "100200300");
        assert_eq!(imported.capability_flags(), (true, true));

        // 2. 手动粘贴明文会话 JSON 写入 decrypted_token.txt，has_api_token == true
        let pasted_sess = adapter
            .parse_manual_token(&sess_json.to_string())
            .await
            .expect("paste session");
        assert_eq!(pasted_sess.uid, "100200300");
        assert_eq!(pasted_sess.capability_flags(), (true, true));

        // 3. 相同前缀的 JWT 生成不同哈希 UID 且不泄露原始前缀
        let jwt_a = "eyJhbGciOiJIUzI1NiJ9.payload_one_aaa";
        let jwt_b = "eyJhbGciOiJIUzI1NiJ9.payload_two_bbb";
        let env_a = adapter.parse_manual_token(jwt_a).await.unwrap();
        let env_b = adapter.parse_manual_token(jwt_b).await.unwrap();
        assert_ne!(env_a.uid, env_b.uid);
        assert!(!env_a.uid.contains("eyJhbGciOiJI"));

        // 4. Token 模式切号不得残留上一账号的 enterpriseId / refreshToken / accounts
        let stale_prev = serde_json::json!({
            "account": { "uid": "old_uid", "enterpriseId": "ent-999", "phoneNumber": "13900001111" },
            "auth": { "accessToken": "old-tok", "refreshToken": { "$wbEncrypted": 1, "envelope": "stale" } },
            "accounts": [{ "uid": "old_uid" }],
            "allAccounts": [{ "uid": "old_uid" }]
        });
        std::fs::write(
            dir.join(AUTH_FILENAME),
            serde_json::to_vec(&stale_prev).unwrap(),
        )
        .unwrap();
        let target_cred = CredentialEnvelope {
            tool_id: "workbuddy".to_string(),
            uid: "new_uid_777".to_string(),
            display_name: Some("NewUser".to_string()),
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "new-tok-777".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };
        adapter
            .apply_session(&host, &target_cred)
            .await
            .expect("apply session");
        let written: Value =
            serde_json::from_slice(&std::fs::read(dir.join(AUTH_FILENAME)).unwrap()).unwrap();
        assert_eq!(
            written.pointer("/account/uid").and_then(|v| v.as_str()),
            Some("new_uid_777")
        );
        assert!(written.pointer("/account/enterpriseId").is_none());
        assert!(written.pointer("/account/phoneNumber").is_none());
        assert!(written.pointer("/auth/refreshToken").is_none());
        assert_eq!(
            written.pointer("/accounts/0/uid").and_then(|v| v.as_str()),
            Some("new_uid_777")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn jwt_claims_and_enterprise_headers_without_fallback_user_id() {
        let adapter = WorkBuddyAdapter::new();
        let header = BASE64_URL.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = BASE64_URL.encode(
            br#"{"sub":"99001122","email":"corp@example.com","name":"CorpUser","enterpriseId":"ent-42"}"#,
        );
        let jwt = format!("{header}.{payload}.sig_part");

        let parsed = adapter.parse_manual_token(&jwt).await.expect("parse jwt");
        assert_eq!(parsed.uid, "99001122");
        assert_eq!(parsed.display_email.as_deref(), Some("corp@example.com"));
        assert_eq!(parsed.display_name.as_deref(), Some("CorpUser"));
        if let CredentialPayload::Token { extra, .. } = &parsed.payload {
            assert_eq!(
                extra.get("enterprise_id").map(String::as_str),
                Some("ent-42")
            );
        } else {
            panic!("expected Token payload");
        }

        let json_env = adapter
            .parse_manual_token(
                r#"{"token":"tok-ent","uid":"real_88","enterpriseId":"ent-99","domain":"corp.oa"}"#,
            )
            .await
            .expect("parse flat json");
        if let CredentialPayload::Token { extra, .. } = &json_env.payload {
            assert_eq!(
                extra.get("enterprise_id").map(String::as_str),
                Some("ent-99")
            );
            assert_eq!(extra.get("domain").map(String::as_str), Some("corp.oa"));
        } else {
            panic!("expected Token payload");
        }

        let headers_real =
            WorkBuddyAdapter::build_headers("tok-1", "real_88", Some("ent-99"), Some("corp.oa"))
                .expect("headers real");
        assert_eq!(headers_real.get("X-User-Id").unwrap(), "real_88");
        assert_eq!(headers_real.get("X-Enterprise-Id").unwrap(), "ent-99");
        assert_eq!(headers_real.get("X-Tenant-Id").unwrap(), "ent-99");
        assert_eq!(headers_real.get("X-Domain").unwrap(), "corp.oa");

        let headers_fallback =
            WorkBuddyAdapter::build_headers("tok-2", "wb_manual_abcdef123456", None, None)
                .expect("headers fallback");
        assert!(
            headers_fallback.get("X-User-Id").is_none(),
            "兜底哈希 UID 不得作为 X-User-Id 发送"
        );
    }

    #[test]
    fn claim_response_rejects_html_on_200_and_accepts_empty_or_null() {
        let html_err = WorkBuddyAdapter::parse_claim_response_body(
            reqwest::StatusCode::OK,
            "<html><body>WAF Challenge</body></html>",
        )
        .expect_err("HTML 200 must fail");
        assert!(matches!(
            html_err,
            AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                ..
            }
        ));

        assert_eq!(
            WorkBuddyAdapter::parse_claim_response_body(reqwest::StatusCode::OK, "  ").unwrap(),
            Value::Null
        );
        assert_eq!(
            WorkBuddyAdapter::parse_claim_response_body(reqwest::StatusCode::OK, "null").unwrap(),
            Value::Null
        );
    }

    #[tokio::test]
    async fn workbuddy_refresh_rotates_near_expiry_forces_fresh_and_maps_401() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let adapter = WorkBuddyAdapter::with_api_base(server.uri());

        let make_tok = |exp: i64| {
            let hdr = BASE64_URL.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
            let pay =
                BASE64_URL.encode(serde_json::json!({ "sub": "wb-1", "exp": exp }).to_string());
            format!("{hdr}.{pay}.sig")
        };

        let fresh_exp = Utc::now().timestamp() + 3600;
        let fresh_cred = CredentialEnvelope {
            tool_id: "workbuddy".to_string(),
            uid: "wb-1".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: make_tok(fresh_exp),
                refresh_token: Some("rt-old".into()),
                expires_at: None,
                extra: HashMap::new(),
            },
        };
        assert!(adapter
            .refresh_credential(&fresh_cred)
            .await
            .expect("fresh skip")
            .is_none());

        let soon_exp = Utc::now().timestamp() + 120;
        let near_cred = CredentialEnvelope {
            tool_id: "workbuddy".to_string(),
            uid: "wb-1".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: make_tok(soon_exp),
                refresh_token: Some("rt-old".into()),
                expires_at: None,
                extra: HashMap::new(),
            },
        };
        let next_tok = make_tok(Utc::now().timestamp() + 7200);
        Mock::given(method("POST"))
            .and(path("/v2/plugin/auth/token/refresh"))
            .and(header("X-Refresh-Token", "rt-old"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "accessToken": next_tok,
                    "refreshToken": "rt-new",
                    "expiresIn": 3600
                }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        let updated = adapter
            .refresh_credential(&near_cred)
            .await
            .expect("refresh ok")
            .expect("updated envelope");
        match &updated.payload {
            CredentialPayload::Token {
                access_token,
                refresh_token,
                expires_at,
                ..
            } => {
                assert_eq!(access_token, &next_tok);
                assert_eq!(refresh_token.as_deref(), Some("rt-new"));
                assert!(expires_at.unwrap_or(0) > Utc::now().timestamp_millis());
            }
            _ => panic!("expected Token"),
        }

        // force_refresh_credential 绕过未临期检查
        let forced_tok = make_tok(Utc::now().timestamp() + 9000);
        Mock::given(method("POST"))
            .and(path("/v2/plugin/auth/token/refresh"))
            .and(header("X-Refresh-Token", "rt-old"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "accessToken": forced_tok,
                    "refreshToken": "rt-forced",
                    "expiresIn": 7200
                }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let forced = adapter
            .force_refresh_credential(&fresh_cred)
            .await
            .expect("force ok")
            .expect("forced envelope");
        match &forced.payload {
            CredentialPayload::Token { refresh_token, .. } => {
                assert_eq!(refresh_token.as_deref(), Some("rt-forced"));
            }
            _ => panic!("expected Token"),
        }

        Mock::given(method("POST"))
            .and(path("/v2/plugin/auth/token/refresh"))
            .respond_with(ResponseTemplate::new(401))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let err = adapter.refresh_credential(&near_cred).await.unwrap_err();
        assert!(matches!(
            err,
            AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                ..
            }
        ));
    }
}
