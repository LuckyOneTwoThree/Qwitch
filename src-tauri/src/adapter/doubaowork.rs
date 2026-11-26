use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{TimeZone, Utc};
use reqwest::header::{ACCEPT, CONTENT_TYPE, COOKIE, REFERER, USER_AGENT};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::adapter::common::{
    is_idempotent_claimed_phrase, json_uid_to_string, map_business_code, map_http_status,
    merge_post_claim_quota, shared_http_client,
};
use crate::adapter::{
    write_private_file_atomic, AdapterCapabilities, AdapterError, AdapterManifest, ClaimResult,
    SessionBackup, ToolAdapter,
};
use crate::domain::{
    AuthMethod, ClaimStatus, CredentialEnvelope, CredentialPayload, QuotaBucket, QuotaSnapshot,
    UnifiedErrorCode,
};
use crate::platform::PlatformHost;

const DEFAULT_WEB_BASE: &str = "https://www.doubao.com";
const DEFAULT_ACCOUNTS_BASE: &str = "https://accounts.doubao.com";
const AID: &str = "1044603";
const DOUBAO_BASE_QUERY_PARAMS: &[(&str, &str)] = &[
    ("aid", "1044603"),
    ("real_aid", "1044603"),
    ("device_platform", "web"),
    ("doubao_device_platform", "desktop"),
    ("web_platform", "desktop"),
    ("pkg_type", "release_version"),
    ("version_code", "20800"),
    ("pc_version", "2.37.19"),
    ("doubao_pc_version", "2.37.19"),
    // 与官方 `DoubaoWork.app` 2.37.19 的 `UniParamsInit` 对齐：main.js 里
    // `runtime: () => uh.web` / `runtime_version: () => "3.46.7"`，并非 local_web。
    ("runtime", "web"),
    ("runtime_version", "3.46.7"),
    ("language", "zh"),
    ("region", "CN"),
    ("sys_region", "CN"),
    ("samantha_web", "1"),
    ("use-olympus-account", "1"),
];
const DEFAULT_CHECKIN_SCENE: &str = "pc_incentive_task_mvp_v3";
/// 豆包探索计划的 campaign group（客户端 `async/59729.js` 中 `d.$`）。
const CAMPAIGN_GROUP_CODE: &str = "pc_incentive_task_mvp_v3";
/// 每日签到在 campaign 内的 item 编码（客户端 `daily_signin` 卡片 `task_code`）。
const DAILY_SIGNIN_ITEM_CODE: &str = "daily_signin";
/// 活动规则：单用户通过本活动可获得的专业版试用总天数上限为 20 天。
/// 账号持有超过该天数的活动赠送套餐时，服务端发放校验会失败（AGW 兜底码 710010702），
/// 因此本地直接判定为不可领，避免无意义地反复打写接口。
const CAMPAIGN_TRIAL_CAP_DAYS: i64 = 20;
const DOUBAO_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

const SNAPSHOT_ZIP_KEY: &str = "doubaowork_profile.zip";
const SNAPSHOT_TOKEN_FILE: &str = "decrypted_token.txt";

const MAX_SNAPSHOT_ZIP_BYTES: usize = 80 * 1024 * 1024;
const MAX_SNAPSHOT_ENTRIES: usize = 500;
const MAX_SNAPSHOT_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DIR_DEPTH: usize = 12;

const FIXED_SIDECARS: &[&str] = &[
    "Default/Cookies-journal",
    "Default/Cookies-wal",
    "Default/Cookies-shm",
    "Default/Network/Cookies-journal",
    "Default/Network/Cookies-wal",
    "Default/Network/Cookies-shm",
    "sdk_storage/common.db-wal",
    "sdk_storage/common.db-shm",
];

/// 豆包工作（DoubaoWork）内置适配器（双轨模式：本机快照切号 + WebView/Cookie 查额签到）。
pub struct DoubaoWorkAdapter {
    manifest: AdapterManifest,
    http: reqwest::Client,
    web_base: String,
    accounts_base: String,
    host: Option<Arc<dyn PlatformHost>>,
}

#[derive(Debug, Default)]
struct DoubaoDeviceParams {
    device_id: Option<String>,
    install_id: Option<String>,
    web_id: Option<String>,
}

fn sanitize_device_param(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty()
        || s.len() > 64
        || matches!(s, "0" | "-1")
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return None;
    }
    Some(s.to_string())
}

impl DoubaoWorkAdapter {
    pub fn new() -> Self {
        Self::with_bases(DEFAULT_WEB_BASE, DEFAULT_ACCOUNTS_BASE, None)
    }

    pub fn with_host(host: Arc<dyn PlatformHost>) -> Self {
        Self::with_bases(DEFAULT_WEB_BASE, DEFAULT_ACCOUNTS_BASE, Some(host))
    }

    #[cfg(test)]
    pub(crate) fn with_base_urls(
        web_base: impl Into<String>,
        accounts_base: impl Into<String>,
    ) -> Self {
        Self::with_bases(web_base, accounts_base, None)
    }

    #[cfg(test)]
    pub(crate) fn with_base_urls_and_host(
        web_base: impl Into<String>,
        accounts_base: impl Into<String>,
        host: Arc<dyn PlatformHost>,
    ) -> Self {
        Self::with_bases(web_base, accounts_base, Some(host))
    }

    fn with_bases(
        web_base: impl Into<String>,
        accounts_base: impl Into<String>,
        host: Option<Arc<dyn PlatformHost>>,
    ) -> Self {
        Self {
            manifest: AdapterManifest {
                id: "doubaowork".to_string(),
                name: "豆包工作".to_string(),
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
            http: shared_http_client().clone(),
            web_base: web_base.into().trim_end_matches('/').to_string(),
            accounts_base: accounts_base.into().trim_end_matches('/').to_string(),
            host,
        }
    }

    /// 校验 Cookie 串并查询豆包账号信息，返回可落盘的 `Token` 凭据信封。
    pub async fn verify_cookie_and_build_envelope(
        cookie_header: &str,
    ) -> Result<CredentialEnvelope, AdapterError> {
        Self::new().verify_cookie_with_base(cookie_header).await
    }

    pub(crate) async fn verify_cookie_with_base(
        &self,
        cookie_header: &str,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let normalized = normalize_cookie_header(cookie_header)?;
        let query_path = format!("/passport/account/info/v2/?aid={AID}&account_sdk_source=web");

        let body = match self
            .get_with_cookie(
                &self.web_base,
                &query_path,
                &normalized,
                "校验豆包工作登录态",
            )
            .await
        {
            Ok(v) => v,
            Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                ..
            }) => {
                self.get_with_cookie(
                    &self.accounts_base,
                    &query_path,
                    &normalized,
                    "校验豆包工作登录态",
                )
                .await?
            }
            Err(err) => return Err(err),
        };

        let message = body
            .get("message")
            .and_then(|v| v.as_str())
            .map(str::to_ascii_lowercase);
        let data = body.get("data").unwrap_or(&Value::Null);
        let passport_err = data.get("error_code").and_then(int_field).unwrap_or(0);

