use aes::cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes128;
use async_trait::async_trait;
use base64::{
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD as BASE64_URL},
    Engine as _,
};
use chrono::{DateTime, SecondsFormat, Utc};
use rand::RngCore;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256, Sha512};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::adapter::{
    AdapterCapabilities, AdapterError, AdapterManifest, ClaimResult, ToolAdapter,
};
use crate::domain::{
    AuthMethod, ClaimStatus, CredentialEnvelope, CredentialPayload, QuotaBucket, QuotaSnapshot,
    UnifiedErrorCode,
};
use crate::platform::PlatformHost;

const STORAGE_JSON: &str = "storage.json";
const STATE_VSCDB: &str = "state.vscdb";
const SNAPSHOT_AUTH_KEY: &str = "icube_auth_entries.json";
const SNAPSHOT_VSCDB_KEY: &str = "icube_vscdb_entries.json";
const SNAPSHOT_SERVERDATA_KEY: &str = "icube_serverdata_entries.json";
const DECRYPTED_TOKEN_FILE: &str = "decrypted_token.txt";
const PRIMARY_AUTH_KEY: &str = "iCubeAuthInfo://icube.cloudide";
const USERTAG_KEY: &str = "iCubeAuthInfo://usertag";
const DEVICE_CRED_PREFIX: &str = "iCubeAuthInfo://icube-dc:";
const SERVER_DATA_PREFIX: &str = "iCubeServerData://";
const DEFAULT_TRAE_APP_VERSION: &str = "3.4.22";
const CLAIM_CODE_DEVICE_ALREADY_CHECKED_IN: i64 = 9095;

struct TraeDeviceHeaders {
    device_id: String,
    device_brand: Option<String>,
    device_type: &'static str,
    os_version: Option<String>,
    app_version: String,
}

impl TraeDeviceHeaders {
    /// 与 Trae CN 客户端 `ob()` 完全对齐的设备请求头。
    /// 服务端签到下单强依赖 `x-device-id`，缺失时返回业务码 9004。
    fn apply(&self, headers: &mut HeaderMap) {
        Self::insert(headers, "x-device-id", Some(self.device_id.as_str()));
        Self::insert(headers, "x-device-brand", self.device_brand.as_deref());
        Self::insert(headers, "x-device-type", Some(self.device_type));
        Self::insert(headers, "x-os-version", self.os_version.as_deref());
        Self::insert(headers, "x-app-version", Some(self.app_version.as_str()));
    }

    fn insert(headers: &mut HeaderMap, name: &'static str, value: Option<&str>) {
        let Some(value) = value.and_then(TraeAdapter::sanitize_header_value) else {
            return;
        };
        if let Ok(header_value) = HeaderValue::from_str(&value) {
            headers.insert(HeaderName::from_static(name), header_value);
        }
    }
}

pub struct TraeAdapter {
    is_cn: bool,
    manifest: AdapterManifest,
    http: reqwest::Client,
    host: Option<Arc<dyn PlatformHost>>,
    device_cache: OnceLock<Option<Arc<TraeDeviceHeaders>>>,
}

