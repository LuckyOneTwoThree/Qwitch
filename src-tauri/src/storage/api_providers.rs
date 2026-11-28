use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;
use uuid::Uuid;

use crate::domain::{
    is_api_provider_tool_id, mask_api_key, normalize_api_provider_slug, AccountStatus,
    AccountSummary, AddApiProviderKeyInput, ApiAuthScheme, ApiManualModelEntry, ApiProviderRecord,
    CredentialEnvelope, CredentialPayload, SaveApiProviderInput, UpdateApiProviderKeyInput,
    WireProtocolKind,
};
use crate::storage::{crypto, QwitchDatabase, StorageError};

fn parse_wire_protocol(raw: &str) -> WireProtocolKind {
    match raw.trim().to_ascii_lowercase().as_str() {
        "anthropic_messages" => WireProtocolKind::AnthropicMessages,
        "openai_responses" => WireProtocolKind::OpenaiResponses,
        _ => WireProtocolKind::OpenaiChat,
    }
}

fn wire_protocol_to_str(p: WireProtocolKind) -> &'static str {
    match p {
        WireProtocolKind::OpenaiChat => "openai_chat",
        WireProtocolKind::AnthropicMessages => "anthropic_messages",
        WireProtocolKind::OpenaiResponses => "openai_responses",
    }
}

fn row_to_api_provider(row: &rusqlite::Row<'_>) -> Result<ApiProviderRecord, rusqlite::Error> {
    let extra_raw: String = row.get(7)?;
    let manual_raw: String = row.get(9)?;
    let extra_headers: HashMap<String, String> =
        serde_json::from_str(&extra_raw).unwrap_or_default();
    let manual_models: Vec<ApiManualModelEntry> =
        serde_json::from_str(&manual_raw).unwrap_or_default();
    let proto_raw: String = row.get(4)?;
    let auth_raw: String = row.get(5)?;
    let latency_opt: Option<i64> = row.get(15)?;
    Ok(ApiProviderRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        preset_id: row.get(2)?,
        base_url: row.get(3)?,
        wire_protocol: parse_wire_protocol(&proto_raw),
        auth_scheme: ApiAuthScheme::from_str_lossy(&auth_raw),
        auth_header_name: row.get(6)?,
        extra_headers,
        models_path: row.get(8)?,
        manual_models,
        allow_lan_http: row.get::<_, i64>(10)? != 0,
        accent_color: row.get(11)?,
        icon: row.get(12)?,
        enabled: row.get::<_, i64>(13)? != 0,
        last_test_status: row.get(14)?,
        last_test_latency_ms: latency_opt.map(|v| v.max(0) as u64),
        last_test_error: row.get(16)?,
        last_tested_at: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
    })
}

fn build_provider_extra(provider: &ApiProviderRecord) -> HashMap<String, String> {
    let mut extra = HashMap::new();
    extra.insert("base_url".into(), provider.base_url.trim().to_string());
    extra.insert(
        "wire_protocol".into(),
        wire_protocol_to_str(provider.wire_protocol).to_string(),
    );
    extra.insert(
        "auth_scheme".into(),
        provider.auth_scheme.as_str().to_string(),
    );
    if let Some(hdr) = provider
        .auth_header_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        extra.insert("auth_header_name".into(), hdr.to_string());
    }
    if provider.allow_lan_http {
        extra.insert("allow_lan_http".into(), "true".into());
    }
    if let Some(mp) = provider
        .models_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        extra.insert("models_path".into(), mp.to_string());
    }
    if !provider.extra_headers.is_empty() {
        if let Ok(json) = serde_json::to_string(&provider.extra_headers) {
            extra.insert("extra_headers_json".into(), json);
        }
    }
    extra
}