        if message.as_deref() == Some("error") || passport_err != 0 {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                message: "豆包工作 Cookie 未登录或已失效，请重新登录或粘贴最新 Cookie".into(),
            });
        }

        let uid = data
            .get("user_id_str")
            .and_then(json_uid_to_string)
            .or_else(|| data.get("user_id").and_then(json_uid_to_string))
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "豆包工作账号响应缺少 user_id，尚未完成登录".into(),
            })?;

        let display_name = data
            .get("screen_name")
            .or_else(|| data.get("name"))
            .or_else(|| data.get("user_name"))
            .and_then(non_empty_str);
        let display_email = data
            .get("email")
            .and_then(non_empty_str)
            .filter(|e| e.contains('@'));

        let mut extra = HashMap::new();
        if let Some(sec_uid) = data.get("sec_user_id").and_then(non_empty_str) {
            extra.insert("sec_user_id".to_string(), sec_uid);
        }

        Ok(CredentialEnvelope {
            tool_id: "doubaowork".to_string(),
            uid,
            display_name,
            display_email,
            payload: CredentialPayload::Token {
                access_token: normalized,
                refresh_token: None,
                expires_at: None,
                extra,
            },
        })
    }

    fn resolve_profile_root(host: &dyn PlatformHost) -> Result<PathBuf, AdapterError> {
        let dir = host
            .client_config_dir("doubaowork")
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: e.to_string(),
            })?;
        if dir.join("Default").join("Preferences").is_file() {
            return Ok(dir);
        }
        let win_nested = dir.join("User Data");
        if win_nested.join("Default").join("Preferences").is_file() {
            return Ok(win_nested);
        }
        Ok(dir)
    }

    /// 纯本地只读提取 `Default/Preferences` 中明文 `saman.last_user` 的 `(user_id, display_name)`。
    fn read_last_user(profile_root: &Path) -> Option<(String, Option<String>)> {
        let pref_path = profile_root.join("Default").join("Preferences");
        let bytes = std::fs::read(pref_path).ok()?;
        let val: Value = serde_json::from_slice(&bytes).ok()?;
        let last_user = val.pointer("/saman/last_user")?;
        let uid = last_user
            .get("user_id_str")
            .and_then(json_uid_to_string)
            .or_else(|| last_user.get("user_id").and_then(json_uid_to_string))
            .or_else(|| last_user.get("sec_user_id").and_then(json_uid_to_string))?;
        let name = last_user
            .get("name")
            .or_else(|| last_user.get("user_name"))
            .and_then(non_empty_str);
        Some((uid, name))
    }

    fn is_real_file(path: &Path) -> bool {
        std::fs::symlink_metadata(path)
            .map(|m| !m.file_type().is_symlink() && m.is_file())
            .unwrap_or(false)
    }

    fn is_real_dir(path: &Path) -> bool {
        std::fs::symlink_metadata(path)
            .map(|m| !m.file_type().is_symlink() && m.is_dir())
            .unwrap_or(false)
    }

    /// 枚举需要纳入切号快照的核心认证文件（排除 140MB 沙箱与 37MB 插件目录）。
    fn collect_session_relative_paths(profile_root: &Path) -> Vec<String> {
        let mut rels = Vec::new();
        let fixed = [
            "Default/Preferences",
            "Default/Cookies",
            "Default/Cookies-journal",
            "Default/Cookies-wal",
            "Default/Cookies-shm",
            "Default/Network/Cookies",
            "Default/Network/Cookies-journal",
            "Default/Network/Cookies-wal",
            "Default/Network/Cookies-shm",
            "sdk_storage/common.db",
            "sdk_storage/common.db-wal",
            "sdk_storage/common.db-shm",
        ];
        for rel in fixed {
            if Self::is_real_file(&profile_root.join(rel)) {
                rels.push(rel.to_string());
            }
        }

        // LevelDB 会话与持久化状态（跳过 LOCK 与符号链接）
        for dir_rel in [
            "Default/Local Storage/leveldb",
            "Default/saman_shell_persistent_storage_v1",
        ] {
            let dir_path = profile_root.join(dir_rel);
            if !Self::is_real_dir(&dir_path) {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(&dir_path) {
                for entry in entries.flatten() {
                    let file_name = entry.file_name();
                    let name = file_name.to_string_lossy();
                    if name == "LOCK" || !Self::is_real_file(&entry.path()) {
                        continue;
                    }
                    let rel = format!("{dir_rel}/{name}");
                    if is_allowed_snapshot_rel_path(&rel) {
                        rels.push(rel);
                    }
                }
            }
        }

        // aha/users/**（跳过 cache 与符号链接）
        let aha_users = profile_root.join("aha").join("users");
        if Self::is_real_dir(&aha_users) {
            collect_files_recursive(&aha_users, "aha/users", 0, &mut rels);
        }

        // sdk_storage/<user_hash>/cookie_store.db*
        let sdk_dir = profile_root.join("sdk_storage");
        if Self::is_real_dir(&sdk_dir) {
            if let Ok(entries) = std::fs::read_dir(&sdk_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !Self::is_real_dir(&path) {
                        continue;
                    }
                    let sub_name = entry.file_name().to_string_lossy().to_string();
                    if matches!(sub_name.as_str(), "log" | "settings") {
                        continue;
                    }
                    for db_name in [
                        "cookie_store.db",
                        "cookie_store.db-wal",
                        "cookie_store.db-shm",
                    ] {
                        if Self::is_real_file(&path.join(db_name)) {
                            let rel = format!("sdk_storage/{sub_name}/{db_name}");
                            if is_allowed_snapshot_rel_path(&rel) {
                                rels.push(rel);
                            }
                        }
                    }
                }
            }
        }

        rels.sort();
        rels.dedup();
        rels
    }

    fn build_profile_zip(profile_root: &Path, rels: &[String]) -> Result<Vec<u8>, AdapterError> {
        if rels.len() > MAX_SNAPSHOT_ENTRIES {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!(
                    "豆包工作会话快照文件数（{}）超出上限（{MAX_SNAPSHOT_ENTRIES}）",
                    rels.len()
                ),
            });
        }
        let mut cursor = Cursor::new(Vec::new());
        let mut total_uncompressed: u64 = 0;
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        {
            let mut writer = ZipWriter::new(&mut cursor);
            for rel in rels {
                if !is_allowed_snapshot_rel_path(rel) {
                    continue;
                }
                let src = profile_root.join(rel);
                if !Self::is_real_file(&src) {
                    continue;
                }
                let bytes = std::fs::read(&src).map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: format!("读取豆包工作会话文件失败 ({rel}): {e}"),
                })?;
                total_uncompressed = total_uncompressed.saturating_add(bytes.len() as u64);
                if total_uncompressed > MAX_SNAPSHOT_UNCOMPRESSED_BYTES {
                    return Err(AdapterError::Unified {
                        code: UnifiedErrorCode::InternalError,
                        message: "豆包工作会话快照未压缩体积超出 64 MiB 上限".into(),
                    });
                }
                writer
                    .start_file(rel, opts)
                    .map_err(|e| AdapterError::Unified {
                        code: UnifiedErrorCode::InternalError,
                        message: format!("写入会话快照条目失败 ({rel}): {e}"),
                    })?;
                writer
                    .write_all(&bytes)
                    .map_err(|e| AdapterError::Unified {
                        code: UnifiedErrorCode::InternalError,
                        message: format!("压缩会话快照失败 ({rel}): {e}"),
                    })?;
            }
            writer.finish().map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("封包豆包工作会话快照失败: {e}"),
            })?;
        }
        let zip_bytes = cursor.into_inner();
        if zip_bytes.len() > MAX_SNAPSHOT_ZIP_BYTES {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: "豆包工作会话快照压缩包超出 80 MiB 上限".into(),
            });
        }
        Ok(zip_bytes)
    }

    fn remove_file_if_present(path: &Path, context: &str) -> Result<(), AdapterError> {
        if let Err(e) = std::fs::remove_file(path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: format!("{context} ({}): {e}", path.display()),
                });
            }
        }
        Ok(())
    }

    fn clear_dir_files_except_lock(dir: &Path) -> Result<(), AdapterError> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name().to_string_lossy() == "LOCK" || !Self::is_real_file(&path) {
                continue;
            }
            Self::remove_file_if_present(&path, "清理旧会话缓存文件失败")?;
        }
        Ok(())
    }

    /// 清理所有瞬态 LevelDB 段与 SQLite WAL/SHM 附属日志，供切号与回滚共用。
    fn clean_session_ephemeral_files(profile_root: &Path) -> Result<(), AdapterError> {
        Self::clear_dir_files_except_lock(&profile_root.join("Default/Local Storage/leveldb"))?;
        Self::clear_dir_files_except_lock(
            &profile_root.join("Default/saman_shell_persistent_storage_v1"),
        )?;
        for sidecar in FIXED_SIDECARS {
            Self::remove_file_if_present(&profile_root.join(sidecar), "清理旧会话附属文件失败")?;
        }
        let sdk_dir = profile_root.join("sdk_storage");
        if Self::is_real_dir(&sdk_dir) {
            if let Ok(entries) = std::fs::read_dir(&sdk_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !Self::is_real_dir(&path) {
                        continue;
                    }
                    let sub_name = entry.file_name().to_string_lossy().to_string();
                    if matches!(sub_name.as_str(), "log" | "settings") {
                        continue;
                    }
                    for wal_name in ["cookie_store.db-wal", "cookie_store.db-shm"] {
                        Self::remove_file_if_present(
                            &path.join(wal_name),
                            "清理 sdk_storage WAL 失败",
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    fn extract_cookie(cred: &CredentialEnvelope) -> Result<String, AdapterError> {
        match &cred.payload {
            CredentialPayload::Token { access_token, .. } => normalize_cookie_header(access_token),
            CredentialPayload::LocalSnapshot { files } => {
                let raw = files
                    .get(SNAPSHOT_TOKEN_FILE)
                    .and_then(|b64| BASE64.decode(b64).ok())
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .ok_or_else(|| AdapterError::Unified {
                        code: UnifiedErrorCode::InvalidCredential,
                        message: "该豆包工作账号尚未完成在线授权（仅含本地切号快照），请点击「在线授权」补充 Cookie 后查询额度与签到".into(),
                    })?;
                normalize_cookie_header(&raw)
            }
            CredentialPayload::Cookies { cookies, .. } => {
                if cookies.is_empty() {
                    return Err(AdapterError::Unified {
                        code: UnifiedErrorCode::InvalidCredential,
                        message: "豆包工作 Cookie 集合为空".into(),
                    });
                }
                let joined = cookies
                    .iter()
                    .map(|(k, v)| format!("{}={}", k.trim(), v.trim()))
                    .collect::<Vec<_>>()
                    .join("; ");
                normalize_cookie_header(&joined)
            }
        }
    }

    pub(crate) fn extract_cookie_header(cred: &CredentialEnvelope) -> Result<String, String> {
        Self::extract_cookie(cred).map_err(|e| e.to_string())
    }

    async fn get_with_cookie(
        &self,
        base: &str,
        path_and_query: &str,
        cookie: &str,
        context: &str,
    ) -> Result<Value, AdapterError> {
        let (body, _) = self
            .get_with_cookie_and_set_cookies(base, path_and_query, cookie, context)
            .await?;
        Ok(body)
    }

    async fn get_with_cookie_and_set_cookies(
        &self,
        base: &str,
        path_and_query: &str,
        cookie: &str,
        context: &str,
    ) -> Result<(Value, Vec<String>), AdapterError> {
        let url = format!("{base}{path_and_query}");
        let resp = self
            .http
            .get(&url)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, DOUBAO_USER_AGENT)
            .header(REFERER, format!("{DEFAULT_WEB_BASE}/"))
            .header(COOKIE, cookie)
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("{context}失败: {}", e.without_url()),
            })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(map_http_status(status, context));
        }
        let set_cookies: Vec<String> = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_string))
            .collect();
        let body = resp
            .json::<Value>()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("解析{context}响应失败: {}", e.without_url()),
            })?;
        Ok((body, set_cookies))
    }

    async fn post_with_cookie(
        &self,
        path_and_query: &str,
        cookie: &str,
        body: &Value,
        context: &str,
    ) -> Result<Value, AdapterError> {
        let url = format!("{}{path_and_query}", self.web_base);
        let resp = self
            .http
            .post(&url)
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/json")
            .header(USER_AGENT, DOUBAO_USER_AGENT)
            .header("agw-js-conv", "str")
            .header("Origin", DEFAULT_WEB_BASE)
            .header(REFERER, format!("{DEFAULT_WEB_BASE}/chat/"))
            .header(COOKIE, cookie)
            .json(body)
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("{context}失败: {}", e.without_url()),
            })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(map_http_status(status, context));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("解析{context}响应失败: {}", e.without_url()),
            })
    }

    fn check_agw_code(body: &Value, context: &str, allow_200: bool) -> Result<(), AdapterError> {
        let code = body.get("code").and_then(int_field);
        let is_ok = match code {
            Some(0) => true,
            Some(200) if allow_200 => true,
            _ => false,
        };
        if is_ok {
            return Ok(());
        }
        if let Some(c) = code {
            if matches!(
                c,
                401 | 1009 | 7 | 8 | 401001 | 710012001 | 710012006 | 710012020
            ) {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::AuthExpired,
                    message: format!("{context}：登录态已失效（业务码 {c}）"),
                });
            }
            if c == 710022002 {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::RiskTriggered,
                    message: format!("{context}：触发安全风控（业务码 {c}）"),
                });
            }
            if c == 710012007 {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::RateLimited,
                    message: format!("{context}：请求受限，请稍后再试（业务码 {c}）"),
                });
            }
            // 710010702 / 710010202 / 710010101 是 AGW 网关侧的通用兜底码：
            // 上游业务 handler 未注册、参数契约不匹配或该入口对该端不开放时统一返回
            // 「系统错误」，并不代表登录态、风控或限流。此处必须降级为
            // `UnsupportedCapability`，否则会被当成 `NetworkError` 累计计数、
            // 进而把整个 doubaowork 工具误熔断，并让调度器对同一接口反复重试。
            if matches!(c, 710010702 | 710010202 | 710010101 | 625002002) {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::UnsupportedCapability,
                    message: format!("{context}：该入口当前不可用（业务码 {c}）"),
                });
            }
            if let Some(err) = map_business_code(c, context) {
                return Err(err);
            }
            let msg = body
                .get("msg")
                .or_else(|| body.get("message"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("未知业务错误");
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("{context}：业务码 {c}（{msg}）"),
            });
        }
        Err(AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("{context}：响应缺少业务状态码 code"),
        })
    }

    /// 读取本机豆包工作 `Local State` 中的桌面设备标识（`/aha/device/device_id`、`install_id`）。
    fn read_local_device_params(&self) -> DoubaoDeviceParams {
        let Some(host) = self.host.as_deref() else {
            return DoubaoDeviceParams::default();
        };
        let Ok(profile_root) = Self::resolve_profile_root(host) else {
            return DoubaoDeviceParams::default();
        };
        let state_path = profile_root.join("Local State");
        if !Self::is_real_file(&state_path) {
            return DoubaoDeviceParams::default();
        }
        let Ok(bytes) = std::fs::read(&state_path) else {
            return DoubaoDeviceParams::default();
        };
        let Ok(val) = serde_json::from_slice::<Value>(&bytes) else {
            return DoubaoDeviceParams::default();
        };
        DoubaoDeviceParams {
            device_id: val
                .pointer("/aha/device/device_id")
                .and_then(|v| v.as_str())
                .and_then(sanitize_device_param),
            install_id: val
                .pointer("/aha/device/install_id")
                .and_then(|v| v.as_str())
                .and_then(sanitize_device_param),
            web_id: None,
        }
    }

    /// 构造与官方 `DoubaoWork.app` 2.37.19 (`local_web`) 对齐的公共查询串，
    /// 优先从凭据 `extra` 提取设备标识，缺失时回退本机 `Local State`。
    fn build_doubao_query(&self, cred: Option<&CredentialEnvelope>) -> String {
        let extra = cred.and_then(|c| match &c.payload {
            CredentialPayload::Token { extra, .. } => Some(extra),
            _ => None,
        });
        let extra_param = |key: &str| {
            extra
                .and_then(|map| map.get(key))
                .and_then(|v| sanitize_device_param(v))
        };

        let local = self.read_local_device_params();
        let device_id = extra_param("device_id").or(local.device_id);
        let install_id = extra_param("iid").or(local.install_id);
        let web_id = extra_param("web_id").or(local.web_id);
        let web_tab_id = Uuid::new_v4().to_string();

        let mut params: Vec<(&str, &str)> = DOUBAO_BASE_QUERY_PARAMS.to_vec();
        params.push(("web_tab_id", &web_tab_id));
        if let Some(did) = device_id.as_deref() {
            params.push(("device_id", did));
            params.push(("web_id", web_id.as_deref().unwrap_or(did)));
            params.push(("tea_uuid", did));
        } else if let Some(wid) = web_id.as_deref() {
            params.push(("web_id", wid));
            params.push(("tea_uuid", wid));
        }
        if let Some(iid) = install_id.as_deref() {
            params.push(("iid", iid));
        }

        reqwest::Url::parse_with_params(DEFAULT_WEB_BASE, &params)
            .ok()
            .and_then(|u| u.query().map(str::to_string))
            .unwrap_or_default()
    }

    async fn fetch_quota_summary(
        &self,
        cookie: &str,
        cred: &CredentialEnvelope,
    ) -> Result<Value, AdapterError> {
        let query = self.build_doubao_query(Some(cred));
        let path = format!("/alice/commerce/sale/subscription/quota/summary/?{query}");
        let raw = self
            .post_with_cookie(
                &path,
                cookie,
                &serde_json::json!({ "product_line": "membership" }),
                "查询豆包工作额度",
            )
            .await?;
        Self::check_agw_code(&raw, "查询豆包工作额度", false)?;
        Ok(raw.get("data").cloned().unwrap_or(Value::Null))
    }

    async fn fetch_landing_page(
        &self,
        cookie: &str,
        cred: &CredentialEnvelope,
    ) -> Result<Value, AdapterError> {
        let query = self.build_doubao_query(Some(cred));
        let path = format!("/alice/ug/campaign/landing/get?{query}");
        let raw = self
            .post_with_cookie(
                &path,
                cookie,
                &serde_json::json!({}),
                "查询豆包工作签到状态",
            )
            .await?;
        Self::check_agw_code(&raw, "查询豆包工作签到状态", true)?;
        Ok(raw.get("data").cloned().unwrap_or(Value::Null))
    }

    /// 拉取 campaign v3 详情：这是签到状态的**权威源**。
    /// `landing/get` 只负责投放卡片（`chosen_type` / `landing_cards`），
    /// 真正记录「今天签没签」的是 v3 detail 里 `daily_checkin.checked_in_today`。
    async fn fetch_campaign_v3_detail(
        &self,
        cookie: &str,
        cred: &CredentialEnvelope,
    ) -> Result<Value, AdapterError> {
        let query = self.build_doubao_query(Some(cred));
        let path = format!("/alice/ug/campaign/v3/detail/?{query}");
        let raw = self
            .post_with_cookie(
                &path,
                cookie,
                &serde_json::json!({ "campaign_group_code": CAMPAIGN_GROUP_CODE }),
                "查询豆包工作签到状态",
            )
            .await?;
        Self::check_agw_code(&raw, "查询豆包工作签到状态", true)?;
        Ok(raw.get("data").cloned().unwrap_or(Value::Null))
    }

    /// 从 v3 detail 中读 `daily_signin.daily_checkin`。
    ///
    /// 返回 `(checked_in_today, continuous_days)`；结构缺失时返回 `None`
    /// （表示该账号没有投放签到任务，而不是「没签」）。
    fn daily_checkin_state(v3: &Value) -> Option<(bool, i64)> {
        let tasks = v3.get("tasks").and_then(|v| v.as_array())?;
        let task = tasks.iter().find(|t| {
            t.get("campaign_code")
                .and_then(non_empty_str)
                .is_some_and(|c| c == DAILY_SIGNIN_ITEM_CODE)
        })?;
        let checkin = task.get("daily_checkin")?;
        let checked = checkin
            .get("checked_in_today")
            .is_some_and(|v| v.as_bool() == Some(true) || int_field(v) == Some(1));
        let days = checkin
            .get("continuous_days")
            .and_then(int_field)
            .unwrap_or(0);
        Some((checked, days))
    }

    /// 每日签到的写接口：`POST /alice/ug/campaign/landing/checkin`。
    ///
    /// 官方客户端 2.37.19 的侧边栏签到按钮（`s2-ug-sidebar-popup-host` /
    /// `s2-ug-sidebar-landing-popup`）就是调它，`interactionType` 为 `CheckIn`。
    /// 早期误以为它已废弃（因为实测恒返回 AGW 兜底码 `710010702`），实际原因是
    /// **该账号的活动试用天数超过 20 天上限**，服务端发放校验抛异常；正常账号可用。
    async fn post_landing_checkin_claim(
        &self,
        cookie: &str,
        cred: &CredentialEnvelope,
        precheck: Option<&QuotaSnapshot>,
        current_days: i64,
    ) -> Result<ClaimResult, AdapterError> {
        let landing = self.fetch_landing_page(cookie, cred).await?;
        let candidates = match Self::extract_landing_scene(&landing) {
            Some(scene) => [serde_json::json!({ "scene": scene }), serde_json::json!({})],
            None => [
                serde_json::json!({}),
                serde_json::json!({ "scene": DEFAULT_CHECKIN_SCENE }),
            ],
        };
        let query = self.build_doubao_query(Some(cred));
        let checkin_path = format!("/alice/ug/campaign/landing/checkin?{query}");
        let mut last_err: Option<AdapterError> = None;

        for body in candidates {
            let raw = match self
                .post_with_cookie(&checkin_path, cookie, &body, "领取豆包工作每日签到额度")
                .await
            {
                Ok(v) => v,
                Err(err) => {
                    if is_terminal_claim_error(&err) {
                        return Err(err);
                    }
                    last_err = Some(err);
                    continue;
                }
            };

            let code = raw.get("code").and_then(int_field);
            let msg = raw
                .get("msg")
                .or_else(|| raw.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let data = raw.get("data").unwrap_or(&Value::Null);
            let checked_in = data
                .get("checked_in")
                .is_some_and(|v| v.as_bool() == Some(true) || int_field(v) == Some(1));

            if matches!(code, Some(0 | 200)) && checked_in {
                let days = data.get("continuous_days").and_then(int_field).unwrap_or(0);
                let reward_summary = if days > 0 {
                    format!("每日签到成功（已连续签到 {days} 天）")
                } else {
                    "每日签到已完成".to_string()
                };
                return Ok(ClaimResult {
                    claimed: true,
                    reward_summary,
                    updated_quota: finalize_claimed_quota(
                        self.inspect_quota(cred).await.ok(),
                        precheck,
                    ),
                });
            }

            let nested_msg = data
                .get("msg")
                .or_else(|| data.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if is_idempotent_claimed_phrase(msg) || is_idempotent_claimed_phrase(nested_msg) {
                return Ok(ClaimResult {
                    claimed: false,
                    reward_summary: "今日已签过".into(),
                    updated_quota: finalize_claimed_quota(
                        self.inspect_quota(cred).await.ok(),
                        precheck,
                    ),
                });
            }

            let attempt_err = match Self::check_agw_code(&raw, "领取豆包工作每日签到额度", true)
            {
                Ok(()) => AdapterError::Unified {
                    code: UnifiedErrorCode::NetworkError,
                    message: if msg.trim().is_empty() {
                        "豆包工作每日签到未完成".into()
                    } else {
                        format!("豆包工作每日签到未完成：{msg}")
                    },
                },
                // 710010702 是服务端发放校验抛异常后的 AGW 兜底码。已知最常见的
                // 触发原因是活动试用天数超上限，此处给出可执行的原因提示，
                // 而不是含糊的「系统错误」。
                Err(AdapterError::Unified {
                    code: UnifiedErrorCode::UnsupportedCapability,
                    message,
                }) => AdapterError::Unified {
                    code: UnifiedErrorCode::UnsupportedCapability,
                    message: format!(
                        "{message}；若持续失败，通常是活动试用天数已达上限或该账号未获活动资格"
                    ),
                },
                Err(err) => err,
            };
            if is_terminal_claim_error(&attempt_err) {
                return Err(attempt_err);
            }
            last_err = Some(attempt_err);
        }

        Err(last_err.unwrap_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("豆包工作每日签到未完成（已连续 {current_days} 天）"),
        }))
    }

    /// 查询订阅列表，提取「活动赠送的专业版试用套餐」天数。
    ///
    /// 活动规则第六条规定单用户通过活动可获得的试用总天数上限为 20 天；
    /// 账号持有超限的赠送套餐时，服务端发放校验会失败并返回 AGW 兜底码
    /// `710010702`（`landing/checkin` 与 `item/reward/claim` 都是如此）。
    /// 因此这里先读订阅数据，命中超限就直接判为不可领，不再打写接口。
    ///
    /// 返回 `None` 表示无法判定（接口异常或结构缺失），调用方按原逻辑继续。
    async fn fetch_campaign_trial_days(
        &self,
        cookie: &str,
        cred: &CredentialEnvelope,
    ) -> Option<i64> {
        let query = self.build_doubao_query(Some(cred));
        let path = format!("/alice/commerce/sale/subscription/list/?{query}");
        let raw = self
            .post_with_cookie(
                &path,
                cookie,
                &serde_json::json!({ "product_lines": ["membership"] }),
                "查询豆包工作订阅",
            )
            .await
            .ok()?;
        if raw.get("code").and_then(int_field) != Some(0) {
            return None;
        }
        let data = raw.get("data")?;
        Some(Self::max_campaign_trial_days(data))
    }

    /// 从订阅列表响应中取「活动赠送套餐」的最大天数。
    ///
    /// 只统计 `campaign_infos[].display_duration_days` 且
    /// `is_gift == true` 的生效中记录（`status == 3`）：
    /// 用户自购的正式会员不计入活动试用上限。
    fn max_campaign_trial_days(data: &Value) -> i64 {
        let mut max_days = 0i64;
        let Some(groups) = data.get("groups").and_then(|v| v.as_array()) else {
            return 0;
        };
        for group in groups {
            let Some(records) = group.get("records").and_then(|v| v.as_array()) else {
                continue;
            };
            for rec in records {
                if rec.get("is_gift").and_then(|v| v.as_bool()) != Some(true) {
                    continue;
                }
                // status 3 = 生效中；其它状态（已过期/未生效）不计入当前占用
                if rec.get("status").and_then(int_field) != Some(3) {
                    continue;
                }
                let Some(infos) = rec.get("campaign_infos").and_then(|v| v.as_array()) else {
                    continue;
                };
                for info in infos {
                    if let Some(days) = info.get("display_duration_days").and_then(int_field) {
                        max_days = max_days.max(days);
                    }
                }
            }
        }
        max_days
    }

    /// 是否已触达活动试用天数上限（超限账号无法领取，服务端会报 710010702）。
    fn campaign_trial_capped(trial_days: i64) -> bool {
        trial_days > CAMPAIGN_TRIAL_CAP_DAYS
    }

    fn parse_quota_summary(data: &Value) -> (String, Vec<QuotaBucket>) {
        let mut buckets = Vec::new();
        append_section_buckets(
            data.get("incentive_quota_section"),
            "incentive",
            &mut buckets,
        );
        append_section_buckets(data.get("quota_package_section"), "package", &mut buckets);

        let ent_len_before = buckets.len();
        append_section_buckets(
            data.get("enterprise_window_limit_section"),
            "window",
            &mut buckets,
        );
        let has_enterprise = buckets.len() > ent_len_before;

        let win_len_before = buckets.len();
        append_section_buckets(data.get("window_limit_section"), "window", &mut buckets);
        let has_personal_paid = buckets.len() > win_len_before;

        let member_type = data
            .pointer("/member_info/member_type")
            .and_then(int_field)
            .unwrap_or(0);
        let sub_status = data
            .pointer("/member_info/status")
            .and_then(int_field)
            .unwrap_or(0);

        let plan_name = if has_enterprise || matches!(member_type, 1 | 2) {
            "企业版"
        } else if has_personal_paid || sub_status == 3 {
            "专业版"
        } else {
            "免费版"
        };

        (plan_name.to_string(), buckets)
    }

    fn claim_status_from_landing(data: &Value) -> ClaimStatus {
        let chosen_type = data.get("chosen_type").and_then(int_field).unwrap_or(0);
        match chosen_type {
            // 2 = CompleteCheckinPage
            2 => ClaimStatus::Claimed,
            // 1 = MainPage：必须包含 button_type == 2 (CheckIn) 的按钮才认为今日可签
            1 => {
                let has_checkin_btn = data
                    .pointer("/main_page_info/buttons")
                    .and_then(|v| v.as_array())
                    .is_some_and(|btns| {
                        btns.iter()
                            .any(|b| b.get("button_type").and_then(int_field) == Some(2))
                    });
                if has_checkin_btn {
                    ClaimStatus::Available
                } else {
                    ClaimStatus::Unknown
                }
            }
            // 3 = Cards：卡片必须包含 interaction_type == 2 (CheckIn) 按钮才可签（避免已签/占位卡误判）
            3 => {
                let has_signin_card = data
                    .get("landing_cards")
                    .and_then(|v| v.as_array())
                    .is_some_and(|cards| {
                        cards.iter().any(|card| {
                            card.get("buttons")
                                .and_then(|v| v.as_array())
                                .is_some_and(|btns| {
                                    btns.iter().any(|b| {
                                        b.get("interaction_type").and_then(int_field) == Some(2)
                                    })
                                })
                        })
                    });
                if has_signin_card {
                    ClaimStatus::Available
                } else {
                    ClaimStatus::Unknown
                }
            }
            _ => ClaimStatus::Unknown,
        }
    }

    fn extract_landing_scene(data: &Value) -> Option<String> {
        data.get("campaign_group_code")
            .and_then(non_empty_str)
            .or_else(|| {
                data.pointer("/extra_params/campaign_group_code")
                    .and_then(non_empty_str)
            })
    }
}