impl TraeAdapter {
    fn new_inner(is_cn: bool, host: Option<Arc<dyn PlatformHost>>) -> Self {
        let (id, name) = if is_cn {
            ("trae_cn".to_string(), "Trae CN".to_string())
        } else {
            ("trae".to_string(), "Trae".to_string())
        };
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            is_cn,
            manifest: AdapterManifest {
                id,
                name,
                version: "0.1.0".to_string(),
                is_builtin: true,
                capabilities: AdapterCapabilities {
                    auth_methods: vec![
                        AuthMethod::LocalImport,
                        AuthMethod::WebviewOauth,
                        AuthMethod::TokenPaste,
                    ],
                    supports_quota_query: true,
                    supports_daily_claim: is_cn,
                    supports_client_switch: true,
                    supports_multi_instance: false,
                },
            },
            http,
            host,
            device_cache: OnceLock::new(),
        }
    }

    pub fn new(is_cn: bool) -> Self {
        Self::new_inner(is_cn, None)
    }

    pub fn with_host(is_cn: bool, host: Arc<dyn PlatformHost>) -> Self {
        Self::new_inner(is_cn, Some(host))
    }

    fn tool_id(&self) -> &str {
        if self.is_cn {
            "trae_cn"
        } else {
            "trae"
        }
    }

    fn sanitize_header_value(raw: &str) -> Option<String> {
        let cleaned: String = raw.chars().filter(|c| !c.is_control()).collect();
        let trimmed = cleaned.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    /// 从 storage.json 的 `iCubeAuthInfo://icube-dc:<deviceId>` 键名解析本机设备 ID
    fn device_id_from_storage(storage_path: &Path) -> Option<String> {
        let raw = std::fs::read(storage_path).ok()?;
        let value: Value = serde_json::from_slice(&raw).ok()?;
        value
            .as_object()?
            .keys()
            .find_map(|key| key.strip_prefix(DEVICE_CRED_PREFIX))
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    }

    /// 兜底：aha 设备注册信息（`<app>/aha/TinyStorage`，与客户端同为 tc 加密）
    fn device_id_from_aha(config_dir: &Path) -> Option<String> {
        let app_dir = config_dir.parent()?.parent()?;
        let raw = std::fs::read_to_string(app_dir.join("aha").join("TinyStorage")).ok()?;
        let value: Value = serde_json::from_str(&raw).ok()?;
        let encoded = value
            .get("tiny_storage_data")?
            .get("aha.device.device_id")?
            .as_str()?;
        let decoded = Self::decrypt_tc_string(encoded)?;
        decoded
            .get("device_id_str")
            .or_else(|| decoded.get("device_id"))
            .and_then(crate::adapter::common::json_uid_to_string)
            .filter(|id| id != "0" && !id.trim().is_empty())
    }

    fn device_type_tag() -> &'static str {
        if cfg!(target_os = "macos") {
            "mac"
        } else if cfg!(target_os = "windows") {
            "win"
        } else {
            "linux"
        }
    }

    #[cfg(target_os = "macos")]
    fn detect_device_brand() -> Option<String> {
        let output = std::process::Command::new("sysctl")
            .args(["-n", "hw.model"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Self::sanitize_header_value(&String::from_utf8_lossy(&output.stdout))
    }

    #[cfg(not(target_os = "macos"))]
    fn detect_device_brand() -> Option<String> {
        None
    }

    #[cfg(target_os = "macos")]
    fn detect_os_version() -> Option<String> {
        let output = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if version.is_empty() {
            return None;
        }
        Some(format!("macOS {version}"))
    }

    #[cfg(not(target_os = "macos"))]
    fn detect_os_version() -> Option<String> {
        None
    }

    fn resolve_device_headers(&self) -> Option<TraeDeviceHeaders> {
        let host = self.host.as_ref()?;
        let config_dir = host.client_config_dir(self.tool_id()).ok()?;
        let device_id = Self::device_id_from_aha(&config_dir)
            .or_else(|| Self::device_id_from_storage(&config_dir.join(STORAGE_JSON)))?;
        Some(TraeDeviceHeaders {
            device_id,
            device_brand: Self::detect_device_brand(),
            device_type: Self::device_type_tag(),
            os_version: Self::detect_os_version(),
            app_version: DEFAULT_TRAE_APP_VERSION.to_string(),
        })
    }

    /// 仅缓存成功解析的结果：若本机尚无 Trae 设备标识，后续登录后可重新探测
    fn device_headers(&self) -> Option<Arc<TraeDeviceHeaders>> {
        if let Some(cached) = self.device_cache.get() {
            return cached.clone();
        }
        let resolved = Arc::new(self.resolve_device_headers()?);
        let _ = self.device_cache.set(Some(Arc::clone(&resolved)));
        Some(resolved)
    }

    fn extract_jwt_claims(token: &str) -> Option<Value> {
        let mut parts = token.split('.');
        let _header = parts.next()?;
        let payload_b64 = parts.next()?;
        let bytes = BASE64_URL.decode(payload_b64).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn hash_fallback_uid(prefix: &str, raw: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(raw.as_bytes());
        let hex_digest = hex::encode(hasher.finalize());
        format!("{prefix}_{}", &hex_digest[..12])
    }

    fn tc_mask_a(is_private: bool) -> [u8; 64] {
        const OPE: [u8; 64] = [
            82, 9, 106, 213, 48, 54, 165, 56, 191, 64, 163, 158, 129, 243, 215, 251, 124, 227, 57,
            130, 155, 47, 255, 135, 52, 142, 67, 68, 196, 222, 233, 203, 84, 123, 148, 50, 166,
            194, 35, 61, 238, 76, 149, 11, 66, 250, 195, 78, 8, 46, 161, 102, 40, 217, 36, 178,
            118, 91, 162, 73, 109, 139, 209, 37,
        ];
        const APE: [u8; 64] = [
            31, 221, 168, 51, 136, 7, 199, 49, 177, 18, 16, 89, 39, 128, 236, 95, 96, 81, 127, 169,
            25, 181, 74, 13, 45, 229, 122, 159, 147, 201, 156, 239, 160, 224, 59, 77, 174, 42, 245,
            176, 200, 235, 187, 60, 131, 83, 153, 97, 23, 43, 4, 126, 186, 119, 214, 38, 225, 105,
            20, 99, 85, 33, 12, 125,
        ];
        const RPE: [u8; 64] = [
            191, 192, 216, 250, 122, 246, 220, 97, 31, 254, 98, 27, 8, 72, 71, 176, 135, 99, 96,
            18, 127, 101, 203, 104, 211, 102, 191, 125, 37, 72, 150, 156, 51, 229, 121, 35, 17,
            153, 141, 177, 110, 131, 150, 128, 172, 255, 254, 6, 18, 140, 55, 62, 236, 249, 135,
            64, 135, 12, 117, 4, 89, 149, 168, 209,
        ];
        const NPE: [u8; 64] = [
            246, 204, 26, 232, 232, 70, 129, 109, 223, 146, 169, 242, 23, 241, 105, 145, 50, 196,
            165, 42, 254, 120, 3, 54, 244, 207, 209, 85, 53, 6, 138, 106, 175, 148, 31, 204, 186,
            186, 165, 182, 87, 142, 49, 10, 39, 110, 26, 154, 86, 56, 173, 125, 18, 64, 198, 225,
            99, 99, 83, 82, 191, 134, 76, 170,
        ];
        let mut out = [0u8; 64];
        if is_private {
            for i in 0..64 {
                out[i] = RPE[i] ^ NPE[i];
            }
        } else {
            for i in 0..64 {
                out[i] = OPE[i] ^ APE[i];
            }
        }
        out
    }

    fn decrypt_tc_string(b64_str: &str) -> Option<Value> {
        let raw = BASE64.decode(b64_str.trim()).ok()?;
        if raw.len() <= 6 + 32 + 64 {
            return None;
        }
        let is_private = if raw[0] == 18
            && raw[1] == 57
            && raw[2] == 32
            && raw[3] == 32
            && raw[4] == 2
            && raw[5] == 3
        {
            true
        } else if raw[0] == 116
            && raw[1] == 99
            && raw[2] == 5
            && raw[3] == 16
            && raw[4] == 0
            && raw[5] == 0
        {
            false
        } else {
            return None;
        };

        let seed = &raw[6..38];
        let ct = &raw[38..];
        if ct.is_empty() || ct.len() % 16 != 0 {
            return None;
        }

        let o = Sha512::digest(seed);
        let a = Self::tc_mask_a(is_private);
        let mut hasher = Sha512::new();
        hasher.update(o);
        hasher.update(a);
        let c = hasher.finalize();

        let aes_key = &c[0..16];
        let iv = &c[16..32];
        let cipher = Aes128::new_from_slice(aes_key).ok()?;

        let mut dec = Vec::with_capacity(ct.len());
        let mut prev = iv;
        for chunk in ct.as_chunks::<16>().0 {
            let mut block = [0u8; 16];
            block.copy_from_slice(chunk);
            cipher.decrypt_block(GenericArray::from_mut_slice(&mut block));
            for i in 0..16 {
                block[i] ^= prev[i];
            }
            dec.extend_from_slice(&block);
            prev = chunk;
        }

        let pad = *dec.last()? as usize;
        if pad == 0 || pad > 16 || dec.len() < pad {
            return None;
        }
        if !dec[dec.len() - pad..].iter().all(|&b| b as usize == pad) {
            return None;
        }
        dec.truncate(dec.len() - pad);
        if dec.len() <= 64 {
            return None;
        }

        let expected_hash = &dec[0..64];
        let body = &dec[64..];
        if Sha512::digest(body).as_slice() != expected_hash {
            return None;
        }
        serde_json::from_slice::<Value>(body).ok()
    }

    fn encrypt_tc_string(plaintext: &str) -> Option<String> {
        let mut seed = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut seed);

        let o = Sha512::digest(seed);
        let a = Self::tc_mask_a(false);
        let mut hasher = Sha512::new();
        hasher.update(o);
        hasher.update(a);
        let c = hasher.finalize();

        let aes_key = &c[0..16];
        let iv = &c[16..32];
        let cipher = Aes128::new_from_slice(aes_key).ok()?;

        let body = plaintext.as_bytes();
        let digest = Sha512::digest(body);
        let mut plain = Vec::with_capacity(64 + body.len() + 16);
        plain.extend_from_slice(&digest);
        plain.extend_from_slice(body);
        let pad = 16 - (plain.len() % 16);
        plain.resize(plain.len() + pad, pad as u8);

        let mut out = Vec::with_capacity(6 + 32 + plain.len());
        out.extend_from_slice(&[116, 99, 5, 16, 0, 0]);
        out.extend_from_slice(&seed);

        let mut prev = [0u8; 16];
        prev.copy_from_slice(iv);
        for chunk in plain.as_chunks::<16>().0 {
            let mut block = [0u8; 16];
            for i in 0..16 {
                block[i] = chunk[i] ^ prev[i];
            }
            cipher.encrypt_block(GenericArray::from_mut_slice(&mut block));
            out.extend_from_slice(&block);
            prev = block;
        }

        Some(BASE64.encode(out))
    }

    fn parse_auth_entries_from_storage(
        storage_val: &Value,
    ) -> Result<Map<String, Value>, AdapterError> {
        let obj = storage_val
            .as_object()
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "storage.json 根节点不是 JSON 对象".into(),
            })?;

        let mut auth_entries = Map::new();
        for (k, v) in obj {
            if k.starts_with("iCubeAuthInfo://") {
                auth_entries.insert(k.clone(), v.clone());
            }
        }

        if auth_entries.is_empty() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::ClientNotFound,
                message: "Trae storage.json 中未找到 iCubeAuthInfo 登录态，请先在客户端登录".into(),
            });
        }

        Ok(auth_entries)
    }

    fn extract_identity_from_json(
        tool_id: &str,
        parsed: &Value,
        raw_for_hash: &str,
    ) -> (String, Option<String>, Option<String>, Option<String>) {
        let uid = parsed
            .get("userId")
            .or_else(|| parsed.get("uid"))
            .or_else(|| parsed.pointer("/account/uid"))
            .and_then(crate::adapter::common::json_uid_to_string)
            .unwrap_or_else(|| Self::hash_fallback_uid(tool_id, raw_for_hash));
        let email = parsed
            .get("email")
            .or_else(|| parsed.pointer("/account/email"))
            .or_else(|| parsed.pointer("/account/nonPlainTextMobile"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let name = parsed
            .pointer("/account/username")
            .or_else(|| parsed.pointer("/account/nickname"))
            .or_else(|| parsed.get("nickName"))
            .or_else(|| parsed.get("name"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let token = parsed
            .get("token")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        (uid, name, email, token)
    }

    fn extract_identity_from_auth_entries(
        tool_id: &str,
        entries: &Map<String, Value>,
    ) -> (String, Option<String>, Option<String>, Option<String>) {
        let primary = entries
            .get(PRIMARY_AUTH_KEY)
            .or_else(|| entries.values().next());

        if let Some(val) = primary {
            if let Some(raw_str) = val.as_str() {
                if raw_str.trim_start().starts_with('{') {
                    if let Ok(parsed) = serde_json::from_str::<Value>(raw_str) {
                        return Self::extract_identity_from_json(tool_id, &parsed, raw_str);
                    }
                } else if let Some(parsed) = Self::decrypt_tc_string(raw_str) {
                    return Self::extract_identity_from_json(tool_id, &parsed, raw_str);
                }
                // 无法解密时生成稳定指纹 UID 用于防重复绑定，原样保留密文用于冷切换
                return (Self::hash_fallback_uid(tool_id, raw_str), None, None, None);
            }
        }

        let serialized = serde_json::to_string(entries).unwrap_or_default();
        (
            Self::hash_fallback_uid(tool_id, &serialized),
            None,
            None,
            None,
        )
    }

    fn read_vscdb_auth_entries(vscdb_path: &std::path::Path) -> Option<Map<String, Value>> {
        let conn = rusqlite::Connection::open_with_flags(
            vscdb_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .ok()?;
        let mut stmt = conn
            .prepare("SELECT key, value FROM ItemTable WHERE key LIKE 'iCubeAuthInfo://%'")
            .ok()?;
        let mut rows = stmt.query([]).ok()?;
        let mut entries = Map::new();
        while let Ok(Some(row)) = rows.next() {
            let Ok(key) = row.get::<_, String>(0) else {
                continue;
            };
            let Ok(val_ref) = row.get_ref(1) else {
                continue;
            };
            let val_str = match val_ref {
                rusqlite::types::ValueRef::Text(b) | rusqlite::types::ValueRef::Blob(b) => {
                    String::from_utf8_lossy(b).to_string()
                }
                _ => continue,
            };
            entries.insert(key, Value::String(val_str));
        }
        if entries.is_empty() {
            None
        } else {
            Some(entries)
        }
    }

    fn apply_vscdb_auth_entries(
        vscdb_path: &std::path::Path,
        entries: &Map<String, Value>,
    ) -> Result<(), AdapterError> {
        if !vscdb_path.is_file() {
            return Ok(());
        }
        let mut conn =
            rusqlite::Connection::open(vscdb_path).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("打开 Trae state.vscdb 失败: {e}"),
            })?;
        let _ = conn.busy_timeout(Duration::from_millis(5000));
        let tx = conn.transaction().map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("开启 Trae state.vscdb 事务失败: {e}"),
        })?;
        tx.execute(
            "DELETE FROM ItemTable WHERE key LIKE 'iCubeAuthInfo://%'",
            [],
        )
        .map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("清理 Trae state.vscdb 旧登录态失败: {e}"),
        })?;
        for (k, v) in entries {
            let val_str = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            tx.execute(
                "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?1, ?2)",
                rusqlite::params![k, val_str],
            )
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("写入 Trae state.vscdb 登录态失败: {e}"),
            })?;
        }
        tx.commit().map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("提交 Trae state.vscdb 事务失败: {e}"),
        })?;
        Ok(())
    }

    fn base_api_url(&self) -> &'static str {
        if self.is_cn {
            "https://api.trae.cn"
        } else {
            "https://api.trae.ai"
        }
    }

    fn build_api_headers(&self, token: &str) -> Result<HeaderMap, AdapterError> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let auth_val = HeaderValue::from_str(&format!("Cloud-IDE-JWT {token}")).map_err(|_| {
            AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "Trae Token 包含非法字符".into(),
            }
        })?;
        headers.insert(AUTHORIZATION, auth_val);
        if self.is_cn {
            headers.insert("X-User-Region", HeaderValue::from_static("CN"));
        }
        if let Some(device) = self.device_headers() {
            device.apply(&mut headers);
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

    fn extract_token_from_cred(&self, cred: &CredentialEnvelope) -> (Option<String>, bool) {
        let mut token_opt: Option<String> = None;
        let mut is_encrypted_local = false;

        match &cred.payload {
            CredentialPayload::Token { access_token, .. } => {
                token_opt = Some(access_token.clone());
            }
            CredentialPayload::LocalSnapshot { files } => {
                if let Some(b64) = files.get(SNAPSHOT_AUTH_KEY) {
                    if let Ok(bytes) = BASE64.decode(b64) {
                        if let Ok(entries) = serde_json::from_slice::<Map<String, Value>>(&bytes) {
                            let (_, _, _, tok) =
                                Self::extract_identity_from_auth_entries(self.tool_id(), &entries);
                            if tok.is_some() {
                                token_opt = tok;
                            } else {
                                is_encrypted_local = true;
                            }
                        }
                    }
                }
                if token_opt.is_none() {
                    if let Some(tok_b64) = files.get(DECRYPTED_TOKEN_FILE) {
                        if let Ok(bytes) = BASE64.decode(tok_b64) {
                            if let Ok(tok) = String::from_utf8(bytes) {
                                if !tok.trim().is_empty() {
                                    token_opt = Some(tok.trim().to_string());
                                }
                            }
                        }
                    }
                }
            }
            CredentialPayload::Cookies { .. } => {}
        }

        (token_opt, is_encrypted_local)
    }

    fn parse_checkin_status_from_body(
        body: &Value,
    ) -> Result<(ClaimStatus, Option<i64>), AdapterError> {
        if let Some(code) = body.get("code").and_then(|v| v.as_i64()) {
            if code != 0 {
                let msg = body
                    .get("message")
                    .or_else(|| body.get("msg"))
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
                        message: format!("Trae CN 登录态已失效 ({code}): {msg}"),
                    });
                }
                return Ok((ClaimStatus::Unknown, None));
            }
        }
        let data = body.get("data").filter(|v| v.is_object()).unwrap_or(body);

        let enabled = data.get("enable").and_then(|v| v.as_bool());
        let credits = data
            .get("credits")
            .or_else(|| data.get("extra_credits"))
            .and_then(|v| v.as_i64());

        if enabled == Some(false) {
            return Ok((ClaimStatus::Unsupported, credits));
        }

        let checked_in = data
            .get("checked_in")
            .and_then(|v| v.as_bool())
            .or_else(|| data.get("did_checked_in").and_then(|v| v.as_bool()));

        let claim_status = match checked_in {
            Some(true) => ClaimStatus::Claimed,
            Some(false) => ClaimStatus::Available,
            None => ClaimStatus::Unknown,
        };

        Ok((claim_status, credits))
    }

    async fn fetch_checkin_status(
        &self,
        token: &str,
    ) -> Result<(ClaimStatus, Option<i64>), AdapterError> {
        if !self.is_cn {
            return Ok((ClaimStatus::Unsupported, None));
        }

        let url = format!(
            "{}/trae/api/v2/ug/checkin_credits/status",
            self.base_api_url()
        );
        let headers = self.build_api_headers(token)?;
        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(&serde_json::json!({ "req_source": 1 }))
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("查询 Trae CN 签到状态失败: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            return Err(Self::map_http_status(status, "查询 Trae CN 签到状态"));
        }

        let body: Value = resp.json().await.map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("解析 Trae CN 签到状态失败: {e}"),
        })?;
        if let Some(code) = body.get("code").and_then(|v| v.as_i64()) {
            if let Some(err) =
                crate::adapter::common::map_business_code(code, "查询 Trae CN 签到状态")
            {
                return Err(err);
            }
        }
        Self::parse_checkin_status_from_body(&body)
    }

    async fn fetch_entitlement_quota(&self, token: &str) -> Result<QuotaSnapshot, AdapterError> {
        let url = format!(
            "{}/trae/api/v2/pay/user_current_entitlement_list",
            self.base_api_url()
        );
        let headers = self.build_api_headers(token)?;

        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(&serde_json::json!({
                "require_usage": true,
                "full_data": true
            }))
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("查询 Trae 额度失败: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            return Err(Self::map_http_status(status, "查询 Trae 额度"));
        }

        let body: Value = resp.json().await.map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("解析 Trae 额度响应失败: {e}"),
        })?;

        if let Some(code) = body.get("code").and_then(|v| v.as_i64()) {
            if code != 0 {
                if let Some(err) = crate::adapter::common::map_business_code(code, "查询 Trae 额度")
                {
                    return Err(err);
                }
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::NetworkError,
                    message: format!("Trae 额度接口返回异常业务码 {code}"),
                });
            }
        }

        let mut used = 0.0_f64;
        let mut total = 0.0_f64;
        let mut plan_name: Option<String> = None;
        let mut reset_at: Option<String> = None;

        if let Some(summary) = body
            .get("usage_summary")
            .or_else(|| body.pointer("/data/usage_summary"))
        {
            used = summary
                .get("consumed_amount")
                .or_else(|| summary.get("used"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            total = summary
                .get("total_amount")
                .or_else(|| summary.get("total"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
        }

        let packs = body
            .get("user_entitlement_pack_list")
            .or_else(|| body.pointer("/data/user_entitlement_pack_list"))
            .and_then(|v| v.as_array());

        if let Some(packs) = packs {
            // 优先从主订阅包 (product_type == 0) 读取套餐名与过期时间
            for pack in packs {
                let is_base_plan = pack
                    .pointer("/entitlement_base_info/product_type")
                    .and_then(|v| v.as_i64())
                    == Some(0);
                if is_base_plan {
                    if plan_name.is_none() {
                        plan_name = pack
                            .get("display_desc")
                            .or_else(|| {
                                pack.pointer("/entitlement_base_info/product_extra/package_name")
                            })
                            .or_else(|| pack.pointer("/entitlement_base_info/package_name"))
                            .or_else(|| pack.pointer("/entitlement_base_info/product_name"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty());
                    }
                    if reset_at.is_none() {
                        if let Some(end_ts) = pack
                            .pointer("/entitlement_base_info/end_time")
                            .or_else(|| pack.get("expire_time"))
                            .and_then(|v| v.as_i64())
                        {
                            let secs = if end_ts > 1_000_000_000_000 {
                                end_ts / 1000
                            } else {
                                end_ts
                            };
                            if let Some(dt) = chrono::DateTime::from_timestamp(secs, 0) {
                                reset_at = Some(dt.to_rfc3339());
                            }
                        }
                    }
                }
            }

            let need_pack_usage = total <= 0.0;
            for pack in packs {
                if need_pack_usage {
                    if let Some(usage) = pack.get("usage") {
                        used += usage
                            .get("used")
                            .or_else(|| usage.get("consumed_amount"))
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        total += usage
                            .get("total")
                            .or_else(|| usage.get("total_amount"))
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                    }
                }
                if plan_name.is_none() {
                    plan_name = pack
                        .pointer("/entitlement_base_info/product_extra/package_name")
                        .or_else(|| pack.pointer("/entitlement_base_info/package_name"))
                        .or_else(|| pack.pointer("/entitlement_base_info/product_name"))
                        .or_else(|| pack.get("package_name"))
                        .or_else(|| pack.get("product_name"))
                        .or_else(|| pack.get("display_desc"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                }
                if reset_at.is_none() {
                    if let Some(end_ts) = pack
                        .pointer("/entitlement_base_info/end_time")
                        .or_else(|| pack.get("end_time"))
                        .or_else(|| pack.get("expire_time"))
                        .and_then(|v| v.as_i64())
                    {
                        let secs = if end_ts > 1_000_000_000_000 {
                            end_ts / 1000
                        } else {
                            end_ts
                        };
                        if let Some(dt) = chrono::DateTime::from_timestamp(secs, 0) {
                            reset_at = Some(dt.to_rfc3339());
                        }
                    }
                }
            }
        }

        let total_balance = body
            .get("total_balance")
            .or_else(|| body.pointer("/data/total_balance"))
            .and_then(|v| v.as_f64());

        if total <= 0.0 {
            if let Some(bal) = total_balance.filter(|b| *b >= 0.0) {
                total = bal + used;
            }
        }

        if total <= 0.0 {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: "Trae 额度响应中未包含有效用量数据".into(),
            });
        }

        let remaining = total_balance.unwrap_or_else(|| (total - used).max(0.0));
        Ok(QuotaSnapshot {
            plan_name,
            claim_status: ClaimStatus::Unsupported,
            buckets: vec![QuotaBucket {
                key: "trae_credits".into(),
                label: "可用额度".into(),
                used,
                total,
                remaining,
                unit: "Credits".into(),
                reset_at,
            }],
            fetched_at: Utc::now().to_rfc3339(),
        })
    }

    /// 构造与 Trae CN 客户端一致的登录态载荷。
    /// `expiredAt` 是客户端判定"是否已登录"的硬性字段，缺失会导致启动后直接回到登录页。
    fn build_token_auth_entry(
        &self,
        cred: &CredentialEnvelope,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<Value, AdapterError> {
        let expires_secs = expires_at.or_else(|| {
            Self::extract_jwt_claims(access_token)
                .and_then(|claims| claims.get("exp").and_then(|v| v.as_i64()))
        });
        let expired_at = expires_secs
            .and_then(|secs| DateTime::from_timestamp(secs, 0))
            .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Millis, true))
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "Token 缺少有效期（exp），无法写入 Trae 登录态；请重新授权或粘贴完整 JWT"
                    .into(),
            })?;

        let mut account = Map::new();
        account.insert(
            "username".into(),
            Value::String(cred.display_name.clone().unwrap_or_default()),
        );
        account.insert(
            "email".into(),
            Value::String(cred.display_email.clone().unwrap_or_default()),
        );
        if self.is_cn {
            account.insert("scope".into(), Value::String("marscode".into()));
            account.insert("loginScope".into(), Value::String("trae".into()));
            account.insert("userTag".into(), Value::String("cn".into()));
            account.insert("storeRegion".into(), Value::String("CN".into()));
            account.insert("migrateToSG".into(), Value::Bool(false));
        }

        let mut payload = Map::new();
        payload.insert("token".into(), Value::String(access_token.to_string()));
        if let Some(refresh) = refresh_token {
            payload.insert("refreshToken".into(), Value::String(refresh.to_string()));
        }
        payload.insert("expiredAt".into(), Value::String(expired_at));
        payload.insert("userId".into(), Value::String(cred.uid.clone()));
        payload.insert(
            "host".into(),
            Value::String(self.base_api_url().to_string()),
        );
        if self.is_cn {
            payload.insert(
                "userRegion".into(),
                serde_json::json!({ "region": "CN", "_aiRegion": "CN" }),
            );
        }
        payload.insert("account".into(), Value::Object(account));

        Ok(Value::Object(payload))
    }

    fn collect_server_entries(root: &Value) -> Map<String, Value> {
        root.as_object()
            .map(|obj| {
                obj.iter()
                    .filter(|(k, _)| k.starts_with(SERVER_DATA_PREFIX))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 跨账号切换时 `iCubeServerData://*`（套餐/签到/活动缓存）必须随账号替换，
    /// 否则新账号启动时会先展示上一个账号的套餐与签到状态。
    fn apply_server_entries(
        root_obj: &mut Map<String, Value>,
        entries: Option<Map<String, Value>>,
    ) {
        root_obj.retain(|k, _| !k.starts_with(SERVER_DATA_PREFIX));
        if let Some(entries) = entries {
            for (k, v) in entries {
                root_obj.insert(k, v);
            }
        }
    }
}

#[async_trait]
impl ToolAdapter for TraeAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    fn daily_claim_device_key(&self) -> Option<String> {
        if !self.is_cn {
            return None;
        }
        let device = self.device_headers()?;
        let mut hasher = Sha256::new();
        hasher.update(device.device_id.as_bytes());
        Some(hex::encode(hasher.finalize()))
    }

    async fn import_from_local(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let config_dir =
            host.client_config_dir(self.tool_id())
                .map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: e.to_string(),
                })?;
        let storage_path = config_dir.join(STORAGE_JSON);
        if !storage_path.is_file() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::ClientNotFound,
                message: format!("未找到 Trae 会话文件: {}", storage_path.display()),
            });
        }

        let raw = std::fs::read(&storage_path).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("读取 Trae storage.json 失败: {e}"),
        })?;
        let storage_val: Value =
            serde_json::from_slice(&raw).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("解析 Trae storage.json 失败: {e}"),
            })?;

        let auth_entries = Self::parse_auth_entries_from_storage(&storage_val)?;
        let (uid, name, email, token_opt) =
            Self::extract_identity_from_auth_entries(self.tool_id(), &auth_entries);

        let entries_bytes =
            serde_json::to_vec(&auth_entries).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: e.to_string(),
            })?;

        let mut files = HashMap::new();
        files.insert(SNAPSHOT_AUTH_KEY.to_string(), BASE64.encode(entries_bytes));
        if let Some(tok) = token_opt {
            files.insert(
                DECRYPTED_TOKEN_FILE.to_string(),
                BASE64.encode(tok.as_bytes()),
            );
        }

        let vscdb_path = config_dir.join(STATE_VSCDB);
        if vscdb_path.is_file() {
            if let Some(vscdb_entries) = Self::read_vscdb_auth_entries(&vscdb_path) {
                if let Ok(vscdb_bytes) = serde_json::to_vec(&vscdb_entries) {
                    files.insert(SNAPSHOT_VSCDB_KEY.to_string(), BASE64.encode(vscdb_bytes));
                }
            }
        }

        let server_entries = Self::collect_server_entries(&storage_val);
        if !server_entries.is_empty() {
            if let Ok(server_bytes) = serde_json::to_vec(&server_entries) {
                files.insert(
                    SNAPSHOT_SERVERDATA_KEY.to_string(),
                    BASE64.encode(server_bytes),
                );
            }
        }

        Ok(CredentialEnvelope {
            tool_id: self.tool_id().to_string(),
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
                message: "输入 Token 不能为空".into(),
            });
        }

        if trimmed.starts_with('{') {
            let val: Value = serde_json::from_str(trimmed).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("JSON 格式无效: {e}"),
            })?;

            if let Some(obj) = val.as_object() {
                if obj.keys().any(|k| k.starts_with("iCubeAuthInfo://")) {
                    let entries = Self::parse_auth_entries_from_storage(&val)?;
                    let (uid, name, email, token_opt) =
                        Self::extract_identity_from_auth_entries(self.tool_id(), &entries);
                    let entries_bytes =
                        serde_json::to_vec(&entries).map_err(|e| AdapterError::Unified {
                            code: UnifiedErrorCode::InternalError,
                            message: e.to_string(),
                        })?;
                    let mut files = HashMap::new();
                    files.insert(SNAPSHOT_AUTH_KEY.to_string(), BASE64.encode(entries_bytes));
                    if let Some(tok) = token_opt {
                        files.insert(
                            DECRYPTED_TOKEN_FILE.to_string(),
                            BASE64.encode(tok.as_bytes()),
                        );
                    }
                    let server_entries = Self::collect_server_entries(&val);
                    if !server_entries.is_empty() {
                        if let Ok(server_bytes) = serde_json::to_vec(&server_entries) {
                            files.insert(
                                SNAPSHOT_SERVERDATA_KEY.to_string(),
                                BASE64.encode(server_bytes),
                            );
                        }
                    }
                    return Ok(CredentialEnvelope {
                        tool_id: self.tool_id().to_string(),
                        uid,
                        display_name: name,
                        display_email: email,
                        payload: CredentialPayload::LocalSnapshot { files },
                    });
                }
            }

            let token = val
                .get("token")
                .or_else(|| val.get("accessToken"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "JSON 中缺少 token 字段".into(),
                })?;

            let claims = Self::extract_jwt_claims(token);
            let uid = val
                .get("userId")
                .or_else(|| val.get("uid"))
                .or_else(|| val.get("user_id"))
                .and_then(crate::adapter::common::json_uid_to_string)
                .or_else(|| {
                    claims.as_ref().and_then(|c| {
                        c.get("userId")
                            .or_else(|| c.get("sub"))
                            .or_else(|| c.get("uid"))
                            .and_then(crate::adapter::common::json_uid_to_string)
                    })
                })
                .unwrap_or_else(|| Self::hash_fallback_uid(self.tool_id(), token));

            let email = val
                .get("email")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    claims
                        .as_ref()
                        .and_then(|c| c.get("email").and_then(|v| v.as_str()))
                        .map(|s| s.to_string())
                });

            let name = val
                .get("name")
                .or_else(|| val.get("nickName"))
                .or_else(|| val.get("nickname"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    claims.as_ref().and_then(|c| {
                        c.get("name")
                            .or_else(|| c.get("nickname"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                    })
                });

            return Ok(CredentialEnvelope {
                tool_id: self.tool_id().to_string(),
                uid,
                display_name: name,
                display_email: email,
                payload: CredentialPayload::Token {
                    access_token: token.to_string(),
                    refresh_token: val
                        .get("refreshToken")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    expires_at: claims
                        .as_ref()
                        .and_then(|c| c.get("exp").and_then(|v| v.as_i64())),
                    extra: HashMap::new(),
                },
            });
        }

        let clean_token = trimmed
            .strip_prefix("Cloud-IDE-JWT ")
            .or_else(|| trimmed.strip_prefix("Bearer "))
            .unwrap_or(trimmed)
            .trim()
            .to_string();
        if clean_token.is_empty() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "输入 Token 不能为空".into(),
            });
        }

        let claims = Self::extract_jwt_claims(&clean_token);
        let uid = claims
            .as_ref()
            .and_then(|c| {
                c.get("userId")
                    .or_else(|| c.get("sub"))
                    .or_else(|| c.get("uid"))
                    .and_then(crate::adapter::common::json_uid_to_string)
            })
            .unwrap_or_else(|| Self::hash_fallback_uid(self.tool_id(), &clean_token));

        let email = claims
            .as_ref()
            .and_then(|c| c.get("email").and_then(|v| v.as_str()))
            .map(|s| s.to_string());

        let name = claims.as_ref().and_then(|c| {
            c.get("name")
                .or_else(|| c.get("nickname"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });

        let expires_at = claims
            .as_ref()
            .and_then(|c| c.get("exp").and_then(|v| v.as_i64()));

        Ok(CredentialEnvelope {
            tool_id: self.tool_id().to_string(),
            uid,
            display_name: name,
            display_email: email,
            payload: CredentialPayload::Token {
                access_token: clean_token,
                refresh_token: None,
                expires_at,
                extra: HashMap::new(),
            },
        })
    }

    async fn inspect_quota(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<QuotaSnapshot, AdapterError> {
        let (token_opt, is_encrypted_local) = self.extract_token_from_cred(cred);

        if let Some(token) = token_opt {
            if let Some(claims) = Self::extract_jwt_claims(&token) {
                if let Some(exp) = claims.get("exp").and_then(|v| v.as_i64()) {
                    if exp <= Utc::now().timestamp() {
                        return Err(AdapterError::Unified {
                            code: UnifiedErrorCode::AuthExpired,
                            message: "Trae Token 已过期，请重新登录或导入".into(),
                        });
                    }
                }
            }

            let mut quota = self.fetch_entitlement_quota(&token).await?;
            if self.is_cn {
                match self.fetch_checkin_status(&token).await {
                    Ok((cs, _)) => quota.claim_status = cs,
                    Err(
                        e @ AdapterError::Unified {
                            code: UnifiedErrorCode::AuthExpired,
                            ..
                        },
                    ) => {
                        return Err(e);
                    }
                    Err(
                        e @ AdapterError::Unified {
                            code: UnifiedErrorCode::RateLimited | UnifiedErrorCode::RiskTriggered,
                            ..
                        },
                    ) => {
                        return Err(e);
                    }
                    Err(_) => quota.claim_status = ClaimStatus::Unknown,
                }
            }
            return Ok(quota);
        }

        Err(AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: if is_encrypted_local {
                "本地加密会话未能提取在线 Token，请点击「在线授权」以开启查额与签到".into()
            } else {
                "缺少可用于查额的在线 Token，请点击「在线授权」以开启查额与签到".into()
            },
        })
    }

    async fn claim_daily(
        &self,
        cred: &CredentialEnvelope,
        precheck: Option<&QuotaSnapshot>,
    ) -> Result<ClaimResult, AdapterError> {
        if !self.is_cn {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::UnsupportedCapability,
                message: "Trae 国际版不支持每日签到领取".into(),
            });
        }

        let (token_opt, _) = self.extract_token_from_cred(cred);
        let token = token_opt.ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "缺少可用于签到的 Trae Token".into(),
        })?;

        // 调度器已完成幂等前置查时直接复用快照，避免重复请求平台
        let (current_status, credits_hint) = match precheck {
            Some(q) => (q.claim_status.clone(), None),
            None => self.fetch_checkin_status(&token).await?,
        };
        if current_status == ClaimStatus::Claimed {
            return Ok(ClaimResult {
                claimed: false,
                reward_summary: "今日已签过".into(),
                updated_quota: match precheck {
                    Some(q) => Some(q.clone()),
                    None => self.inspect_quota(cred).await.ok(),
                },
            });
        }
        if current_status == ClaimStatus::Unsupported {
            return Ok(ClaimResult {
                claimed: false,
                reward_summary: "当前账号暂不支持每日签到".into(),
                updated_quota: None,
            });
        }
        if current_status != ClaimStatus::Available {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: "Trae CN 签到状态未知，为避免重复请求已中止领取".into(),
            });
        }

        let url = format!(
            "{}/trae/api/v2/ug/checkin_credits/claim",
            self.base_api_url()
        );
        let headers = self.build_api_headers(&token)?;
        // 签到下单强依赖设备标识：缺少时请求必然返回 9004，提前给出可操作提示
        if !headers.contains_key("x-device-id") {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::ClientNotFound,
                message:
                    "未检测到本机 Trae CN 设备标识，无法签到；请先在本机登录一次 Trae CN 客户端后重试"
                        .into(),
            });
        }
        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(&serde_json::json!({ "req_source": 1 }))
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("Trae CN 每日签到失败: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            // 400/409 可能承载"今日已签"幂等响应，先识别再走通用错误
            if status.as_u16() == 400 || status.as_u16() == 409 {
                let err_text = resp.text().await.unwrap_or_default();
                if crate::adapter::common::is_idempotent_claimed_phrase(&err_text) {
                    let mut refreshed = match self.inspect_quota(cred).await {
                        Ok(q) => q,
                        Err(_) => precheck.cloned().unwrap_or(QuotaSnapshot {
                            plan_name: None,
                            claim_status: ClaimStatus::Claimed,
                            buckets: Vec::new(),
                            fetched_at: Utc::now().to_rfc3339(),
                        }),
                    };
                    refreshed.claim_status = ClaimStatus::Claimed;
                    return Ok(ClaimResult {
                        claimed: false,
                        reward_summary: "今日已签过".into(),
                        updated_quota: Some(refreshed),
                    });
                }
            }
            return Err(Self::map_http_status(status, "Trae CN 每日签到"));
        }

        let body: Value = resp.json().await.map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("解析 Trae CN 签到响应失败: {e}"),
        })?;
        let Some(code) = body.get("code").and_then(|v| v.as_i64()) else {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: "Trae CN 签到响应缺少状态码，请刷新核实".into(),
            });
        };
        if code != 0 {
            // 9095：同一设备每天仅能签到一次，其他账号再签会命中该业务码
            if code == CLAIM_CODE_DEVICE_ALREADY_CHECKED_IN {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::DeviceDailyLimitReached,
                    message: "本机今日已使用 Trae CN 签到额度；当前账号未领取积分".into(),
                });
            }
            if let Some(err) = crate::adapter::common::map_business_code(code, "Trae CN 签到") {
                return Err(err);
            }
            let msg = body
                .get("message")
                .or_else(|| body.get("msg"))
                .and_then(|v| v.as_str())
                .unwrap_or("签到未成功");
            if crate::adapter::common::is_idempotent_claimed_phrase(msg) {
                let mut refreshed = match self.inspect_quota(cred).await {
                    Ok(q) => q,
                    Err(_) => precheck.cloned().unwrap_or(QuotaSnapshot {
                        plan_name: None,
                        claim_status: ClaimStatus::Claimed,
                        buckets: Vec::new(),
                        fetched_at: Utc::now().to_rfc3339(),
                    }),
                };
                refreshed.claim_status = ClaimStatus::Claimed;
                return Ok(ClaimResult {
                    claimed: false,
                    reward_summary: "今日已签过".into(),
                    updated_quota: Some(refreshed),
                });
            }
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("Trae CN 签到返回异常 ({code}): {msg}"),
            });
        }

        let gained_opt = body
            .pointer("/data/credits")
            .or_else(|| body.get("credits"))
            .and_then(|v| v.as_i64())
            .or(credits_hint);
        let refreshed = self.inspect_quota(cred).await.ok();
        let reward_summary = match gained_opt {
            Some(g) => format!("成功领取 {g} Credits"),
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
        let (auth_entries, server_entries, snapshot_vscdb): (
            Map<String, Value>,
            Option<Map<String, Value>>,
            Option<Map<String, Value>>,
        ) = match &cred.payload {
            CredentialPayload::LocalSnapshot { files } => {
                let b64 = files
                    .get(SNAPSHOT_AUTH_KEY)
                    .ok_or_else(|| AdapterError::Unified {
                        code: UnifiedErrorCode::InvalidCredential,
                        message: "快照中缺少 Trae 认证条目".into(),
                    })?;
                let bytes = BASE64.decode(b64).map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: format!("快照解码失败: {e}"),
                })?;
                let entries: Map<String, Value> =
                    serde_json::from_slice(&bytes).map_err(|e| AdapterError::Unified {
                        code: UnifiedErrorCode::InvalidCredential,
                        message: format!("快照反序列化失败: {e}"),
                    })?;
                let server = files
                    .get(SNAPSHOT_SERVERDATA_KEY)
                    .and_then(|b64| BASE64.decode(b64).ok())
                    .and_then(|bytes| serde_json::from_slice::<Map<String, Value>>(&bytes).ok());
                let vscdb = files
                    .get(SNAPSHOT_VSCDB_KEY)
                    .and_then(|b64| BASE64.decode(b64).ok())
                    .and_then(|bytes| serde_json::from_slice::<Map<String, Value>>(&bytes).ok());
                (entries, server, vscdb)
            }
            CredentialPayload::Token {
                access_token,
                refresh_token,
                expires_at,
                ..
            } => {
                let payload = self.build_token_auth_entry(
                    cred,
                    access_token,
                    refresh_token.as_deref(),
                    *expires_at,
                )?;
                let entry_val = if self.is_cn {
                    Self::encrypt_tc_string(&payload.to_string())
                        .unwrap_or_else(|| payload.to_string())
                } else {
                    payload.to_string()
                };
                let mut map = Map::new();
                map.insert(PRIMARY_AUTH_KEY.to_string(), Value::String(entry_val));
                (map, None, None)
            }
            CredentialPayload::Cookies { .. } => {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::UnsupportedCapability,
                    message: "Trae 不支持 Cookie 会话切换".into(),
                });
            }
        };

        let config_dir =
            host.client_config_dir(self.tool_id())
                .map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: e.to_string(),
                })?;
        std::fs::create_dir_all(&config_dir).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("创建 Trae 配置目录失败: {e}"),
        })?;

        let storage_path = config_dir.join(STORAGE_JSON);
        let mut root_obj = if storage_path.is_file() {
            let existing = std::fs::read(&storage_path).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("读取现有 Trae storage.json 失败，已中止切号以保护原配置: {e}"),
            })?;
            let parsed =
                serde_json::from_slice::<Value>(&existing).map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: format!(
                        "解析现有 Trae storage.json 失败，已中止切号以保护原配置: {e}"
                    ),
                })?;
            parsed
                .as_object()
                .cloned()
                .ok_or_else(|| AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: "现有 Trae storage.json 根节点不是 JSON 对象，已中止切号".into(),
                })?
        } else {
            Map::new()
        };

        // 仅清理并替换账号级登录态；设备密钥（icube-dc）与 usertag 属于本机，
        // Token 导入的账号没有这两类快照，需保留以免破坏设备绑定
        let preserved: Vec<(String, Value)> = root_obj
            .iter()
            .filter(|(k, _)| k.starts_with(DEVICE_CRED_PREFIX) || k.as_str() == USERTAG_KEY)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        root_obj.retain(|k, _| !k.starts_with("iCubeAuthInfo://"));
        for (k, v) in preserved {
            if !auth_entries.contains_key(&k) {
                root_obj.insert(k, v);
            }
        }
        for (k, v) in &auth_entries {
            root_obj.insert(k.clone(), v.clone());
        }
        Self::apply_server_entries(&mut root_obj, server_entries);

        let serialized =
            serde_json::to_vec_pretty(&root_obj).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: e.to_string(),
            })?;

        let tmp_path = config_dir.join(".storage.json.tmp");
        if let Err(e) = std::fs::write(&tmp_path, serialized) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("写入临时 storage.json 失败: {e}"),
            });
        }
        if let Err(e) = std::fs::rename(&tmp_path, &storage_path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("替换 Trae storage.json 失败: {e}"),
            });
        }

        // 以最终 storage.json 中的账号态为准覆盖 vscdb，同时保留快照里的历史条目
        let mut vscdb_entries = snapshot_vscdb.unwrap_or_default();
        for (k, v) in root_obj
            .iter()
            .filter(|(k, _)| k.starts_with("iCubeAuthInfo://"))
        {
            vscdb_entries.insert(k.clone(), v.clone());
        }
        Self::apply_vscdb_auth_entries(&config_dir.join(STATE_VSCDB), &vscdb_entries)?;

        Ok(())
    }

    async fn backup_session(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<crate::adapter::SessionBackup, AdapterError> {
        let config_dir =
            host.client_config_dir(self.tool_id())
                .map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: e.to_string(),
                })?;
        crate::adapter::backup_files(&[
            config_dir.join(STORAGE_JSON),
            config_dir.join(STATE_VSCDB),
            config_dir.join(format!("{STATE_VSCDB}-wal")),
            config_dir.join(format!("{STATE_VSCDB}-shm")),
            config_dir.join(format!("{STATE_VSCDB}-journal")),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHost(std::path::PathBuf);

    #[async_trait]
    impl PlatformHost for TestHost {
        fn qwitch_data_dir(&self) -> Result<std::path::PathBuf, crate::platform::HostError> {
            Ok(self.0.clone())
        }
        fn client_config_dir(
            &self,
            _tool_id: &str,
        ) -> Result<std::path::PathBuf, crate::platform::HostError> {
            Ok(self.0.clone())
        }
        fn detect_client_executable(&self, _tool_id: &str) -> Option<std::path::PathBuf> {
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
            _executable: &std::path::Path,
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

    #[test]
    fn trae_checkin_parser_is_fail_closed_when_fields_missing() {
        let missing = serde_json::json!({ "code": 0, "data": { "enable": true } });
        let (cs, _) = TraeAdapter::parse_checkin_status_from_body(&missing).unwrap();
        assert_eq!(cs, ClaimStatus::Unknown);

        let avail = serde_json::json!({ "code": 0, "data": { "checked_in": false, "enable": true, "credits": 50 } });
        let (cs2, cr2) = TraeAdapter::parse_checkin_status_from_body(&avail).unwrap();
        assert_eq!(cs2, ClaimStatus::Available);
        assert_eq!(cr2, Some(50));

        let disabled = serde_json::json!({ "code": 0, "data": { "enable": false } });
        let (cs3, _) = TraeAdapter::parse_checkin_status_from_body(&disabled).unwrap();
        assert_eq!(cs3, ClaimStatus::Unsupported);

        let err_body = serde_json::json!({ "code": 401, "message": "Token expired" });
        assert!(matches!(
            TraeAdapter::parse_checkin_status_from_body(&err_body),
            Err(AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn trae_corrupt_storage_json_aborts_apply_session_and_missing_token_errors() {
        let adapter = TraeAdapter::new(true);
        assert!(adapter
            .manifest()
            .capabilities
            .auth_methods
            .contains(&AuthMethod::TokenPaste));

        // 无法解出 Token 时不伪造 100% session_health 快照
        let mut files = HashMap::new();
        files.insert(
            SNAPSHOT_AUTH_KEY.to_string(),
            BASE64.encode(br#"{"iCubeAuthInfo://icube.cloudide":"encrypted-unreadable"}"#),
        );
        let no_tok_cred = CredentialEnvelope {
            tool_id: "trae_cn".to_string(),
            uid: "trae_cn_123".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::LocalSnapshot { files },
        };
        assert!(matches!(
            adapter.inspect_quota(&no_tok_cred).await,
            Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                ..
            })
        ));

        // parse_manual_token 支持数字型 userId
        let numeric_json = serde_json::json!({ "token": "tok-abc-123", "userId": 424242 });
        let parsed = adapter
            .parse_manual_token(&numeric_json.to_string())
            .await
            .expect("numeric userId");
        assert_eq!(parsed.uid, "424242");

        // 现有 storage.json 损坏时不得覆盖为 {}
        let dir = std::env::temp_dir().join(format!("qwitch_trae_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(STORAGE_JSON), b"{corrupt json").unwrap();
        let host = TestHost(dir.clone());
        let tok_cred = CredentialEnvelope {
            tool_id: "trae_cn".to_string(),
            uid: "u_1".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "tok-1".to_string(),
                refresh_token: None,
                // 有效期内才进入 storage.json 读取路径，保持"损坏配置中止切号"的测试意图
                expires_at: Some(Utc::now().timestamp() + 3600),
                extra: HashMap::new(),
            },
        };
        assert!(adapter.apply_session(&host, &tok_cred).await.is_err());
        assert_eq!(
            std::fs::read(dir.join(STORAGE_JSON)).unwrap(),
            b"{corrupt json"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trae_token_auth_entry_matches_client_required_fields() {
        let adapter = TraeAdapter::new(true);
        let cred = CredentialEnvelope {
            tool_id: "trae_cn".to_string(),
            uid: "3704729780097880".to_string(),
            display_name: Some("lemontree".to_string()),
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjE3OTYwMDU1MzR9.sig".to_string(),
                refresh_token: None,
                expires_at: Some(1796005534),
                extra: HashMap::new(),
            },
        };
        let entry = adapter
            .build_token_auth_entry(
                &cred,
                "eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjE3OTYwMDU1MzR9.sig",
                None,
                Some(1796005534),
            )
            .expect("token entry");
        // 客户端以 expiredAt 判定是否已登录，缺失会直接回到登录页
        assert_eq!(
            entry.get("expiredAt").and_then(|v| v.as_str()),
            Some("2026-11-30T02:25:34.000Z")
        );
        assert_eq!(
            entry.pointer("/account/scope").and_then(|v| v.as_str()),
            Some("marscode")
        );
        assert_eq!(
            entry
                .pointer("/userRegion/_aiRegion")
                .and_then(|v| v.as_str()),
            Some("CN")
        );
        assert_eq!(
            entry.get("host").and_then(|v| v.as_str()),
            Some("https://api.trae.cn")
        );

        // 无 exp 的 Token 必须失败，避免写入客户端无法识别的登录态
        let no_exp = adapter
            .build_token_auth_entry(&cred, "opaque-token", None, None)
            .unwrap_err();
        assert!(matches!(
            no_exp,
            AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                ..
            }
        ));
    }

    #[test]
    fn trae_device_id_and_server_entries_helpers() {
        let dir = std::env::temp_dir().join(format!("qwitch_trae_dev_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = serde_json::json!({
            "iCubeAuthInfo://icube-dc:93900972767536": "dc",
            "iCubeAuthInfo://icube.cloudide": "auth",
            "iCubeServerData://icube.cloudide": "{\"entitlementInfo\":{}}",
            "iCubeLastVersion": "3.4.22"
        });
        std::fs::write(dir.join(STORAGE_JSON), storage.to_string()).unwrap();
        assert_eq!(
            TraeAdapter::device_id_from_storage(&dir.join(STORAGE_JSON)).as_deref(),
            Some("93900972767536")
        );

        let root = serde_json::from_value::<Value>(storage).unwrap();
        let server = TraeAdapter::collect_server_entries(&root);
        assert_eq!(server.len(), 1);
        assert!(server.contains_key("iCubeServerData://icube.cloudide"));

        let mut merged = Map::new();
        merged.insert(
            "iCubeServerData://icube.cloudide".to_string(),
            Value::String("stale".into()),
        );
        merged.insert("keep".to_string(), Value::Bool(true));
        TraeAdapter::apply_server_entries(&mut merged, Some(server));
        assert_eq!(
            merged.get("iCubeServerData://icube.cloudide"),
            root.get("iCubeServerData://icube.cloudide")
        );
        assert_eq!(merged.get("keep"), Some(&Value::Bool(true)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trae_device_headers_resolve_from_local_storage() {
        let dir = std::env::temp_dir().join(format!("qwitch_trae_hdr_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(STORAGE_JSON),
            serde_json::json!({
                "iCubeAuthInfo://icube-dc:93900972767536": "dc",
                "iCubeAuthInfo://icube.cloudide": "auth"
            })
            .to_string(),
        )
        .unwrap();

        let adapter = TraeAdapter::with_host(true, Arc::new(TestHost(dir.clone())));
        let headers = adapter.build_api_headers("tok").expect("headers");
        assert_eq!(
            headers.get("x-device-id").and_then(|v| v.to_str().ok()),
            Some("93900972767536")
        );
        assert_eq!(
            headers.get("x-device-type").and_then(|v| v.to_str().ok()),
            Some(TraeAdapter::device_type_tag())
        );
        assert_eq!(
            headers.get("x-user-region").and_then(|v| v.to_str().ok()),
            Some("CN")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn trae_token_switch_writes_complete_session_and_preserves_device_entries() {
        let dir = std::env::temp_dir().join(format!("qwitch_trae_switch_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let existing = serde_json::json!({
            "iCubeAuthInfo://icube-dc:93900972767536": "dc-keep",
            "iCubeAuthInfo://usertag": "usertag-keep",
            "iCubeAuthInfo://icube.cloudide": "old-primary",
            "iCubeServerData://icube.cloudide": "stale-server",
            "editor.fontSize": 14
        });
        std::fs::write(dir.join(STORAGE_JSON), existing.to_string()).unwrap();

        let adapter = TraeAdapter::new(true);
        let cred = CredentialEnvelope {
            tool_id: "trae_cn".to_string(),
            uid: "3704729780097880".to_string(),
            display_name: Some("lemontree".to_string()),
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjE3OTYwMDU1MzR9.sig".to_string(),
                refresh_token: None,
                expires_at: Some(1796005534),
                extra: HashMap::new(),
            },
        };
        adapter
            .apply_session(&TestHost(dir.clone()), &cred)
            .await
            .expect("token switch");

        let root: Value = serde_json::from_slice(&std::fs::read(dir.join(STORAGE_JSON)).unwrap())
            .expect("storage json");
        // 设备密钥与 usertag 保留，账号级登录态被替换
        assert_eq!(
            root.get("iCubeAuthInfo://icube-dc:93900972767536"),
            Some(&Value::String("dc-keep".into()))
        );
        assert_eq!(
            root.get("iCubeAuthInfo://usertag"),
            Some(&Value::String("usertag-keep".into()))
        );
        // 旧账号的套餐/签到缓存必须清掉，避免新账号启动时展示旧数据
        assert!(root.get("iCubeServerData://icube.cloudide").is_none());
        assert_eq!(root.get("editor.fontSize"), Some(&Value::from(14)));

        let primary = root
            .get(PRIMARY_AUTH_KEY)
            .and_then(|v| v.as_str())
            .expect("primary auth");
        let decoded = TraeAdapter::decrypt_tc_string(primary).expect("decrypt auth entry");
        assert_eq!(
            decoded.get("expiredAt").and_then(|v| v.as_str()),
            Some("2026-11-30T02:25:34.000Z")
        );
        assert_eq!(
            decoded.pointer("/account/scope").and_then(|v| v.as_str()),
            Some("marscode")
        );
        assert_eq!(
            decoded.get("userId").and_then(|v| v.as_str()),
            Some("3704729780097880")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