impl QwitchDatabase {
    pub fn list_api_providers(&self) -> Result<Vec<ApiProviderRecord>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, preset_id, base_url, wire_protocol, auth_scheme,
                    auth_header_name, extra_headers_json, models_path, manual_models_json,
                    allow_lan_http, accent_color, icon, enabled, last_test_status,
                    last_test_latency_ms, last_test_error, last_tested_at, created_at, updated_at
             FROM api_providers
             ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map([], row_to_api_provider)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_api_provider(&self, id: &str) -> Result<Option<ApiProviderRecord>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.query_row(
            "SELECT id, name, preset_id, base_url, wire_protocol, auth_scheme,
                    auth_header_name, extra_headers_json, models_path, manual_models_json,
                    allow_lan_http, accent_color, icon, enabled, last_test_status,
                    last_test_latency_ms, last_test_error, last_tested_at, created_at, updated_at
             FROM api_providers
             WHERE id = ?1",
            params![id],
            row_to_api_provider,
        )
        .optional()
        .map_err(StorageError::from)
    }

    pub fn upsert_api_provider(
        &self,
        master_key: &[u8; 32],
        input: SaveApiProviderInput,
    ) -> Result<ApiProviderRecord, StorageError> {
        let name = input.name.trim().to_string();
        if name.is_empty() {
            return Err(StorageError::Validation("厂商名称不能为空".into()));
        }
        let base_url = input.base_url.trim().to_string();
        if base_url.is_empty() {
            return Err(StorageError::Validation("Base URL 不能为空".into()));
        }
        let now = Utc::now().to_rfc3339();

        let (id, existing) = if let Some(explicit_id) = input
            .id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if !is_api_provider_tool_id(explicit_id) {
                return Err(StorageError::Validation(format!(
                    "API 厂商 ID 必须以 api_ 开头: {explicit_id}"
                )));
            }
            let found = self.get_api_provider(explicit_id)?;
            (explicit_id.to_string(), found)
        } else {
            let base_slug = normalize_api_provider_slug(
                input.preset_id.as_deref().unwrap_or(name.as_str()),
            );
            let existing_list = self.list_api_providers()?;
            let mut candidate = base_slug.clone();
            let mut idx = 2usize;
            while existing_list.iter().any(|p| p.id == candidate) {
                candidate = format!("{base_slug}_{idx}");
                idx += 1;
            }
            (candidate, None)
        };

        let enabled = input
            .enabled
            .unwrap_or_else(|| existing.as_ref().is_none_or(|e| e.enabled));
        let created_at = existing
            .as_ref()
            .map(|e| e.created_at.clone())
            .unwrap_or_else(|| now.clone());
        let last_test_status = existing.as_ref().and_then(|e| e.last_test_status.clone());
        let last_test_latency_ms = existing.as_ref().and_then(|e| e.last_test_latency_ms);
        let last_test_error = existing.as_ref().and_then(|e| e.last_test_error.clone());
        let last_tested_at = existing.as_ref().and_then(|e| e.last_tested_at.clone());

        let clean_headers: HashMap<String, String> = input
            .extra_headers
            .into_iter()
            .filter(|(k, v)| !k.trim().is_empty() && !v.trim().is_empty())
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        let clean_manual: Vec<ApiManualModelEntry> = input
            .manual_models
            .into_iter()
            .filter(|m| !m.id.trim().is_empty())
            .map(|mut m| {
                m.id = m.id.trim().to_string();
                m
            })
            .collect();

        let extra_headers_json = serde_json::to_string(&clean_headers)?;
        let manual_models_json = serde_json::to_string(&clean_manual)?;
        let auth_header_name = input
            .auth_header_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let models_path = input
            .models_path
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let preset_id = input
            .preset_id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let accent_color = input
            .accent_color
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let icon = input
            .icon
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        {
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            conn.execute(
                "INSERT INTO api_providers (
                    id, name, preset_id, base_url, wire_protocol, auth_scheme,
                    auth_header_name, extra_headers_json, models_path, manual_models_json,
                    allow_lan_http, accent_color, icon, enabled, last_test_status,
                    last_test_latency_ms, last_test_error, last_tested_at, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    preset_id = excluded.preset_id,
                    base_url = excluded.base_url,
                    wire_protocol = excluded.wire_protocol,
                    auth_scheme = excluded.auth_scheme,
                    auth_header_name = excluded.auth_header_name,
                    extra_headers_json = excluded.extra_headers_json,
                    models_path = excluded.models_path,
                    manual_models_json = excluded.manual_models_json,
                    allow_lan_http = excluded.allow_lan_http,
                    accent_color = excluded.accent_color,
                    icon = excluded.icon,
                    enabled = excluded.enabled,
                    updated_at = excluded.updated_at",
                params![
                    id,
                    name,
                    preset_id,
                    base_url,
                    wire_protocol_to_str(input.wire_protocol),
                    input.auth_scheme.as_str(),
                    auth_header_name,
                    extra_headers_json,
                    models_path,
                    manual_models_json,
                    if input.allow_lan_http { 1 } else { 0 },
                    accent_color,
                    icon,
                    if enabled { 1 } else { 0 },
                    last_test_status,
                    last_test_latency_ms.map(|v| v as i64),
                    last_test_error,
                    last_tested_at,
                    created_at,
                    now,
                ],
            )?;
        }

        let saved = self
            .get_api_provider(&id)?
            .ok_or_else(|| StorageError::NotFound(format!("api_provider {id}")))?;

        // 更新后把最新 Base URL / 协议 / 鉴权配置同步到下属已有 Key 凭据
        self.sync_provider_metadata_to_accounts(master_key, &saved)?;

        if let Some(initial_key) = input
            .initial_api_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.add_api_provider_key(
                master_key,
                AddApiProviderKeyInput {
                    provider_id: saved.id.clone(),
                    label: input.initial_key_label,
                    api_key: initial_key.to_string(),
                },
            )?;
        }

        Ok(saved)
    }

    pub fn set_api_provider_enabled(&self, id: &str, enabled: bool) -> Result<(), StorageError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let changed = conn.execute(
            "UPDATE api_providers SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
            params![if enabled { 1 } else { 0 }, now, id],
        )?;
        if changed == 0 {
            return Err(StorageError::NotFound(format!("api_provider {id}")));
        }
        Ok(())
    }

    pub fn save_api_provider_manual_models(
        &self,
        id: &str,
        models: Vec<ApiManualModelEntry>,
    ) -> Result<ApiProviderRecord, StorageError> {
        let now = Utc::now().to_rfc3339();
        let clean: Vec<ApiManualModelEntry> = models
            .into_iter()
            .filter(|m| !m.id.trim().is_empty())
            .map(|mut m| {
                m.id = m.id.trim().to_string();
                m
            })
            .collect();
        let json = serde_json::to_string(&clean)?;
        {
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            let changed = conn.execute(
                "UPDATE api_providers SET manual_models_json = ?1, updated_at = ?2 WHERE id = ?3",
                params![json, now, id],
            )?;
            if changed == 0 {
                return Err(StorageError::NotFound(format!("api_provider {id}")));
            }
        }
        self.get_api_provider(id)?
            .ok_or_else(|| StorageError::NotFound(format!("api_provider {id}")))
    }

    pub fn update_api_provider_test_result(
        &self,
        id: &str,
        ok: bool,
        latency_ms: u64,
        error: Option<&str>,
    ) -> Result<(), StorageError> {
        let now = Utc::now().to_rfc3339();
        let status = if ok { "ok" } else { "failed" };
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.execute(
            "UPDATE api_providers
             SET last_test_status = ?1,
                 last_test_latency_ms = ?2,
                 last_test_error = ?3,
                 last_tested_at = ?4,
                 updated_at = ?4
             WHERE id = ?5",
            params![status, latency_ms as i64, error, now, id],
        )?;
        Ok(())
    }

    pub fn delete_api_provider(&self, id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.execute(
            "DELETE FROM account_discovered_models WHERE tool_id = ?1",
            params![id],
        )?;
        conn.execute("DELETE FROM accounts WHERE tool_id = ?1", params![id])?;
        let deleted = conn.execute("DELETE FROM api_providers WHERE id = ?1", params![id])?;
        if deleted == 0 {
            return Err(StorageError::NotFound(format!("api_provider {id}")));
        }
        Ok(())
    }

    pub fn add_api_provider_key(
        &self,
        master_key: &[u8; 32],
        input: AddApiProviderKeyInput,
    ) -> Result<AccountSummary, StorageError> {
        let provider = self
            .get_api_provider(&input.provider_id)?
            .ok_or_else(|| StorageError::NotFound(format!("api_provider {}", input.provider_id)))?;
        let api_key = input.api_key.trim();
        if api_key.is_empty() {
            return Err(StorageError::Validation("API Key 不能为空".into()));
        }

        let uid_hash = crypto::compute_uid_hash_hmac(master_key, &provider.id, api_key);
        let masked = mask_api_key(api_key);
        let now = Utc::now().to_rfc3339();
        let extra = build_provider_extra(&provider);

        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let existing_id: Option<String> = conn
            .query_row(
                "SELECT id FROM accounts WHERE tool_id = ?1 AND uid_hash = ?2",
                params![provider.id, uid_hash],
                |r| r.get(0),
            )
            .optional()?;

        if existing_id.is_some() {
            return Err(StorageError::Validation(
                "该厂商下已存在相同的 API Key，请勿重复添加".into(),
            ));
        }

        let existing_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM accounts WHERE tool_id = ?1",
            params![provider.id],
            |r| r.get(0),
        )?;
        let label = input
            .label
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("{} Key #{}", provider.name, existing_count + 1));

        let account_id = Uuid::new_v4().to_string();
        let envelope = CredentialEnvelope {
            tool_id: provider.id.clone(),
            uid: uid_hash.clone(),
            display_name: Some(label.clone()),
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: api_key.to_string(),
                refresh_token: None,
                expires_at: None,
                extra,
            },
        };
        let aad = crypto::credential_aad(&provider.id, &account_id);
        let encrypted = crypto::encrypt_credential(master_key, &aad, &envelope)?;

        conn.execute(
            "INSERT INTO accounts (
                id, tool_id, label, account_name, uid_masked, uid_hash, email_masked,
                status, is_active_on_client, has_local_snapshot, has_api_token,
                device_id_masked, device_type, encrypted_credential,
                credential_expires_at, renewal_mode, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 'active', 0, 0, 1, NULL, NULL, ?7, NULL, 'manual', ?8, ?8)",
            params![
                account_id,
                provider.id,
                label,
                label,
                masked,
                uid_hash,
                encrypted,
                now,
            ],
        )?;
        drop(conn);

        self.get_account_summary(&account_id)
    }

    pub fn update_api_provider_key(
        &self,
        master_key: &[u8; 32],
        input: UpdateApiProviderKeyInput,
    ) -> Result<AccountSummary, StorageError> {
        let (summary, mut env) = self.get_account_credential(master_key, &input.account_id)?;
        if !is_api_provider_tool_id(&summary.tool_id) {
            return Err(StorageError::Validation(
                "仅 API 资产 Key 支持此更新接口".into(),
            ));
        }
        let now = Utc::now().to_rfc3339();
        let new_label = input
            .label
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let new_key = input
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);

        if let Some(ref key_val) = new_key {
            let new_uid_hash = crypto::compute_uid_hash_hmac(master_key, &summary.tool_id, key_val);
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            let dup: Option<String> = conn
                .query_row(
                    "SELECT id FROM accounts WHERE tool_id = ?1 AND uid_hash = ?2 AND id != ?3",
                    params![summary.tool_id, new_uid_hash, summary.id],
                    |r| r.get(0),
                )
                .optional()?;
            if dup.is_some() {
                return Err(StorageError::Validation(
                    "该厂商下已存在相同的 API Key".into(),
                ));
            }
            drop(conn);
            env.uid = new_uid_hash.clone();
            if let CredentialPayload::Token { access_token, .. } = &mut env.payload {
                *access_token = key_val.clone();
            }
            let aad = crypto::credential_aad(&summary.tool_id, &summary.id);
            let encrypted = crypto::encrypt_credential(master_key, &aad, &env)?;
            let masked = mask_api_key(key_val);
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            conn.execute(
                "UPDATE accounts
                 SET uid_hash = ?1,
                     uid_masked = ?2,
                     encrypted_credential = ?3,
                     status = CASE WHEN status = 'expired' THEN 'active' ELSE status END,
                     updated_at = ?4
                 WHERE id = ?5",
                params![new_uid_hash, masked, encrypted, now, summary.id],
            )?;
        }

        if let Some(lbl) = new_label {
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            conn.execute(
                "UPDATE accounts SET label = ?1, account_name = ?1, updated_at = ?2 WHERE id = ?3",
                params![lbl, now, summary.id],
            )?;
        }

        self.get_account_summary(&summary.id)
    }

    pub fn sync_provider_metadata_to_accounts(
        &self,
        master_key: &[u8; 32],
        provider: &ApiProviderRecord,
    ) -> Result<(), StorageError> {
        let accounts = self.list_accounts(Some(&provider.id))?;
        let synced_extra = build_provider_extra(provider);
        for acc in accounts {
            let Ok((_, mut env)) = self.get_account_credential(master_key, &acc.id) else {
                continue;
            };
            if let CredentialPayload::Token { extra, .. } = &mut env.payload {
                *extra = synced_extra.clone();
            } else {
                continue;
            }
            let aad = crypto::credential_aad(&acc.tool_id, &acc.id);
            let encrypted = crypto::encrypt_credential(master_key, &aad, &env)?;
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            conn.execute(
                "UPDATE accounts SET encrypted_credential = ?1 WHERE id = ?2",
                params![encrypted, acc.id],
            )?;
        }
        Ok(())
    }

    /// 将历史 `custom_api_key` 账号平滑迁移至 `api_custom`（「自定义 API」）厂商。
    pub fn migrate_legacy_custom_api_key(
        &self,
        master_key: &[u8; 32],
    ) -> Result<usize, StorageError> {
        let legacy_accounts = self.list_accounts(Some("custom_api_key"))?;
        if legacy_accounts.is_empty() {
            return Ok(0);
        }

        let target_id = "api_custom";
        let now = Utc::now().to_rfc3339();
        let mut first_base_url = "https://api.openai.com".to_string();
        let mut decrypted_items: Vec<(String, String, CredentialEnvelope)> = Vec::new();

        for acc in &legacy_accounts {
            if let Ok((_, mut env)) = self.get_account_credential(master_key, &acc.id) {
                if let CredentialPayload::Token { extra, .. } = &env.payload {
                    if let Some(b) = extra.get("base_url").filter(|s| !s.trim().is_empty()) {
                        first_base_url = b.clone();
                    }
                }
                env.tool_id = target_id.to_string();
                let new_uid_hash = if acc.uid_masked.is_empty() {
                    crypto::compute_uid_hash_hmac(master_key, target_id, &env.uid)
                } else {
                    match &env.payload {
                        CredentialPayload::Token { access_token, .. } => {
                            crypto::compute_uid_hash_hmac(master_key, target_id, access_token)
                        }
                        _ => crypto::compute_uid_hash_hmac(master_key, target_id, &env.uid),
                    }
                };
                decrypted_items.push((acc.id.clone(), new_uid_hash, env));
            }
        }

        if self.get_api_provider(target_id)?.is_none() {
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            conn.execute(
                "INSERT INTO api_providers (
                    id, name, preset_id, base_url, wire_protocol, auth_scheme,
                    auth_header_name, extra_headers_json, models_path, manual_models_json,
                    allow_lan_http, accent_color, icon, enabled, created_at, updated_at
                 ) VALUES (?1, '自定义 API', 'custom', ?2, 'openai_chat', 'bearer', NULL, '{}', NULL, '[]', 0, '#71717a', NULL, 1, ?3, ?3)",
                params![target_id, first_base_url, now],
            )?;
        }

        let mut migrated = 0usize;
        for (account_id, new_uid_hash, env) in decrypted_items {
            let new_aad = crypto::credential_aad(target_id, &account_id);
            let encrypted = crypto::encrypt_credential(master_key, &new_aad, &env)?;
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            conn.execute(
                "UPDATE accounts
                 SET tool_id = ?1, uid_hash = ?2, encrypted_credential = ?3, updated_at = ?4
                 WHERE id = ?5",
                params![target_id, new_uid_hash, encrypted, now, account_id],
            )?;
            conn.execute(
                "UPDATE account_discovered_models SET tool_id = ?1 WHERE account_id = ?2",
                params![target_id, account_id],
            )?;
            migrated += 1;
        }

        let _ = AccountStatus::Active;
        Ok(migrated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::upstream_model_protocol;
    use crate::plugins::PluginRegistry;

    fn temp_db_and_key() -> (QwitchDatabase, [u8; 32], std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("qwitch_api_prov_{}", Uuid::new_v4()));
        let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open db");
        let key = [7u8; 32];
        (db, key, dir)
    }

    #[test]
    fn api_provider_crud_multi_key_and_registry_sync() {
        let (db, key, dir) = temp_db_and_key();

        let provider = db
            .upsert_api_provider(
                &key,
                SaveApiProviderInput {
                    id: None,
                    name: "DeepSeek".into(),
                    preset_id: Some("deepseek".into()),
                    base_url: "https://api.deepseek.com".into(),
                    wire_protocol: WireProtocolKind::OpenaiChat,
                    auth_scheme: ApiAuthScheme::Bearer,
                    auth_header_name: None,
                    extra_headers: HashMap::new(),
                    models_path: None,
                    manual_models: vec![ApiManualModelEntry {
                        id: "deepseek-chat".into(),
                        display_name: Some("DeepSeek Chat".into()),
                        vendor_wire_id: None,
                        context_window: Some(64000),
                        max_output_tokens: Some(8192),
                    }],
                    allow_lan_http: false,
                    accent_color: Some("#4d6bfe".into()),
                    icon: None,
                    enabled: Some(true),
                    initial_api_key: Some("sk-deepseek-first-0001".into()),
                    initial_key_label: Some("主力 Key".into()),
                },
            )
            .expect("save provider");
        assert_eq!(provider.id, "api_deepseek");

        // 同厂商追加第二把 Key，且重复 Key 被 HMAC 拦截
        let k2 = db
            .add_api_provider_key(
                &key,
                AddApiProviderKeyInput {
                    provider_id: provider.id.clone(),
                    label: Some("备用 Key".into()),
                    api_key: "sk-deepseek-second-0002".into(),
                },
            )
            .expect("add second key");
        assert_eq!(k2.tool_id, "api_deepseek");
        assert!(db
            .add_api_provider_key(
                &key,
                AddApiProviderKeyInput {
                    provider_id: provider.id.clone(),
                    label: None,
                    api_key: "sk-deepseek-first-0001".into(),
                },
            )
            .is_err());

        // 更新 Key
        let updated = db
            .update_api_provider_key(
                &key,
                UpdateApiProviderKeyInput {
                    account_id: k2.id.clone(),
                    label: Some("备用 Key 改名".into()),
                    api_key: Some("sk-deepseek-second-9999".into()),
                },
            )
            .expect("update key");
        assert_eq!(updated.label, "备用 Key 改名");
        assert!(updated.uid_masked.ends_with("9999"));

        // 同步到 PluginRegistry + 候选账号为 T1Primary 平权
        let mut reg = PluginRegistry::new_with_builtins();
        let all = db.list_api_providers().unwrap();
        reg.sync_api_providers(&all);
        crate::plugins::sync_global_registry_snapshot(&reg);
        assert!(reg.get_egress_transport("api_deepseek").is_some());
        assert_eq!(
            upstream_model_protocol("api_deepseek", "claude-3-5-sonnet"),
            WireProtocolKind::OpenaiChat
        );

        let candidates = db.list_upstream_candidates(&key).unwrap();
        let ds_cands: Vec<_> = candidates
            .iter()
            .filter(|c| c.tool_id == "api_deepseek")
            .collect();
        assert_eq!(ds_cands.len(), 2);
        assert!(ds_cands
            .iter()
            .all(|c| c.default_tier == crate::domain::MemberTier::T1Primary));

        // 级联删除
        db.delete_api_provider("api_deepseek").unwrap();
        assert!(db.list_accounts(Some("api_deepseek")).unwrap().is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrates_legacy_custom_api_key_accounts() {
        let (db, key, dir) = temp_db_and_key();
        let mut extra = HashMap::new();
        extra.insert("base_url".to_string(), "https://relay.example.com".to_string());
        let env = CredentialEnvelope {
            tool_id: "custom_api_key".into(),
            uid: "legacy-key-1".into(),
            display_name: Some("老 API Key".into()),
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "sk-legacy-123456".into(),
                refresh_token: None,
                expires_at: None,
                extra,
            },
        };
        db.upsert_account(&key, Some("老 API Key"), &env).unwrap();
        let migrated = db.migrate_legacy_custom_api_key(&key).unwrap();
        assert_eq!(migrated, 1);
        assert!(db.list_accounts(Some("custom_api_key")).unwrap().is_empty());
        let new_list = db.list_accounts(Some("api_custom")).unwrap();
        assert_eq!(new_list.len(), 1);
        let (_, cred) = db.get_account_credential(&key, &new_list[0].id).unwrap();
        assert_eq!(cred.tool_id, "api_custom");

        let _ = std::fs::remove_dir_all(dir);
    }
}