impl Default for DoubaoWorkAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn collect_files_recursive(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_DIR_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), "cache" | "Cache" | "LOCK") {
            continue;
        }
        let path = entry.path();
        let rel = format!("{prefix}/{name}");
        if DoubaoWorkAdapter::is_real_dir(&path) {
            collect_files_recursive(&path, &rel, depth + 1, out);
        } else if DoubaoWorkAdapter::is_real_file(&path) && is_allowed_snapshot_rel_path(&rel) {
            out.push(rel);
        }
    }
}

/// 严格白名单校验快照内相对路径，彻底阻断路径穿越与工作区外写入。
pub(crate) fn is_allowed_snapshot_rel_path(rel: &str) -> bool {
    let norm = rel.replace('\\', "/");
    if norm.is_empty() || norm.starts_with('/') || norm.contains('\0') {
        return false;
    }
    let path = Path::new(&norm);
    for comp in path.components() {
        match comp {
            Component::Normal(seg) => {
                if seg.to_string_lossy().contains(':') {
                    return false;
                }
            }
            _ => return false,
        }
    }
    matches!(
        norm.as_str(),
        "Default/Preferences"
            | "Default/Cookies"
            | "Default/Cookies-journal"
            | "Default/Cookies-wal"
            | "Default/Cookies-shm"
            | "Default/Network/Cookies"
            | "Default/Network/Cookies-journal"
            | "Default/Network/Cookies-wal"
            | "Default/Network/Cookies-shm"
            | "sdk_storage/common.db"
            | "sdk_storage/common.db-wal"
            | "sdk_storage/common.db-shm"
    ) || (norm.starts_with("Default/Local Storage/leveldb/") && !norm.ends_with("/LOCK"))
        || (norm.starts_with("Default/saman_shell_persistent_storage_v1/")
            && !norm.ends_with("/LOCK"))
        || (norm.starts_with("aha/users/") && !norm.contains("/cache/"))
        || (norm.starts_with("sdk_storage/")
            && (norm.ends_with("/cookie_store.db")
                || norm.ends_with("/cookie_store.db-wal")
                || norm.ends_with("/cookie_store.db-shm")))
}

