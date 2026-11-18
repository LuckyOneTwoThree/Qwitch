use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

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

fn shared_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

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
let chunks = [], size = 0;
function reply(v) { process.stdout.write(JSON.stringify({version: 1, ...v}), () => process.exit(v.ok ? 0 : 1)); }
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

pub struct WorkBuddyAdapter {
    manifest: AdapterManifest,
    http: reqwest::Client,
}

impl WorkBuddyAdapter {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            manifest: AdapterManifest {
                id: "workbuddy".to_string(),
                name: "WorkBuddy".to_string(),
                version: "0.1.0".to_string(),
                is_builtin: true,
                capabilities: AdapterCapabilities {
                    auth_methods: vec![
                        AuthMethod::LocalImport,
                        AuthMethod::WebviewOauth,
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
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty());
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
            if detected.extension().and_then(|e| e.to_str()) == Some("app")
                || detected.is_dir()
            {
                let macos_dir = detected.join("Contents/MacOS");
                for name in ["WorkBuddy", "Electron"] {
                    let candidate = macos_dir.join(name);
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
                if let Ok(entries) = std::fs::read_dir(&macos_dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_file() {
                            return Some(p);
                        }
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
        {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "WorkBuddy 端点必须为无查询参数的 HTTPS 地址".into(),
            });
        }
        let host = parsed
            .host_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let allowed = host == "copilot.tencent.com"
            || host == "www.workbuddy.cn"
            || host == "workbuddy.cn"
            || host.ends_with(".workbuddy.cn")
            || host.ends_with(".tencent.com");
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

        let mut child = Command::new(runtime_bin)
            .args(["-e", AUTH_HELPER_JS])
            .env("ELECTRON_RUN_AS_NODE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::ClientNotFound,
                message: format!("无法启动 WorkBuddy 运行时解密凭据: {e}"),
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&req_bytes).await;
            let _ = stdin.shutdown().await;
        }

        let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
            .await
            .map_err(|_| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: "WorkBuddy 解密助手执行超时".into(),
            })?
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("等待 WorkBuddy 解密助手失败: {e}"),
            })?;

        let reply: Value =
            serde_json::from_slice(&output.stdout).map_err(|_| AdapterError::Unified {
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
        out.remove("accessToken").ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "WorkBuddy 加密凭据解密失败 (DECRYPT_FAILED)".into(),
        })
    }

    fn format_display_identity(
        nickname: Option<&str>,
        phone: Option<&str>,
        email: Option<&str>,
    ) -> (Option<String>, Option<String>) {
        let clean_nick = nickname
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let clean_phone = phone
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let clean_email = email
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let display_name = match (clean_nick, clean_phone) {
            (Some(n), Some(p)) if n.chars().count() <= 1 || n == "~" => {
                Some(format!("WorkBuddy ({p})"))
            }
            (Some(n), _) => Some(n.to_string()),
            (None, Some(p)) => Some(format!("WorkBuddy ({p})")),
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
            .map_err(|e| format!("请求 WorkBuddy 授权状态失败: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("WorkBuddy 授权状态接口返回 HTTP {}", resp.status().as_u16()));
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

    pub async fn poll_oauth_result(state_token: &str) -> Result<Option<CredentialEnvelope>, String> {
        let http = shared_http_client();

        let token_url = format!("{WEB_AUTH_ORIGIN}/v2/plugin/auth/token?state={state_token}");
        let resp = http
            .get(&token_url)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, "WorkBuddy")
            .header("X-No-Authorization", "true")
            .header("X-No-User-Id", "true")
            .header("X-No-Enterprise-Id", "true")
            .header("X-No-Department-Info", "true")
            .send()
            .await
            .map_err(|e| format!("轮询 WorkBuddy Token 失败: {e}"))?;

        if !resp.status().is_success() {
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
        for _ in 0..4 {
            let acc_url = format!("{WEB_AUTH_ORIGIN}/v2/plugin/login/account?state={state_token}");
            if let Ok(acc_resp) = http
                .get(&acc_url)
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
                    if d.get("uid").and_then(|v| v.as_str()).is_some() {
                        account_val = d.clone();
                        break;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(350)).await;
        }

        if account_val.is_null() {
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
                        if let Some(first) = cons_body
                            .pointer("/data/accounts/0")
                            .or_else(|| cons_body.pointer("/accounts/0"))
                        {
                            account_val = first.clone();
                        }
                    }
                }
            }
        }

        let uid = account_val
            .get("uid")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| "WorkBuddy 授权成功但未获取到账号 UID".to_string())?
            .to_string();

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
                let b64 = files.get(AUTH_FILENAME).ok_or_else(|| AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "快照中缺少 workbuddy-desktop.info".into(),
                })?;
                let raw = BASE64.decode(b64).map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: format!("快照 Base64 解码失败: {e}"),
                })?;
                let sess = Self::parse_session_json(&raw)?;
                let uid = sess["account"]["uid"].as_str().unwrap_or(&cred.uid).to_string();
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
                    let bin = Self::resolve_runtime_binary(h).ok_or_else(|| AdapterError::Unified {
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
                    sess.pointer("/account/enterpriseId").and_then(|v| v.as_str()),
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
                let headers = Self::build_headers(access_token, &cred.uid, None, None)?;
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
        if let Ok(v) = HeaderValue::from_str(uid) {
            headers.insert("X-User-Id", v);
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

    fn map_http_status(status: reqwest::StatusCode, context: &str) -> AdapterError {
        match status.as_u16() {
            401 => AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                message: format!("{context}: 登录态已过期 (HTTP 401)"),
            },
            403 => AdapterError::Unified {
                code: UnifiedErrorCode::RiskTriggered,
                message: format!("{context}: 服务端拒绝访问 (HTTP 403)"),
            },
            429 => AdapterError::Unified {
                code: UnifiedErrorCode::RateLimited,
                message: format!("{context}: 请求过于频繁 (HTTP 429)"),
            },
            code => AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("{context}: HTTP {code}"),
            },
        }
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
        let streak_days = data.get("streak_days").and_then(|v| v.as_i64()).unwrap_or(0);
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
            return Err(Self::map_http_status(status, "查询 WorkBuddy 状态"));
        }

        let body: Value = resp.json().await.map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("解析 WorkBuddy 状态响应失败: {e}"),
        })?;

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
            if sum_resp.status().is_success() {
                if let Ok(sum_body) = sum_resp.json::<Value>().await {
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
                            cycle_total += parse_json_f64(pkg.get("CycleTotalCapacity")).unwrap_or(0.0);
                            cycle_remain += parse_json_f64(pkg.get("CycleRemainCapacity")).unwrap_or(0.0);
                            cycle_used += parse_json_f64(pkg.get("CycleUsedCapacity")).unwrap_or(0.0);
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
}

fn parse_json_f64(val: Option<&Value>) -> Option<f64> {
    let v = val?;
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

fn dig_data<'a>(val: &'a Value) -> &'a Value {
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
        let uid = sess["account"]["uid"].as_str().unwrap_or_default().to_string();

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
            .or_else(|| sess.pointer("/account/phoneNumber").and_then(|v| v.as_str()));
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

    async fn parse_manual_token(&self, raw_input: &str) -> Result<CredentialEnvelope, AdapterError> {
        let trimmed = raw_input.trim();
        if trimmed.is_empty() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "输入凭据不能为空".into(),
            });
        }

        if trimmed.starts_with('{') {
            if let Ok(sess) = Self::parse_session_json(trimmed.as_bytes()) {
                let uid = sess["account"]["uid"].as_str().unwrap_or_default().to_string();
                let email = sess
                    .pointer("/account/email")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let name = sess
                    .pointer("/account/nickName")
                    .or_else(|| sess.pointer("/account/nickname"))
                    .or_else(|| sess.pointer("/account/name"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let mut files = HashMap::new();
                files.insert(AUTH_FILENAME.to_string(), BASE64.encode(trimmed.as_bytes()));
                return Ok(CredentialEnvelope {
                    tool_id: "workbuddy".to_string(),
                    uid,
                    display_name: name,
                    display_email: email,
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
                .ok_or_else(|| AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "JSON 中缺少 token/accessToken 字段".into(),
                })?;
            let uid = val
                .get("userId")
                .or_else(|| val.get("uid"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("wb_oauth_{}", &token.chars().take(12).collect::<String>()));
            let email = val
                .get("email")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let name = val
                .get("name")
                .or_else(|| val.get("nickName"))
                .or_else(|| val.get("nickname"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
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
                    expires_at: None,
                    extra: HashMap::new(),
                },
            });
        }

        Ok(CredentialEnvelope {
            tool_id: "workbuddy".to_string(),
            uid: format!("wb_manual_{}", &trimmed.chars().take(12).collect::<String>()),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: trimmed.to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        })
    }

    async fn inspect_quota(&self, cred: &CredentialEnvelope) -> Result<QuotaSnapshot, AdapterError> {
        let host = crate::platform::NativePlatformHost;
        let (endpoint, _uid, headers) = Self::extract_api_context(cred, Some(&host)).await?;
        self.fetch_quota_and_checkin(&endpoint, &headers).await
    }

    async fn claim_daily(&self, cred: &CredentialEnvelope) -> Result<ClaimResult, AdapterError> {
        let host = crate::platform::NativePlatformHost;
        let (endpoint, _uid, headers) = Self::extract_api_context(cred, Some(&host)).await?;

        let current = self.fetch_quota_and_checkin(&endpoint, &headers).await?;
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
            return Err(Self::map_http_status(status, "WorkBuddy 每日签到"));
        }

        let raw_text = resp.text().await.map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("读取 WorkBuddy 签到响应失败: {e}"),
        })?;
        let body: Value = serde_json::from_str(&raw_text).map_err(|e| {
            if !status.is_success() {
                Self::map_http_status(status, "WorkBuddy 每日签到")
            } else {
                AdapterError::Unified {
                    code: UnifiedErrorCode::NetworkError,
                    message: format!("解析 WorkBuddy 签到响应失败: {e}"),
                }
            }
        })?;

        let code = body.get("code").and_then(|v| v.as_i64());
        let msg = body
            .get("msg")
            .or_else(|| body.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let msg_lower = msg.to_lowercase();
        let already_claimed = code == Some(10001)
            || msg.contains("已签")
            || msg.contains("重复")
            || msg_lower.contains("already")
            || msg_lower.contains("checked");
        if already_claimed {
            let refreshed = self
                .fetch_quota_and_checkin(&endpoint, &headers)
                .await
                .ok()
                .or(Some(current));
            return Ok(ClaimResult {
                claimed: false,
                reward_summary: "今日已签过".into(),
                updated_quota: refreshed,
            });
        }

        if !status.is_success() {
            return Err(Self::map_http_status(status, "WorkBuddy 每日签到"));
        }

        if let Some(c) = code.filter(|&c| c != 0) {
            let detail = if msg.is_empty() { "签到未成功" } else { msg };
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("WorkBuddy 每日签到返回异常 ({c}): {detail}"),
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
        let refreshed = self.fetch_quota_and_checkin(&endpoint, &headers).await.ok();
        let reward_summary = match credit_opt {
            Some(c) => format!("成功领取 {c} 积分"),
            None => "每日签到已完成".to_string(),
        };

        Ok(ClaimResult {
            claimed: true,
            reward_summary,
            updated_quota: refreshed,
        })
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
                let b64 = files.get(AUTH_FILENAME).ok_or_else(|| AdapterError::Unified {
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

                let mut root = if target.is_file() {
                    std::fs::read(&target)
                        .ok()
                        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
                        .filter(|v| v.is_object())
                        .unwrap_or_else(|| serde_json::json!({}))
                } else {
                    serde_json::json!({})
                };

                if !root.get("account").is_some_and(|v| v.is_object()) {
                    root["account"] = serde_json::json!({});
                }
                if !root.get("auth").is_some_and(|v| v.is_object()) {
                    root["auth"] = serde_json::json!({});
                }

                if let Some(acc_obj) = root.get_mut("account").and_then(|v| v.as_object_mut()) {
                    acc_obj.insert("uid".to_string(), Value::String(cred.uid.clone()));
                    if let Some(email) = cred.display_email.as_ref().filter(|s| !s.trim().is_empty()) {
                        acc_obj.insert("email".to_string(), Value::String(email.clone()));
                    }
                    if let Some(name) = cred.display_name.as_ref().filter(|s| !s.trim().is_empty()) {
                        acc_obj.insert("nickName".to_string(), Value::String(name.clone()));
                    }
                }
                if let Some(auth_obj) = root.get_mut("auth").and_then(|v| v.as_object_mut()) {
                    auth_obj.insert("accessToken".to_string(), Value::String(access_token.clone()));
                    if let Some(rt) = refresh_token.as_ref().filter(|s| !s.trim().is_empty()) {
                        auth_obj.insert("refreshToken".to_string(), Value::String(rt.clone()));
                    }
                    if !auth_obj.contains_key("endpoint") {
                        auth_obj.insert("endpoint".to_string(), Value::String(endpoint.to_string()));
                    }
                }

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

        let tmp = auth_dir.join(".workbuddy-desktop.info.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("写入临时凭据失败: {e}"),
        })?;
        std::fs::rename(&tmp, &target).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("替换 WorkBuddy 会话文件失败: {e}"),
        })?;

        Ok(())
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
}
