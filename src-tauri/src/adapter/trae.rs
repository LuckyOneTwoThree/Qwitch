use aes::cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes128;
use async_trait::async_trait;
use base64::{
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD as BASE64_URL},
    Engine as _,
};
use chrono::Utc;
use rand::RngCore;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256, Sha512};
use std::collections::HashMap;
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
const DECRYPTED_TOKEN_FILE: &str = "decrypted_token.txt";
const PRIMARY_AUTH_KEY: &str = "iCubeAuthInfo://icube.cloudide";

pub struct TraeAdapter {
    is_cn: bool,
    manifest: AdapterManifest,
    http: reqwest::Client,
}

impl TraeAdapter {
    pub fn new(is_cn: bool) -> Self {
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
                    auth_methods: vec![AuthMethod::LocalImport, AuthMethod::WebviewOauth],
                    supports_quota_query: true,
                    supports_daily_claim: is_cn,
                    supports_client_switch: true,
                    supports_multi_instance: false,
                },
            },
            http,
        }
    }

    fn tool_id(&self) -> &str {
        if self.is_cn {
            "trae_cn"
        } else {
            "trae"
        }
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
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
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
}

#[async_trait]
impl ToolAdapter for TraeAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
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
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    claims.as_ref().and_then(|c| {
                        c.get("userId")
                            .or_else(|| c.get("sub"))
                            .or_else(|| c.get("uid"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
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

        let claims = Self::extract_jwt_claims(&clean_token);
        let uid = claims
            .as_ref()
            .and_then(|c| {
                c.get("userId")
                    .or_else(|| c.get("sub"))
                    .or_else(|| c.get("uid"))
                    .and_then(|v| v.as_str())
            })
            .map(|s| s.to_string())
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

        let label = if is_encrypted_local {
            "本地加密会话（支持冷切换）"
        } else {
            "会话有效（未能直连额度接口）"
        };

        Ok(QuotaSnapshot {
            plan_name: None,
            claim_status: ClaimStatus::Unsupported,
            buckets: vec![QuotaBucket {
                key: "session_health".into(),
                label: label.into(),
                used: 0.0,
                total: 100.0,
                remaining: 100.0,
                unit: "%".into(),
                reset_at: None,
            }],
            fetched_at: Utc::now().to_rfc3339(),
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
            if let Some(err) = crate::adapter::common::map_business_code(code, "Trae CN 签到") {
                return Err(err);
            }
            let msg = body
                .get("message")
                .or_else(|| body.get("msg"))
                .and_then(|v| v.as_str())
                .unwrap_or("签到未成功");
            let lower = msg.to_lowercase();
            if msg.contains("已签")
                || msg.contains("重复")
                || lower.contains("already")
                || lower.contains("checked")
            {
                let refreshed = self.inspect_quota(cred).await.ok();
                return Ok(ClaimResult {
                    claimed: false,
                    reward_summary: "今日已签过".into(),
                    updated_quota: refreshed,
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
        let auth_entries: Map<String, Value> = match &cred.payload {
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
                serde_json::from_slice(&bytes).map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: format!("快照反序列化失败: {e}"),
                })?
            }
            CredentialPayload::Token {
                access_token,
                refresh_token,
                ..
            } => {
                let mut map = Map::new();
                let payload_json = serde_json::json!({
                    "token": access_token,
                    "refreshToken": refresh_token,
                    "userId": cred.uid,
                    "host": self.base_api_url(),
                    "account": {
                        "username": cred.display_name.as_deref().unwrap_or(""),
                        "email": cred.display_email.as_deref().unwrap_or(""),
                    }
                });
                let entry_val = if self.is_cn {
                    Self::encrypt_tc_string(&payload_json.to_string())
                        .unwrap_or_else(|| payload_json.to_string())
                } else {
                    payload_json.to_string()
                };
                map.insert(PRIMARY_AUTH_KEY.to_string(), Value::String(entry_val));
                map
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
            let existing = std::fs::read(&storage_path).unwrap_or_default();
            serde_json::from_slice::<Value>(&existing)
                .ok()
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default()
        } else {
            Map::new()
        };

        // 仅清理并替换 iCubeAuthInfo://* 键，保留用户 IDE 主题与窗口设置
        root_obj.retain(|k, _| !k.starts_with("iCubeAuthInfo://"));
        for (k, v) in &auth_entries {
            root_obj.insert(k.clone(), v.clone());
        }

        let serialized = serde_json::to_vec_pretty(&Value::Object(root_obj)).map_err(|e| {
            AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: e.to_string(),
            }
        })?;

        let tmp_path = config_dir.join(".storage.json.tmp");
        std::fs::write(&tmp_path, serialized).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("写入临时 storage.json 失败: {e}"),
        })?;
        std::fs::rename(&tmp_path, &storage_path).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("替换 Trae storage.json 失败: {e}"),
        })?;

        let vscdb_entries = if let CredentialPayload::LocalSnapshot { files } = &cred.payload {
            files
                .get(SNAPSHOT_VSCDB_KEY)
                .and_then(|b64| BASE64.decode(b64).ok())
                .and_then(|bytes| serde_json::from_slice::<Map<String, Value>>(&bytes).ok())
                .unwrap_or_else(|| auth_entries.clone())
        } else {
            auth_entries
        };
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
        Ok(crate::adapter::backup_files(&[
            config_dir.join(STORAGE_JSON),
            config_dir.join(STATE_VSCDB),
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