fn is_valid_cookie_kv(k: &str, v: &str) -> bool {
    !k.is_empty()
        && !v.is_empty()
        && k.bytes()
            .all(|b| b > 0x20 && b < 0x7f && !matches!(b, b';' | b'='))
        && v.bytes().all(|b| (0x20..=0x7e).contains(&b) && b != b';')
        && reqwest::header::HeaderValue::from_str(&format!("{k}={v}")).is_ok()
}

/// 标准化用户粘贴的 Cookie / JSON / cURL，确保无 CRLF 注入且至少包含有效豆包会话键。
pub(crate) fn normalize_cookie_header(raw: &str) -> Result<String, AdapterError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "豆包工作 Cookie 不能为空".into(),
        });
    }

    let extracted = if trimmed.starts_with('{') {
        let val: Value = serde_json::from_str(trimmed).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: format!("JSON 格式无效: {e}"),
        })?;
        if let Some(s) = val
            .get("cookie")
            .or_else(|| val.get("Cookie"))
            .and_then(|v| v.as_str())
        {
            s.to_string()
        } else if let Some(obj) = val
            .get("cookies")
            .and_then(|v| v.as_object())
            .or_else(|| val.as_object())
        {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|vs| format!("{}={}", k.trim(), vs.trim())))
                .collect::<Vec<_>>()
                .join("; ")
        } else {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "JSON 中未找到 cookie 字段或键值映射".into(),
            });
        }
    } else if trimmed.starts_with("curl ")
        || trimmed.contains("Cookie:")
        || trimmed.contains("cookie:")
        || trimmed.contains(" -b ")
    {
        extract_cookie_from_text(trimmed)
    } else {
        trimmed.to_string()
    };

    let clean = strip_cookie_prefix(&extracted).trim();
    if clean.chars().any(|c| matches!(c, '\r' | '\n' | '\0')) {
        return Err(AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "Cookie 包含非法换行或控制字符".into(),
        });
    }

    let mut pairs = Vec::new();
    let mut has_session_key = false;
    for part in clean.split(';') {
        let seg = part.trim();
        let Some((name, val)) = seg.split_once('=') else {
            continue;
        };
        let k = name.trim();
        let v = val.trim();
        if k.is_empty() || v.is_empty() {
            continue;
        }
        let is_session = matches!(k, "sessionid" | "sessionid_ss" | "sid_tt");
        if !is_valid_cookie_kv(k, v) {
            if is_session {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "豆包工作关键会话 Cookie 含非法或非 ASCII 字符".into(),
                });
            }
            // 跳过含中文/控制字节的辅助 Cookie，避免污染 HTTP 请求头
            continue;
        }
        if is_session {
            has_session_key = true;
        }
        pairs.push(format!("{k}={v}"));
    }

    if !has_session_key || pairs.is_empty() {
        return Err(AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "豆包工作 Cookie 缺少有效的 sessionid / sid_tt 会话字段".into(),
        });
    }

    let joined = pairs.join("; ");
    if reqwest::header::HeaderValue::from_str(&joined).is_err() {
        return Err(AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "豆包工作 Cookie 请求头格式非法".into(),
        });
    }
    Ok(joined)
}

fn strip_cookie_prefix(raw: &str) -> &str {
    let trimmed = raw.trim();
    if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("cookie:") {
        trimmed[7..].trim()
    } else {
        trimmed
    }
}

fn extract_cookie_from_text(raw: &str) -> String {
    // 提取 curl `-H 'Cookie: ...'` / `-H "cookie: ..."` / `-b '...'` 中的 Cookie 值
    for marker in [
        "-H 'Cookie:",
        "-H 'cookie:",
        "-H \"Cookie:",
        "-H \"cookie:",
        "--header 'Cookie:",
        "--header \"Cookie:",
    ] {
        if let Some(pos) = raw.find(marker) {
            let quote = if marker.contains('\'') { '\'' } else { '"' };
            let rest = &raw[pos + marker.len()..];
            if let Some(end) = rest.find(quote) {
                return rest[..end].trim().to_string();
            }
        }
    }
    for marker in ["-b '", "-b \"", "--cookie '", "--cookie \""] {
        if let Some(pos) = raw.find(marker) {
            let quote = marker.chars().last().unwrap_or('\'');
            let rest = &raw[pos + marker.len()..];
            if let Some(end) = rest.find(quote) {
                return rest[..end].trim().to_string();
            }
        }
    }
    for line in raw.lines() {
        let l = line.trim();
        if l.len() >= 7 && l[..7].eq_ignore_ascii_case("cookie:") {
            return l[7..].trim().to_string();
        }
    }
    raw.to_string()
}

fn append_section_buckets(section: Option<&Value>, kind_hint: &str, out: &mut Vec<QuotaBucket>) {
    let Some(groups) = section
        .and_then(|s| s.get("window_limit_groups"))
        .and_then(|v| v.as_array())
    else {
        return;
    };
    let section_exhausted = section
        .and_then(|s| s.get("usage_exhausted"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    for (g_idx, group) in groups.iter().enumerate() {
        let group_name = group.get("feature_group_name").and_then(non_empty_str);
        let Some(limits) = group.get("window_limits").and_then(|v| v.as_array()) else {
            continue;
        };
        for (l_idx, item) in limits.iter().enumerate() {
            let window_type = item.get("window_type").and_then(int_field).unwrap_or(0);
            let unlimited = item
                .pointer("/exemption/active")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let base_label = match (kind_hint, window_type) {
                ("incentive", _) => "活动奖励额度",
                ("package", _) => "额度包",
                (_, 1) => "当前时段额度",
                (_, 2) => "近 7 天额度",
                (_, 3) => "订阅总额度",
                (_, 4) => "每月额度",
                _ => "可用额度",
            };
            let label = match &group_name {
                Some(gn) if !gn.is_empty() && gn != base_label => format!("{base_label} · {gn}"),
                _ => base_label.to_string(),
            };

            let (used, total, remaining, unit) = match (
                opt_amount_field(item.get("used_amount")),
                opt_amount_field(item.get("total_amount")),
                opt_amount_field(item.get("used_percent")),
            ) {
                (Some(u), Some(t), _) if t > 0.0 => {
                    let rem = (t - u).max(0.0);
                    (u, t, rem, "额度".to_string())
                }
                (_, _, Some(pct)) => {
                    let clamped = if section_exhausted && !unlimited {
                        100.0
                    } else {
                        pct.clamp(0.0, 100.0)
                    };
                    (clamped, 100.0, (100.0 - clamped).max(0.0), "%".to_string())
                }
                _ => continue,
            };

            let raw_end = if unlimited {
                item.pointer("/exemption/end_time").and_then(int_field)
            } else {
                item.get("end_time").and_then(int_field)
            };
            let reset_at = raw_end.and_then(epoch_num_to_rfc3339);

            out.push(QuotaBucket {
                key: format!("{kind_hint}_{window_type}_{g_idx}_{l_idx}"),
                label,
                used,
                total,
                remaining,
                unit,
                reset_at,
            });
        }
    }
}

fn int_field(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

/// 将服务端响应中的 `Set-Cookie` 增量合并进现有 Cookie 串。
/// 仅当键值有实际变更或新增时返回 `Some(merged)`，无变化则返回 `None`。
fn merge_set_cookies(existing: &str, set_cookies: &[String]) -> Option<String> {
    if set_cookies.is_empty() {
        return None;
    }
    let mut pairs: Vec<(String, String)> = Vec::new();
    for part in existing.split(';') {
        if let Some((k, v)) = part.trim().split_once('=') {
            let key = k.trim();
            if !key.is_empty() {
                pairs.push((key.to_string(), v.trim().to_string()));
            }
        }
    }
    let before = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ");

    let mut changed = false;
    for raw_sc in set_cookies {
        let lower = raw_sc.to_ascii_lowercase();
        if lower.contains("max-age=0") {
            continue;
        }
        let Some(first_seg) = raw_sc.split(';').next() else {
            continue;
        };
        let Some((k, v)) = first_seg.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim();
        if key.is_empty()
            || val.is_empty()
            || key.chars().any(|c| matches!(c, '\r' | '\n' | '\0'))
            || val.chars().any(|c| matches!(c, '\r' | '\n' | '\0'))
        {
            continue;
        }
        if let Some(pos) = pairs.iter().position(|(ek, _)| ek == key) {
            if pairs[pos].1 != val {
                pairs[pos].1 = val.to_string();
                changed = true;
            }
        } else {
            pairs.push((key.to_string(), val.to_string()));
            changed = true;
        }
    }

    if !changed {
        return None;
    }
    let merged = pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    (merged != before).then_some(merged)
}

fn opt_amount_field(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    let val = v
        .as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))?;
    val.is_finite().then_some(val)
}

fn non_empty_str(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn epoch_num_to_rfc3339(n: i64) -> Option<String> {
    if n <= 0 {
        return None;
    }
    if n >= 1_000_000_000_000 {
        Utc.timestamp_millis_opt(n)
            .single()
            .map(|dt| dt.to_rfc3339())
    } else {
        Utc.timestamp_opt(n, 0).single().map(|dt| dt.to_rfc3339())
    }
}

fn is_terminal_claim_error(err: &AdapterError) -> bool {
    matches!(
        err,
        AdapterError::Unified {
            code: UnifiedErrorCode::AuthExpired
                | UnifiedErrorCode::RiskTriggered
                | UnifiedErrorCode::RateLimited,
            ..
        }
    )
}

fn finalize_claimed_quota(
    inspected: Option<QuotaSnapshot>,
    precheck: Option<&QuotaSnapshot>,
) -> Option<QuotaSnapshot> {
    let mut quota = merge_post_claim_quota(inspected, precheck, true);
    if let Some(ref mut q) = quota {
        // 豆包工作每日签到成功/已完成页即收敛为 Claimed
        q.claim_status = ClaimStatus::Claimed;
    }
    quota
}

#[async_trait]
impl ToolAdapter for DoubaoWorkAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    fn detect_local_active_uid(&self, host: &dyn PlatformHost) -> Option<String> {
        let profile_root = Self::resolve_profile_root(host).ok()?;
        let (uid, _) = Self::read_last_user(&profile_root)?;
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
        let profile_root = Self::resolve_profile_root(host)?;
        if !profile_root.join("Default").join("Preferences").is_file() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::ClientNotFound,
                message: "未检测到豆包工作桌面端配置目录，请先安装并登录一次客户端".into(),
            });
        }

        let Some((uid, display_name)) = Self::read_last_user(&profile_root) else {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "豆包工作桌面端尚未登录，请先在客户端登录后再导入".into(),
            });
        };

        let rels = Self::collect_session_relative_paths(&profile_root);
        let zip_bytes = Self::build_profile_zip(&profile_root, &rels)?;

        let mut files = HashMap::new();
        files.insert(SNAPSHOT_ZIP_KEY.to_string(), BASE64.encode(&zip_bytes));

        Ok(CredentialEnvelope {
            tool_id: "doubaowork".to_string(),
            uid,
            display_name,
            display_email: None,
            payload: CredentialPayload::LocalSnapshot { files },
        })
    }

    async fn parse_manual_token(
        &self,
        raw_input: &str,
    ) -> Result<CredentialEnvelope, AdapterError> {
        self.verify_cookie_with_base(raw_input).await
    }

    async fn inspect_quota(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<QuotaSnapshot, AdapterError> {
        let cookie = Self::extract_cookie(cred)?;
        let summary_data = self.fetch_quota_summary(&cookie, cred).await?;
        let (plan_name, buckets) = Self::parse_quota_summary(&summary_data);

        // 每日签到状态探测：以 campaign v3 detail 的 `daily_checkin` 为权威源，
        // `landing/get` 只作为补充（它只反映卡片投放，不反映已签事实）。
        // 认证/风控/限流错误向上熔断，其余异常降级为 Unknown。
        let mut claim_status = match self.fetch_campaign_v3_detail(&cookie, cred).await {
            Ok(v3) => match Self::daily_checkin_state(&v3) {
                Some((true, _)) => ClaimStatus::Claimed,
                Some((false, _)) => ClaimStatus::Available,
                None => ClaimStatus::Unsupported,
            },
            Err(
                e @ AdapterError::Unified {
                    code:
                        UnifiedErrorCode::AuthExpired
                        | UnifiedErrorCode::RiskTriggered
                        | UnifiedErrorCode::RateLimited,
                    ..
                },
            ) => return Err(e),
            Err(_) => ClaimStatus::Unknown,
        };

        if claim_status == ClaimStatus::Unknown {
            claim_status = match self.fetch_landing_page(&cookie, cred).await {
                Ok(landing) => Self::claim_status_from_landing(&landing),
                Err(
                    e @ AdapterError::Unified {
                        code:
                            UnifiedErrorCode::AuthExpired
                            | UnifiedErrorCode::RiskTriggered
                            | UnifiedErrorCode::RateLimited,
                        ..
                    },
                ) => return Err(e),
                Err(_) => ClaimStatus::Unknown,
            };
        }

        // 活动试用天数上限：超限账号的发放校验必然失败（服务端报 710010702），
        // 直接判为不可领，避免 UI 一直显示可领按钮、调度器反复打写接口。
        if claim_status == ClaimStatus::Available {
            if let Some(days) = self.fetch_campaign_trial_days(&cookie, cred).await {
                if Self::campaign_trial_capped(days) {
                    claim_status = ClaimStatus::Unsupported;
                }
            }
        }

        Ok(QuotaSnapshot {
            plan_name: Some(plan_name),
            claim_status,
            buckets,
            fetched_at: Utc::now().to_rfc3339(),
        })
    }

    async fn claim_daily(
        &self,
        cred: &CredentialEnvelope,
        precheck: Option<&QuotaSnapshot>,
    ) -> Result<ClaimResult, AdapterError> {
        let cookie = Self::extract_cookie(cred)?;

        if let Some(q) = precheck {
            match q.claim_status {
                ClaimStatus::Claimed => {
                    return Ok(ClaimResult {
                        claimed: false,
                        reward_summary: "今日已签过".into(),
                        updated_quota: Some(q.clone()),
                    });
                }
                ClaimStatus::Unsupported => {
                    // 先判断是不是「活动试用天数超限」这种可解释的原因，
                    // 避免只回一句含糊的「暂不支持」。
                    let summary = match self.fetch_campaign_trial_days(&cookie, cred).await {
                        Some(days) if Self::campaign_trial_capped(days) => format!(
                            "活动试用天数已达上限（当前 {days} 天 / 上限 {CAMPAIGN_TRIAL_CAP_DAYS} 天），套餐到期后可再签"
                        ),
                        _ => "当前账号暂不支持每日签到".to_string(),
                    };
                    return Ok(ClaimResult {
                        claimed: false,
                        reward_summary: summary,
                        updated_quota: None,
                    });
                }
                ClaimStatus::Available | ClaimStatus::Unknown => {}
            }
        }

        // 先看权威源：v3 detail 里已签就直接幂等返回，避免多打一次写接口。
        let mut current_days: i64 = 0;
        match self.fetch_campaign_v3_detail(&cookie, cred).await {
            Ok(v3) => match Self::daily_checkin_state(&v3) {
                Some((true, _days)) => {
                    return Ok(ClaimResult {
                        claimed: false,
                        reward_summary: "今日已签过".into(),
                        updated_quota: finalize_claimed_quota(
                            self.inspect_quota(cred).await.ok(),
                            precheck,
                        ),
                    });
                }
                Some((false, days)) => current_days = days,
                None => {
                    // 该账号没有投放签到任务：不再尝试写接口，直接判定不支持。
                    return Ok(ClaimResult {
                        claimed: false,
                        reward_summary: "当前账号暂不支持每日签到".into(),
                        updated_quota: None,
                    });
                }
            },
            Err(
                e @ AdapterError::Unified {
                    code:
                        UnifiedErrorCode::AuthExpired
                        | UnifiedErrorCode::RiskTriggered
                        | UnifiedErrorCode::RateLimited,
                    ..
                },
            ) => return Err(e),
            Err(_) => {
                // v3 detail 不可用时退回 landing 投放判断，避免误判为「可领」
                let landing = self.fetch_landing_page(&cookie, cred).await?;
                if Self::claim_status_from_landing(&landing) == ClaimStatus::Claimed {
                    return Ok(ClaimResult {
                        claimed: false,
                        reward_summary: "今日已签过".into(),
                        updated_quota: finalize_claimed_quota(
                            self.inspect_quota(cred).await.ok(),
                            precheck,
                        ),
                    });
                }
            }
        }

        // 活动试用天数上限：超限账号的发放校验必然失败（服务端报 710010702），
        // 这里直接返回「不可领」并说明原因，不再无意义地打写接口。
        if let Some(days) = self.fetch_campaign_trial_days(&cookie, cred).await {
            if Self::campaign_trial_capped(days) {
                return Ok(ClaimResult {
                    claimed: false,
                    reward_summary: format!(
                        "活动试用天数已达上限（当前 {days} 天 / 上限 {CAMPAIGN_TRIAL_CAP_DAYS} 天），套餐到期后可再签"
                    ),
                    updated_quota: None,
                });
            }
        }

        // 写接口：`landing/checkin`。
        // 官方客户端 2.37.19 的侧边栏签到按钮（`s2-ug-sidebar-popup-host`）就是调它；
        // 之前以为它已废弃是误判——它在超限账号上恒报 710010702，在正常账号上可用。
        self.post_landing_checkin_claim(&cookie, cred, precheck, current_days)
            .await
    }

    /// 豆包工作 Cookie 轻量保活与 `Set-Cookie` 续期回写：
    /// 定期触达 `/passport/account/info/v2/` 维持服务端滑动会话，并把响应 `Set-Cookie`
    /// 中续期/轮换的 Cookie 合并回本地凭据；缺少在线 Cookie（纯本地切号快照）时静默跳过。
    async fn refresh_credential(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<Option<CredentialEnvelope>, AdapterError> {
        let cookie = match Self::extract_cookie(cred) {
            Ok(c) => c,
            Err(_) => return Ok(None),
        };
        let query_path = format!("/passport/account/info/v2/?aid={AID}&account_sdk_source=web");
        let (body, set_cookies) = match self
            .get_with_cookie_and_set_cookies(
                &self.web_base,
                &query_path,
                &cookie,
                "保活豆包工作登录态",
            )
            .await
        {
            Ok(pair) => pair,
            Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                ..
            }) => {
                self.get_with_cookie_and_set_cookies(
                    &self.accounts_base,
                    &query_path,
                    &cookie,
                    "保活豆包工作登录态",
                )
                .await?
            }
            Err(err) => return Err(err),
        };

        let message = body
            .get("message")
            .and_then(|v| v.as_str())
            .map(str::to_ascii_lowercase);
        let data = body.get("data").unwrap_or(&Value::Null);
        let passport_err = data.get("error_code").and_then(int_field).unwrap_or(0);
        if message.as_deref() == Some("error") || passport_err != 0 {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                message: "豆包工作 Cookie 已失效，请重新登录或粘贴最新 Cookie".into(),
            });
        }

        let screen_name = data
            .get("screen_name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);
        let merged_cookie = merge_set_cookies(&cookie, &set_cookies);
        let name_changed = screen_name
            .as_ref()
            .is_some_and(|n| cred.display_name.as_deref() != Some(n.as_str()));
        if merged_cookie.is_none() && !name_changed {
            return Ok(None);
        }

        let mut updated = cred.clone();
        if let Some(n) = screen_name {
            updated.display_name = Some(n);
        }
        if let Some(merged_cookie) = merged_cookie {
            match &mut updated.payload {
                CredentialPayload::Token { access_token, .. } => {
                    *access_token = merged_cookie;
                }
                CredentialPayload::LocalSnapshot { files } => {
                    files.insert(
                        SNAPSHOT_TOKEN_FILE.to_string(),
                        BASE64.encode(merged_cookie.as_bytes()),
                    );
                }
                CredentialPayload::Cookies { cookies, .. } => {
                    cookies.clear();
                    for pair in merged_cookie.split(';') {
                        if let Some((k, v)) = pair.split_once('=') {
                            let key = k.trim();
                            if !key.is_empty() {
                                cookies.insert(key.to_string(), v.trim().to_string());
                            }
                        }
                    }
                }
            }
        }
        Ok(Some(updated))
    }

    async fn fetch_account_profile(
        &self,
        cred: &CredentialEnvelope,
    ) -> Option<(Option<String>, Option<String>)> {
        let cookie = Self::extract_cookie(cred).ok()?;
        let query_path = format!("/passport/account/info/v2/?aid={AID}&account_sdk_source=web");
        let (body, _) = match self
            .get_with_cookie_and_set_cookies(
                &self.web_base,
                &query_path,
                &cookie,
                "查询豆包工作账号资料",
            )
            .await
        {
            Ok(pair) => pair,
            Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                ..
            }) => self
                .get_with_cookie_and_set_cookies(
                    &self.accounts_base,
                    &query_path,
                    &cookie,
                    "查询豆包工作账号资料",
                )
                .await
                .ok()?,
            Err(_) => return None,
        };
        let message = body
            .get("message")
            .and_then(|v| v.as_str())
            .map(str::to_ascii_lowercase);
        let data = body.get("data").unwrap_or(&Value::Null);
        let passport_err = data.get("error_code").and_then(int_field).unwrap_or(0);
        if message.as_deref() == Some("error") || passport_err != 0 {
            return None;
        }
        let screen_name = data
            .get("screen_name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)?;
        Some((Some(screen_name), None))
    }

    async fn apply_session(
        &self,
        host: &dyn PlatformHost,
        cred: &CredentialEnvelope,
    ) -> Result<(), AdapterError> {
        let CredentialPayload::LocalSnapshot { files } = &cred.payload else {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::UnsupportedCapability,
                message: "该账号仅包含在线 Cookie 凭据，缺少桌面端切换快照；请先在豆包工作桌面端登录该账号并点「本机导入」补全快照".into(),
            });
        };

        let zip_b64 = files.get(SNAPSHOT_ZIP_KEY).ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::UnsupportedCapability,
            message: "该账号仅包含在线 Cookie 凭据，缺少桌面端切换快照；请先在豆包工作桌面端登录该账号并点「本机导入」补全快照".into(),
        })?;
        let zip_bytes = BASE64.decode(zip_b64).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: format!("豆包工作会话快照解码失败: {e}"),
        })?;
        if zip_bytes.len() > MAX_SNAPSHOT_ZIP_BYTES {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "豆包工作会话快照体积超出限制，已拒绝写入".into(),
            });
        }

        let mut archive =
            ZipArchive::new(Cursor::new(zip_bytes)).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("豆包工作会话快照格式无效: {e}"),
            })?;
        if archive.len() > MAX_SNAPSHOT_ENTRIES {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "豆包工作会话快照条目数量过多，已拒绝写入".into(),
            });
        }

        let mut extracted_files: Vec<(String, Vec<u8>)> = Vec::with_capacity(archive.len());
        let mut total_bytes: u64 = 0;
        for idx in 0..archive.len() {
            let mut entry = archive.by_index(idx).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("读取会话快照条目失败: {e}"),
            })?;
            if entry.is_dir() || entry.is_symlink() {
                continue;
            }
            let Some(enclosed) = entry.enclosed_name() else {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "豆包工作会话快照包含非法路径，已拒绝写入".into(),
                });
            };
            let rel_str = enclosed.to_string_lossy().replace('\\', "/");
            if !is_allowed_snapshot_rel_path(&rel_str) {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: format!("豆包工作会话快照包含受限外部路径: {rel_str}"),
                });
            }

            let mut buf = Vec::new();
            let remaining_cap = MAX_SNAPSHOT_UNCOMPRESSED_BYTES.saturating_sub(total_bytes);
            entry
                .by_ref()
                .take(remaining_cap.saturating_add(1))
                .read_to_end(&mut buf)
                .map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: format!("解压会话快照条目失败 ({rel_str}): {e}"),
                })?;
            total_bytes = total_bytes.saturating_add(buf.len() as u64);
            if total_bytes > MAX_SNAPSHOT_UNCOMPRESSED_BYTES {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "豆包工作会话快照解压总大小超出限制，已拒绝写入".into(),
                });
            }
            extracted_files.push((rel_str, buf));
        }

        if !extracted_files
            .iter()
            .any(|(rel, _)| rel == "Default/Preferences")
        {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "豆包工作会话快照缺少核心 Preferences 文件，已取消写入".into(),
            });
        }

        let profile_root = Self::resolve_profile_root(host)?;
        // 清理旧账号遗留 LevelDB 段与 SQLite WAL 附属日志，避免跨号混用崩溃
        Self::clean_session_ephemeral_files(&profile_root)?;

        for (rel, bytes) in extracted_files {
            write_private_file_atomic(&profile_root.join(&rel), &bytes)?;
        }
        Ok(())
    }

    async fn backup_session(&self, host: &dyn PlatformHost) -> Result<SessionBackup, AdapterError> {
        let profile_root = Self::resolve_profile_root(host)?;
        let mut rels = Self::collect_session_relative_paths(&profile_root);
        for base in [
            "Default/Preferences",
            "Default/Cookies",
            "Default/Network/Cookies",
            "sdk_storage/common.db",
        ]
        .into_iter()
        .chain(FIXED_SIDECARS.iter().copied())
        {
            if !rels.iter().any(|r| r == base) {
                rels.push(base.to_string());
            }
        }
        let paths: Vec<PathBuf> = rels.into_iter().map(|r| profile_root.join(r)).collect();
        crate::adapter::backup_files(&paths)
    }

    async fn restore_session(
        &self,
        host: &dyn PlatformHost,
        backup: &SessionBackup,
    ) -> Result<(), AdapterError> {
        let profile_root = Self::resolve_profile_root(host)?;
        Self::clean_session_ephemeral_files(&profile_root)?;
        crate::adapter::restore_files(backup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{KeyExpectation, ScopedPlatformHost};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// 有状态的 `v3/detail` 响应器：前 `pending_before` 次返回「未签」，
    /// 之后返回「已签」。用于真实模拟平台异步落账，避免多个同路径 mock 匹配顺序不确定。
    struct CheckinStateResponder {
        calls: std::sync::atomic::AtomicUsize,
        pending_before: usize,
        days: i64,
    }

    impl CheckinStateResponder {
        fn new(pending_before: usize, days: i64) -> Self {
            Self {
                calls: std::sync::atomic::AtomicUsize::new(0),
                pending_before,
                days,
            }
        }
    }

    impl wiremock::Respond for CheckinStateResponder {
        fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let checked = n >= self.pending_before;
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "campaign_code": "pc_incentive_task_mvp_v3",
                    "tasks": [{
                        "campaign_code": "daily_signin",
                        "campaign_group_code": "pc_incentive_task_mvp_v3",
                        "task_status": if checked { 3 } else { 1 },
                        "reward_mode": 2,
                        "daily_checkin": {
                            "checked_in_today": checked,
                            "continuous_days": if checked { self.days } else { 0 },
                            "total_days": if checked { self.days } else { 0 },
                            "reward": { "reward_amount": 400, "reward_type": "quota_grant" }
                        }
                    }]
                }
            }))
        }
    }

    struct DummyHost {
        root: PathBuf,
    }

    #[async_trait]
    impl PlatformHost for DummyHost {
        fn qwitch_data_dir(&self) -> Result<PathBuf, crate::platform::HostError> {
            Ok(self.root.join("qwitch"))
        }

        fn client_config_dir(&self, tool_id: &str) -> Result<PathBuf, crate::platform::HostError> {
            Ok(self.root.join(tool_id))
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
            _expectation: &KeyExpectation,
        ) -> Result<[u8; 32], crate::platform::HostError> {
            Ok([0u8; 32])
        }

        fn set_launch_at_login(&self, _enabled: bool) -> Result<(), String> {
            Ok(())
        }
    }

    fn seed_doubao_profile(dir: &Path, uid: &str, name: &str, ldb_file: &str, cookie_bytes: &[u8]) {
        std::fs::create_dir_all(dir.join("Default/Local Storage/leveldb")).unwrap();
        std::fs::create_dir_all(dir.join("Default/saman_shell_persistent_storage_v1")).unwrap();
        std::fs::create_dir_all(dir.join("aha/users/u_hash")).unwrap();
        std::fs::create_dir_all(dir.join("sdk_storage/u_hash")).unwrap();
        // Should be ignored (140MB sandbox / cache)
        std::fs::create_dir_all(dir.join("Default/sandbox_envs_dir")).unwrap();
        std::fs::write(dir.join("Default/sandbox_envs_dir/big.bin"), b"ignored").unwrap();

        let prefs = serde_json::json!({
            "saman": {
                "last_user": {
                    "user_id": uid,
                    "sec_user_id": format!("sec_{uid}"),
                    "name": name
                }
            }
        });
        std::fs::write(dir.join("Default/Preferences"), prefs.to_string()).unwrap();
        std::fs::write(dir.join("Default/Cookies"), cookie_bytes).unwrap();
        std::fs::write(
            dir.join("Default/Local Storage/leveldb").join(ldb_file),
            b"ldb-data",
        )
        .unwrap();
        std::fs::write(
            dir.join("Default/saman_shell_persistent_storage_v1/000003.log"),
            b"saman-ldb",
        )
        .unwrap();
        std::fs::write(dir.join("aha/users/u_hash/Preferences"), b"{}").unwrap();
        std::fs::write(dir.join("sdk_storage/common.db"), b"sqlite-common").unwrap();
        std::fs::write(
            dir.join("sdk_storage/u_hash/cookie_store.db"),
            b"sqlite-cookie",
        )
        .unwrap();
        std::fs::write(
            dir.join("sdk_storage/u_hash/cookie_store.db-wal"),
            b"sqlite-wal",
        )
        .unwrap();
    }

    #[test]
    fn cookie_normalization_handles_raw_json_curl_and_blocks_injection() {
        assert_eq!(
            normalize_cookie_header("Cookie: sessionid=abc123; odin_tt=xyz; foo=bar").unwrap(),
            "sessionid=abc123; odin_tt=xyz; foo=bar"
        );

        let from_json =
            normalize_cookie_header(r#"{"sessionid_ss":"ss-val","passport_csrf_token":"tok"}"#)
                .unwrap();
        assert!(from_json.contains("sessionid_ss=ss-val"));

        let from_curl = normalize_cookie_header(
            "curl 'https://www.doubao.com/' -H 'Accept: application/json' -H 'Cookie: sid_tt=t123; foo=1'",
        )
        .unwrap();
        assert_eq!(from_curl, "sid_tt=t123; foo=1");

        // 辅助 Cookie 含非 ASCII 字节时自动剔除，关键会话键含非 ASCII 时直接拒绝
        let skipped_non_ascii =
            normalize_cookie_header("sessionid=abc123; cn_tip=中文提示; odin_tt=ok").unwrap();
        assert_eq!(skipped_non_ascii, "sessionid=abc123; odin_tt=ok");
        assert!(normalize_cookie_header("sessionid=中文会话").is_err());

        assert!(normalize_cookie_header("foo=bar; baz=1").is_err());
        assert!(normalize_cookie_header("sessionid=abc\r\nEvil: 1").is_err());
    }

    #[test]
    fn read_last_user_prefers_user_id_str_and_zero_total_falls_back_to_percent() {
        let root = std::env::temp_dir().join(format!("qwitch_doubao_uidstr_{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("Default")).unwrap();
        std::fs::write(
            root.join("Default/Preferences"),
            serde_json::json!({
                "saman": {
                    "last_user": {
                        "user_id_str": "9007199254740993123",
                        "user_id": 9007199254740992000u64,
                        "sec_user_id": "sec_fallback",
                        "name": "测试用户"
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        let (uid, name) = DoubaoWorkAdapter::read_last_user(&root).unwrap();
        assert_eq!(uid, "9007199254740993123");
        assert_eq!(name.as_deref(), Some("测试用户"));
        let _ = std::fs::remove_dir_all(&root);

        let mut buckets = Vec::new();
        let section = serde_json::json!({
            "usage_exhausted": false,
            "window_limit_groups": [{
                "feature_group_name": "兜底组",
                "window_limits": [{
                    "window_type": 2,
                    "used_amount": 0,
                    "total_amount": 0,
                    "used_percent": 35
                }]
            }]
        });
        append_section_buckets(Some(&section), "enterprise", &mut buckets);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].unit, "%");
        assert_eq!(buckets[0].remaining, 65.0);
    }

    #[tokio::test]
    async fn scoped_zip_snapshot_detect_import_apply_and_restore_roundtrip() {
        let root = std::env::temp_dir().join(format!("qwitch_doubao_zip_{}", Uuid::new_v4()));
        let profile_dir = root.join("doubaowork");
        seed_doubao_profile(
            &profile_dir,
            "111222",
            "豆包一号",
            "000005.ldb",
            b"cookies-a",
        );

        // Prepare second account snapshot in another temp profile (with Account B distinct leveldb + cookie_store wal)
        let profile_b = root.join("profile_b");
        seed_doubao_profile(&profile_b, "333444", "豆包二号", "000009.ldb", b"cookies-b");
        std::fs::create_dir_all(profile_b.join("sdk_storage/u_b_only")).unwrap();
        std::fs::write(
            profile_b.join("sdk_storage/u_b_only/cookie_store.db-wal"),
            b"wal-b",
        )
        .unwrap();

        let host: Arc<dyn PlatformHost> = Arc::new(DummyHost { root: root.clone() });
        let host_b = ScopedPlatformHost::new(Arc::clone(&host), "doubaowork", profile_b.clone());

        let adapter = DoubaoWorkAdapter::new();
        assert_eq!(
            adapter.detect_local_active_uid(host.as_ref()).as_deref(),
            Some("111222")
        );

        let env_a = adapter
            .import_from_local(host.as_ref())
            .await
            .expect("import a");
        assert_eq!(env_a.capability_flags(), (true, false));

        let env_b = adapter.import_from_local(&host_b).await.expect("import b");
        assert_eq!(env_b.uid, "333444");

        let backup = adapter
            .backup_session(host.as_ref())
            .await
            .expect("backup a");
        adapter
            .apply_session(host.as_ref(), &env_b)
            .await
            .expect("apply b");

        assert_eq!(
            adapter.detect_local_active_uid(host.as_ref()).as_deref(),
            Some("333444")
        );
        assert_eq!(
            std::fs::read(profile_dir.join("Default/Cookies")).unwrap(),
            b"cookies-b"
        );
        assert!(
            !profile_dir
                .join("Default/Local Storage/leveldb/000005.ldb")
                .exists(),
            "旧账号 LevelDB 段应在写入前清理"
        );
        assert!(profile_dir
            .join("Default/Local Storage/leveldb/000009.ldb")
            .is_file());
        // sandbox_envs_dir must stay untouched & excluded from snapshot
        assert!(profile_dir
            .join("Default/sandbox_envs_dir/big.bin")
            .is_file());

        // 恢复原会话 A
        adapter
            .restore_session(host.as_ref(), &backup)
            .await
            .expect("restore a");
        assert_eq!(
            adapter.detect_local_active_uid(host.as_ref()).as_deref(),
            Some("111222")
        );
        assert_eq!(
            std::fs::read(profile_dir.join("Default/Cookies")).unwrap(),
            b"cookies-a"
        );
        assert!(
            !profile_dir
                .join("Default/Local Storage/leveldb/000009.ldb")
                .exists(),
            "回滚时必须清除 B 账号留下的 LevelDB 段"
        );
        assert!(
            !profile_dir
                .join("sdk_storage/u_b_only/cookie_store.db-wal")
                .exists(),
            "回滚时必须清除 B 账号留下的 WAL 附属文件"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn apply_session_rejects_zip_slip_path_traversal() {
        let root = std::env::temp_dir().join(format!("qwitch_doubao_evilzip_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let host = DummyHost { root: root.clone() };
        let mut cur = Cursor::new(Vec::new());
        {
            let mut w = ZipWriter::new(&mut cur);
            w.start_file("../escape.txt", SimpleFileOptions::default())
                .unwrap();
            w.write_all(b"evil").unwrap();
            w.finish().unwrap();
        }
        let mut files = HashMap::new();
        files.insert(
            SNAPSHOT_ZIP_KEY.to_string(),
            BASE64.encode(cur.into_inner()),
        );
        let env = CredentialEnvelope {
            tool_id: "doubaowork".to_string(),
            uid: "999".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::LocalSnapshot { files },
        };
        let err = DoubaoWorkAdapter::new()
            .apply_session(&host, &env)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                ..
            }
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn verify_cookie_inspect_quota_and_claim_daily_integration() {
        let server = MockServer::start().await;
        let adapter = DoubaoWorkAdapter::with_base_urls(server.uri(), server.uri());

        Mock::given(method("GET"))
            .and(path("/passport/account/info/v2/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": "success",
                "data": {
                    "user_id_str": "778899",
                    "screen_name": "豆包打工人",
                    "email": "worker@example.com"
                }
            })))
            .mount(&server)
            .await;

        let cred = adapter
            .verify_cookie_with_base("sessionid=real-session; odin_tt=abc")
            .await
            .expect("verify cookie");
        assert_eq!(cred.tool_id, "doubaowork");
        assert_eq!(cred.uid, "778899");
        assert_eq!(cred.display_name.as_deref(), Some("豆包打工人"));

        Mock::given(method("POST"))
            .and(path("/alice/commerce/sale/subscription/quota/summary/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "member_info": { "status": 3, "member_type": 0 },
                    "incentive_quota_section": {
                        "window_limit_groups": [{
                            "feature_group_name": "日常签到激励",
                            "window_limits": [{
                                "window_type": 3,
                                "used_amount": "120",
                                "total_amount": "1000",
                                "used_percent": 12
                            }]
                        }]
                    },
                    "window_limit_section": {
                        "window_limit_groups": [{
                            "feature_group_name": "豆包工作会员",
                            "window_limits": [{
                                "window_type": 1,
                                "used_percent": 25,
                                "end_time": 1795400000
                            }]
                        }]
                    }
                }
            })))
            .mount(&server)
            .await;

        // v3 detail 是签到状态的权威源。前 2 次（inspect_quota + claim 前置复核）未签，
        // 领取后回查（第 3 次起）已签，由有状态 responder 模拟平台异步落账。
        Mock::given(method("POST"))
            .and(path("/alice/ug/campaign/v3/detail/"))
            .respond_with(CheckinStateResponder::new(2, 5))
            .mount(&server)
            .await;

        let quota = adapter.inspect_quota(&cred).await.expect("inspect quota");
        assert_eq!(quota.plan_name.as_deref(), Some("专业版"));
        assert_eq!(quota.claim_status, ClaimStatus::Available);
        assert_eq!(quota.buckets.len(), 2);
        assert_eq!(quota.buckets[0].remaining, 880.0);
        assert_eq!(quota.buckets[1].unit, "%");

        // 未超限：无活动赠送套餐，走真实写接口 landing/checkin
        Mock::given(method("POST"))
            .and(path("/alice/commerce/sale/subscription/list/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": { "groups": [] }
            })))
            .mount(&server)
            .await;

        // 写接口内部会先读 landing 拿 scene
        Mock::given(method("POST"))
            .and(path("/alice/ug/campaign/landing/get"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "chosen_type": 3,
                    "landing_cards": [{
                        "card_code": "daily_signin",
                        "buttons": [{ "interaction_type": 2, "button_id": "checkin" }]
                    }]
                }
            })))
            .mount(&server)
            .await;

        // 官方客户端签到按钮走的写接口
        Mock::given(method("POST"))
            .and(path("/alice/ug/campaign/landing/checkin"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": { "checked_in": true, "continuous_days": 5 }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        let claimed = adapter
            .claim_daily(&cred, Some(&quota))
            .await
            .expect("claim daily");
        assert!(claimed.claimed);
        assert!(claimed.reward_summary.contains("连续签到 5 天"));
        assert_eq!(
            claimed.updated_quota.as_ref().map(|q| &q.claim_status),
            Some(&ClaimStatus::Claimed)
        );

        // 再领一次：v3 detail 已 checked_in -> 幂等跳过，不再打写接口
        let idem = adapter
            .claim_daily(&cred, None)
            .await
            .expect("already claimed");
        assert!(!idem.claimed);
        assert_eq!(
            idem.updated_quota.as_ref().map(|q| &q.claim_status),
            Some(&ClaimStatus::Claimed)
        );

        // Cards (chosen_type = 3) detection: requires interaction_type == 2 CheckIn button
        let cards_with_btn = DoubaoWorkAdapter::claim_status_from_landing(&serde_json::json!({
            "chosen_type": 3,
            "landing_cards": [{ "card_code": "daily_signin", "buttons": [{ "interaction_type": 2 }] }]
        }));
        assert_eq!(cards_with_btn, ClaimStatus::Available);

        let cards_without_btn = DoubaoWorkAdapter::claim_status_from_landing(&serde_json::json!({
            "chosen_type": 3,
            "landing_cards": [{ "card_code": "daily_signin", "buttons": [] }]
        }));
        assert_eq!(cards_without_btn, ClaimStatus::Unknown);
    }

    #[test]
    fn extract_landing_scene_ignores_campaign_code_and_reads_group_code() {
        let only_campaign_code = serde_json::json!({
            "chosen_type": 1,
            "extra_params": { "campaign_code": "sub_task_only" }
        });
        assert_eq!(
            DoubaoWorkAdapter::extract_landing_scene(&only_campaign_code),
            None
        );

        let top_group_code = serde_json::json!({
            "campaign_group_code": "pc_incentive_task_mvp_v3",
            "extra_params": { "campaign_code": "sub_task_only" }
        });
        assert_eq!(
            DoubaoWorkAdapter::extract_landing_scene(&top_group_code).as_deref(),
            Some("pc_incentive_task_mvp_v3")
        );

        let extra_group_code = serde_json::json!({
            "extra_params": {
                "campaign_code": "sub_task_only",
                "campaign_group_code": "extra_group_v1"
            }
        });
        assert_eq!(
            DoubaoWorkAdapter::extract_landing_scene(&extra_group_code).as_deref(),
            Some("extra_group_v1")
        );
    }

    #[test]
    fn max_campaign_trial_days_counts_only_active_gift_records() {
        // 生效中的赠送套餐，取 campaign_infos 里的天数
        let active_gift = serde_json::json!({
            "groups": [{
                "group_type": 1,
                "records": [{
                    "is_gift": true,
                    "status": 3,
                    "campaign_infos": [
                        { "campaign_scene": 1, "display_duration_days": 30,
                          "subscription_campaign_tag": "pc_gift" }
                    ]
                }]
            }]
        });
        assert_eq!(DoubaoWorkAdapter::max_campaign_trial_days(&active_gift), 30);

        // 自购（is_gift=false）不计入活动试用上限
        let paid = serde_json::json!({
            "groups": [{
                "records": [{
                    "is_gift": false,
                    "status": 3,
                    "campaign_infos": [{ "display_duration_days": 30 }]
                }]
            }]
        });
        assert_eq!(DoubaoWorkAdapter::max_campaign_trial_days(&paid), 0);

        // 非生效状态（status != 3）不计入
        let expired = serde_json::json!({
            "groups": [{
                "records": [{
                    "is_gift": true,
                    "status": 1,
                    "campaign_infos": [{ "display_duration_days": 30 }]
                }]
            }]
        });
        assert_eq!(DoubaoWorkAdapter::max_campaign_trial_days(&expired), 0);

        // 多条记录取最大值
        let multi = serde_json::json!({
            "groups": [{
                "records": [
                    { "is_gift": true, "status": 3,
                      "campaign_infos": [{ "display_duration_days": 7 }] },
                    { "is_gift": true, "status": 3,
                      "campaign_infos": [{ "display_duration_days": 20 }] }
                ]
            }]
        });
        assert_eq!(DoubaoWorkAdapter::max_campaign_trial_days(&multi), 20);

        // 结构缺失返回 0
        assert_eq!(
            DoubaoWorkAdapter::max_campaign_trial_days(&serde_json::json!({})),
            0
        );
    }

    #[test]
    fn campaign_trial_cap_boundary_is_strictly_greater_than_20() {
        assert!(!DoubaoWorkAdapter::campaign_trial_capped(0));
        assert!(!DoubaoWorkAdapter::campaign_trial_capped(20));
        // 超过 20 天才判超限（规则：上限 20 天）
        assert!(DoubaoWorkAdapter::campaign_trial_capped(21));
        assert!(DoubaoWorkAdapter::campaign_trial_capped(30));
    }

    #[tokio::test]
    async fn claim_daily_skips_write_when_campaign_trial_capped() {
        let server = MockServer::start().await;
        let adapter = DoubaoWorkAdapter::with_base_urls(server.uri(), server.uri());
        let cred = CredentialEnvelope {
            tool_id: "doubaowork".to_string(),
            uid: "778899".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "sessionid=valid-cookie; odin_tt=xyz".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };

        Mock::given(method("POST"))
            .and(path("/alice/ug/campaign/v3/detail/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "tasks": [{
                        "campaign_code": "daily_signin",
                        "task_status": 1,
                        "daily_checkin": { "checked_in_today": false, "continuous_days": 0 }
                    }]
                }
            })))
            .mount(&server)
            .await;

        // 账号持有 30 天活动赠送套餐（超过 20 天上限）
        Mock::given(method("POST"))
            .and(path("/alice/commerce/sale/subscription/list/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "groups": [{
                        "records": [{
                            "is_gift": true,
                            "status": 3,
                            "campaign_infos": [{
                                "campaign_scene": 1,
                                "display_duration_days": 30,
                                "subscription_campaign_tag": "pc_gift"
                            }]
                        }]
                    }]
                }
            })))
            .mount(&server)
            .await;

        // 关键断言：超限时绝不发起写请求
        Mock::given(method("POST"))
            .and(path("/alice/ug/campaign/landing/checkin"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": { "checked_in": true, "continuous_days": 1 }
            })))
            .expect(0)
            .mount(&server)
            .await;

        let res = adapter
            .claim_daily(&cred, None)
            .await
            .expect("capped claim");
        assert!(!res.claimed);
        assert!(
            res.reward_summary.contains("上限"),
            "summary={}",
            res.reward_summary
        );
        assert!(res.reward_summary.contains("30"), "{}", res.reward_summary);
    }

    #[tokio::test]
    async fn inspect_quota_marks_unsupported_when_trial_capped() {
        let server = MockServer::start().await;
        let adapter = DoubaoWorkAdapter::with_base_urls(server.uri(), server.uri());
        let cred = CredentialEnvelope {
            tool_id: "doubaowork".to_string(),
            uid: "778899".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "sessionid=valid-cookie; odin_tt=xyz".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };

        Mock::given(method("POST"))
            .and(path("/alice/commerce/sale/subscription/quota/summary/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": { "member_info": { "status": 3, "member_type": 0 } }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/alice/ug/campaign/v3/detail/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "tasks": [{
                        "campaign_code": "daily_signin",
                        "daily_checkin": { "checked_in_today": false, "continuous_days": 0 }
                    }]
                }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/alice/commerce/sale/subscription/list/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "groups": [{
                        "records": [{
                            "is_gift": true,
                            "status": 3,
                            "campaign_infos": [{ "display_duration_days": 30 }]
                        }]
                    }]
                }
            })))
            .mount(&server)
            .await;

        let q = adapter.inspect_quota(&cred).await.expect("inspect");
        // v3 说未签（Available），但超限覆盖为 Unsupported，UI 不再显示可领
        assert_eq!(q.claim_status, ClaimStatus::Unsupported);
    }

    #[test]
    fn daily_checkin_state_reads_authoritative_v3_field() {
        let v3 = serde_json::json!({
            "tasks": [
                { "campaign_code": "points_milestone_500", "task_status": 1 },
                {
                    "campaign_code": "daily_signin",
                    "task_status": 3,
                    "daily_checkin": { "checked_in_today": true, "continuous_days": 7 }
                }
            ]
        });
        assert_eq!(DoubaoWorkAdapter::daily_checkin_state(&v3), Some((true, 7)));

        let not_checked = serde_json::json!({
            "tasks": [{
                "campaign_code": "daily_signin",
                "daily_checkin": { "checked_in_today": false, "continuous_days": 0 }
            }]
        });
        assert_eq!(
            DoubaoWorkAdapter::daily_checkin_state(&not_checked),
            Some((false, 0))
        );

        // 无签到任务投放 -> None（区别于「没签」）
        let no_task = serde_json::json!({ "tasks": [{ "campaign_code": "points_milestone_500" }] });
        assert_eq!(DoubaoWorkAdapter::daily_checkin_state(&no_task), None);
    }

    #[tokio::test]
    async fn claim_daily_attaches_local_state_device_id_and_client_runtime() {
        use wiremock::matchers::{header, query_param};

        let root = std::env::temp_dir().join(format!("qwitch_doubao_did_{}", Uuid::new_v4()));
        let profile_dir = root.join("doubaowork");
        std::fs::create_dir_all(profile_dir.join("Default")).unwrap();
        std::fs::write(
            profile_dir.join("Default/Preferences"),
            r#"{"saman":{"last_user":{"user_id":"778899"}}}"#,
        )
        .unwrap();
        std::fs::write(
            profile_dir.join("Local State"),
            serde_json::json!({
                "aha": {
                    "device": {
                        "device_id": "284592019582341",
                        "install_id": "4910283746192045"
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let server = MockServer::start().await;
        let host: Arc<dyn PlatformHost> = Arc::new(DummyHost { root: root.clone() });
        let adapter = DoubaoWorkAdapter::with_base_urls_and_host(
            server.uri(),
            server.uri(),
            Arc::clone(&host),
        );
        let cred = CredentialEnvelope {
            tool_id: "doubaowork".to_string(),
            uid: "778899".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "sessionid=valid-cookie; odin_tt=xyz".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };

        Mock::given(method("POST"))
            .and(path("/alice/ug/campaign/v3/detail/"))
            .and(header("agw-js-conv", "str"))
            .and(query_param("device_id", "284592019582341"))
            .and(query_param("iid", "4910283746192045"))
            .and(query_param("web_id", "284592019582341"))
            .and(query_param("tea_uuid", "284592019582341"))
            .and(query_param("runtime", "web"))
            .and(query_param("runtime_version", "3.46.7"))
            .respond_with(CheckinStateResponder::new(1, 1))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/alice/commerce/sale/subscription/list/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": { "groups": [] }
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/alice/ug/campaign/landing/get"))
            .and(header("agw-js-conv", "str"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "chosen_type": 3,
                    "landing_cards": [{
                        "card_code": "daily_signin",
                        "buttons": [{ "interaction_type": 2, "button_id": "checkin" }]
                    }]
                }
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/alice/ug/campaign/landing/checkin"))
            .and(header("agw-js-conv", "str"))
            .and(query_param("device_id", "284592019582341"))
            .and(query_param("iid", "4910283746192045"))
            .and(query_param("web_id", "284592019582341"))
            .and(query_param("tea_uuid", "284592019582341"))
            .and(query_param("runtime", "web"))
            .and(query_param("runtime_version", "3.46.7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": { "checked_in": true, "continuous_days": 1 }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        let res = adapter
            .claim_daily(&cred, None)
            .await
            .expect("claim with client-aligned device params");
        assert!(res.claimed);

        let recorded = server.received_requests().await.unwrap();
        let checkin_req = recorded
            .iter()
            .find(|r| r.url.path() == "/alice/ug/campaign/landing/checkin")
            .expect("recorded checkin request");
        let web_tab_id = checkin_req
            .url
            .query_pairs()
            .find(|(k, _)| k == "web_tab_id")
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default();
        assert!(!web_tab_id.is_empty(), "应生成非空 web_tab_id");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn check_agw_code_maps_doubao_d_a9_auth_risk_and_rate_limit_codes() {
        for code in [710012001i64, 710012006, 710012020] {
            let err = DoubaoWorkAdapter::check_agw_code(
                &serde_json::json!({ "code": code, "msg": "auth" }),
                "测试",
                true,
            )
            .unwrap_err();
            assert!(matches!(
                err,
                AdapterError::Unified {
                    code: UnifiedErrorCode::AuthExpired,
                    ..
                }
            ));
        }

        let risk_err = DoubaoWorkAdapter::check_agw_code(
            &serde_json::json!({ "code": 710022002, "msg": "blocked" }),
            "测试",
            true,
        )
        .unwrap_err();
        assert!(matches!(
            risk_err,
            AdapterError::Unified {
                code: UnifiedErrorCode::RiskTriggered,
                ..
            }
        ));

        let rate_err = DoubaoWorkAdapter::check_agw_code(
            &serde_json::json!({ "code": 710012007, "msg": "wait" }),
            "测试",
            true,
        )
        .unwrap_err();
        assert!(matches!(
            rate_err,
            AdapterError::Unified {
                code: UnifiedErrorCode::RateLimited,
                ..
            }
        ));
    }

    #[test]
    fn merge_set_cookies_updates_adds_and_skips_unchanged() {
        let base = "sessionid=old_s; sid_tt=old_s; extra=keep";
        assert!(merge_set_cookies(base, &[]).is_none());
        assert!(merge_set_cookies(base, &["sessionid=old_s; Path=/; HttpOnly".into()]).is_none());

        // Max-Age=0 清退头不应误删现有有效会话
        assert!(merge_set_cookies(base, &["sessionid=; Max-Age=0; Path=/".into()]).is_none());

        let merged = merge_set_cookies(
            base,
            &[
                "sessionid=new_s; Path=/; HttpOnly".into(),
                "uid_tt=u123; Path=/; Secure".into(),
            ],
        )
        .expect("should merge rotated and new cookies");
        assert_eq!(
            merged,
            "sessionid=new_s; sid_tt=old_s; extra=keep; uid_tt=u123"
        );
    }

    #[tokio::test]
    async fn refresh_credential_merges_set_cookie_and_detects_expired() {
        let server = MockServer::start().await;
        let adapter = DoubaoWorkAdapter::with_base_urls(server.uri(), server.uri());
        let cred = CredentialEnvelope {
            tool_id: "doubaowork".to_string(),
            uid: "u_db_1".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "sessionid=old_sid; sid_tt=old_sid".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };

        // 1. 服务端下发 Set-Cookie 轮换 -> 返回更新后的 CredentialEnvelope
        Mock::given(method("GET"))
            .and(path("/passport/account/info/v2/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("set-cookie", "sessionid=new_sid; Path=/; HttpOnly")
                    .set_body_json(serde_json::json!({
                        "message": "success",
                        "data": { "user_id_str": "u_db_1", "error_code": 0 }
                    })),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;

        let updated = adapter
            .refresh_credential(&cred)
            .await
            .expect("keepalive ok")
            .expect("should return updated cookie");
        match &updated.payload {
            CredentialPayload::Token { access_token, .. } => {
                assert!(access_token.contains("sessionid=new_sid"));
                assert!(access_token.contains("sid_tt=old_sid"));
            }
            _ => panic!("expected Token"),
        }

        // 2. 无 Set-Cookie 变更 -> Ok(None)
        Mock::given(method("GET"))
            .and(path("/passport/account/info/v2/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": "success",
                "data": { "user_id_str": "u_db_1", "error_code": 0 }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        assert!(adapter
            .refresh_credential(&updated)
            .await
            .expect("keepalive ok")
            .is_none());

        // 3. error_code != 0 -> AuthExpired
        Mock::given(method("GET"))
            .and(path("/passport/account/info/v2/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": "error",
                "data": { "error_code": 10008 }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let err = adapter.refresh_credential(&updated).await.unwrap_err();
        assert!(matches!(
            err,
            AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                ..
            }
        ));
    }
}
