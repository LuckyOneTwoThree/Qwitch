pub mod crypto;

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;
use thiserror::Error;
use uuid::Uuid;

use crate::domain::{
    AccountStatus, AccountSummary, ActionLogItem, ClaimStatus, CredentialEnvelope, QuotaBucket,
    QuotaSnapshot, UnifiedErrorCode,
};

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Crypto error: {0}")]
    Crypto(String),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Record not found: {0}")]
    NotFound(String),
    #[error("Internal lock poisoned")]
    LockPoisoned,
}

pub struct QwitchDatabase {
    conn: Mutex<Connection>,
}

impl QwitchDatabase {
    pub fn open(db_path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StorageError::Crypto(e.to_string()))?;
        }
        let conn = Connection::open(db_path)?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS accounts (
                id TEXT PRIMARY KEY,
                tool_id TEXT NOT NULL,
                label TEXT NOT NULL,
                uid_masked TEXT NOT NULL,
                uid_hash TEXT NOT NULL,
                email_masked TEXT,
                status TEXT NOT NULL DEFAULT 'active',
                is_active_on_client INTEGER NOT NULL DEFAULT 0,
                has_local_snapshot INTEGER NOT NULL DEFAULT 1,
                has_api_token INTEGER NOT NULL DEFAULT 1,
                encrypted_credential BLOB NOT NULL,
                last_claimed_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(tool_id, uid_hash)
            );

            CREATE TABLE IF NOT EXISTS quota_snapshots (
                id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                plan_name TEXT,
                claim_status TEXT NOT NULL DEFAULT 'unknown',
                buckets_json TEXT NOT NULL,
                fetched_at TEXT NOT NULL,
                FOREIGN KEY (account_id) REFERENCES accounts(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS action_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                account_id TEXT,
                tool_id TEXT NOT NULL,
                action_type TEXT NOT NULL,
                result_status TEXT NOT NULL,
                error_code TEXT,
                message_sanitized TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_accounts_tool ON accounts(tool_id);
            CREATE INDEX IF NOT EXISTS idx_quota_account ON quota_snapshots(account_id, fetched_at DESC);
            CREATE INDEX IF NOT EXISTS idx_logs_created ON action_logs(created_at DESC);
            ",
        )?;
        let _ = conn.execute(
            "ALTER TABLE accounts ADD COLUMN has_local_snapshot INTEGER NOT NULL DEFAULT 1",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE accounts ADD COLUMN has_api_token INTEGER NOT NULL DEFAULT 1",
            [],
        );
        Ok(())
    }

    pub fn upsert_account(
        &self,
        master_key: &[u8; 32],
        label: &str,
        envelope: &CredentialEnvelope,
    ) -> Result<AccountSummary, StorageError> {
        let now = Utc::now().to_rfc3339();
        let uid_hash = crypto::compute_uid_hash(&envelope.tool_id, &envelope.uid);

        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;

        let existing: Option<(String, Vec<u8>)> = conn
            .query_row(
                "SELECT id, encrypted_credential FROM accounts WHERE tool_id = ?1 AND uid_hash = ?2",
                params![envelope.tool_id, uid_hash],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        let account_id = match existing {
            Some((id, old_blob)) => {
                let merged = match crypto::decrypt_credential(master_key, &old_blob) {
                    Ok(old_env) => old_env.merge_from(envelope),
                    Err(_) => envelope.clone(),
                };
                let uid_masked = crypto::mask_uid(&merged.uid);
                let email_masked = merged.display_email.as_deref().map(crypto::mask_email);
                let encrypted_cred = crypto::encrypt_credential(master_key, &merged)?;
                let (has_snap, has_tok) = merged.capability_flags();

                conn.execute(
                    "UPDATE accounts
                     SET label = ?1, uid_masked = ?2, email_masked = ?3, status = 'active',
                         encrypted_credential = ?4, has_local_snapshot = ?5, has_api_token = ?6,
                         updated_at = ?7
                     WHERE id = ?8",
                    params![
                        label,
                        uid_masked,
                        email_masked,
                        encrypted_cred,
                        has_snap as i64,
                        has_tok as i64,
                        now,
                        id
                    ],
                )?;
                id
            }
            None => {
                let id = Uuid::new_v4().to_string();
                let uid_masked = crypto::mask_uid(&envelope.uid);
                let email_masked = envelope.display_email.as_deref().map(crypto::mask_email);
                let encrypted_cred = crypto::encrypt_credential(master_key, envelope)?;
                let (has_snap, has_tok) = envelope.capability_flags();

                conn.execute(
                    "INSERT INTO accounts (
                        id, tool_id, label, uid_masked, uid_hash, email_masked,
                        status, is_active_on_client, has_local_snapshot, has_api_token,
                        encrypted_credential, last_claimed_at, created_at, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 0, ?7, ?8, ?9, NULL, ?10, ?10)",
                    params![
                        id,
                        envelope.tool_id,
                        label,
                        uid_masked,
                        uid_hash,
                        email_masked,
                        has_snap as i64,
                        has_tok as i64,
                        encrypted_cred,
                        now
                    ],
                )?;
                id
            }
        };

        drop(conn);
        self.get_account_summary(&account_id)
    }

    pub fn bind_account_credential(
        &self,
        master_key: &[u8; 32],
        account_id: &str,
        label_override: Option<&str>,
        incoming: &CredentialEnvelope,
    ) -> Result<AccountSummary, StorageError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;

        let existing: Option<(String, String, Vec<u8>)> = conn
            .query_row(
                "SELECT tool_id, label, encrypted_credential FROM accounts WHERE id = ?1",
                params![account_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        let (existing_tool_id, existing_label, old_blob) =
            existing.ok_or_else(|| StorageError::NotFound(account_id.to_string()))?;

        if existing_tool_id != incoming.tool_id {
            return Err(StorageError::Crypto(format!(
                "凭据工具类型不匹配：期望 {existing_tool_id}，实际 {}",
                incoming.tool_id
            )));
        }

        let old_env = crypto::decrypt_credential(master_key, &old_blob)?;
        let merged = old_env.merge_from(incoming);
        let uid_hash = crypto::compute_uid_hash(&merged.tool_id, &merged.uid);

        let conflict: Option<String> = conn
            .query_row(
                "SELECT id FROM accounts WHERE tool_id = ?1 AND uid_hash = ?2 AND id != ?3",
                params![merged.tool_id, uid_hash, account_id],
                |row| row.get(0),
            )
            .optional()?;
        if conflict.is_some() {
            return Err(StorageError::Crypto(
                "该凭据对应的账号已绑定在列表中，请勿重复绑定".into(),
            ));
        }

        let uid_masked = crypto::mask_uid(&merged.uid);
        let email_masked = merged.display_email.as_deref().map(crypto::mask_email);
        let encrypted_cred = crypto::encrypt_credential(master_key, &merged)?;
        let (has_snap, has_tok) = merged.capability_flags();
        let effective_label = label_override
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&existing_label);

        conn.execute(
            "UPDATE accounts
             SET label = ?1, uid_masked = ?2, uid_hash = ?3, email_masked = ?4,
                 status = 'active', encrypted_credential = ?5,
                 has_local_snapshot = ?6, has_api_token = ?7, updated_at = ?8
             WHERE id = ?9",
            params![
                effective_label,
                uid_masked,
                uid_hash,
                email_masked,
                encrypted_cred,
                has_snap as i64,
                has_tok as i64,
                now,
                account_id
            ],
        )?;

        drop(conn);
        self.get_account_summary(account_id)
    }

    pub fn get_account_summary(&self, account_id: &str) -> Result<AccountSummary, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT id, tool_id, label, uid_masked, email_masked, status,
                    is_active_on_client, has_local_snapshot, has_api_token,
                    last_claimed_at, updated_at
             FROM accounts WHERE id = ?1",
        )?;

        let mut summary = stmt
            .query_row(params![account_id], map_account_row)
            .optional()?
            .ok_or_else(|| StorageError::NotFound(account_id.to_string()))?;

        summary.latest_quota = fetch_latest_quota(&conn, &summary.id)?;
        Ok(summary)
    }

    pub fn list_accounts(&self, tool_id: Option<&str>) -> Result<Vec<AccountSummary>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut results = Vec::new();

        if let Some(tid) = tool_id {
            let mut stmt = conn.prepare(
                "SELECT id, tool_id, label, uid_masked, email_masked, status,
                        is_active_on_client, has_local_snapshot, has_api_token,
                        last_claimed_at, updated_at
                 FROM accounts WHERE tool_id = ?1 ORDER BY updated_at DESC",
            )?;
            let rows = stmt.query_map(params![tid], map_account_row)?;
            for row in rows {
                let mut acc = row?;
                acc.latest_quota = fetch_latest_quota(&conn, &acc.id)?;
                results.push(acc);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, tool_id, label, uid_masked, email_masked, status,
                        is_active_on_client, has_local_snapshot, has_api_token,
                        last_claimed_at, updated_at
                 FROM accounts ORDER BY tool_id ASC, updated_at DESC",
            )?;
            let rows = stmt.query_map([], map_account_row)?;
            for row in rows {
                let mut acc = row?;
                acc.latest_quota = fetch_latest_quota(&conn, &acc.id)?;
                results.push(acc);
            }
        }

        Ok(results)
    }

    pub fn get_account_credential(
        &self,
        master_key: &[u8; 32],
        account_id: &str,
    ) -> Result<(AccountSummary, CredentialEnvelope), StorageError> {
        let summary = self.get_account_summary(account_id)?;
        let blob: Vec<u8> = {
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            conn.query_row(
                "SELECT encrypted_credential FROM accounts WHERE id = ?1",
                params![account_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(account_id.to_string()))?
        };

        let envelope = match crypto::decrypt_credential(master_key, &blob) {
            Ok(env) => env,
            Err(e) => {
                let _ = self.update_account_status(account_id, AccountStatus::Expired);
                return Err(StorageError::Crypto(format!(
                    "{} ({}): 凭据已失效或主密钥变更，请重新在线授权 ({e})",
                    summary.label, summary.uid_masked
                )));
            }
        };
        Ok((summary, envelope))
    }

    pub fn update_account_status(
        &self,
        account_id: &str,
        status: AccountStatus,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let now = Utc::now().to_rfc3339();
        let status_str = serialize_account_status(&status);
        let affected = conn.execute(
            "UPDATE accounts SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status_str, now, account_id],
        )?;
        if affected == 0 {
            return Err(StorageError::NotFound(account_id.to_string()));
        }
        Ok(())
    }

    pub fn mark_account_claimed(
        &self,
        account_id: &str,
        claimed_at: &str,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE accounts SET last_claimed_at = ?1, updated_at = ?2 WHERE id = ?3",
            params![claimed_at, now, account_id],
        )?;
        Ok(())
    }

    pub fn set_active_on_client(
        &self,
        tool_id: &str,
        active_account_id: &str,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE accounts SET is_active_on_client = 0 WHERE tool_id = ?1",
            params![tool_id],
        )?;
        let affected = tx.execute(
            "UPDATE accounts SET is_active_on_client = 1 WHERE id = ?1 AND tool_id = ?2",
            params![active_account_id, tool_id],
        )?;
        if affected == 0 {
            return Err(StorageError::NotFound(active_account_id.to_string()));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_account(&self, account_id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let affected = conn.execute("DELETE FROM accounts WHERE id = ?1", params![account_id])?;
        if affected == 0 {
            return Err(StorageError::NotFound(account_id.to_string()));
        }
        Ok(())
    }

    pub fn save_quota_snapshot(
        &self,
        account_id: &str,
        snapshot: &QuotaSnapshot,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let id = Uuid::new_v4().to_string();
        let claim_str = serialize_claim_status(&snapshot.claim_status);
        let buckets_json = serde_json::to_string(&snapshot.buckets)?;

        conn.execute(
            "INSERT INTO quota_snapshots (id, account_id, plan_name, claim_status, buckets_json, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                account_id,
                snapshot.plan_name,
                claim_str,
                buckets_json,
                snapshot.fetched_at
            ],
        )?;
        Ok(())
    }

    pub fn append_action_log(
        &self,
        account_id: Option<&str>,
        tool_id: &str,
        action_type: &str,
        result_status: &str,
        error_code: Option<&UnifiedErrorCode>,
        message_sanitized: &str,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let now = Utc::now().to_rfc3339();
        let err_str = error_code.map(serialize_error_code);

        conn.execute(
            "INSERT INTO action_logs (account_id, tool_id, action_type, result_status, error_code, message_sanitized, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                account_id,
                tool_id,
                action_type,
                result_status,
                err_str,
                message_sanitized,
                now
            ],
        )?;
        Ok(())
    }

    pub fn list_action_logs(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ActionLogItem>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT id, account_id, tool_id, action_type, result_status, error_code, message_sanitized, created_at
             FROM action_logs ORDER BY id DESC LIMIT ?1 OFFSET ?2",
        )?;

        let rows = stmt.query_map(params![limit, offset], |row| {
            let err_opt: Option<String> = row.get(5)?;
            Ok(ActionLogItem {
                id: row.get(0)?,
                account_id: row.get(1)?,
                tool_id: row.get(2)?,
                action_type: row.get(3)?,
                result_status: row.get(4)?,
                error_code: err_opt.as_deref().and_then(parse_error_code),
                message_sanitized: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?;

        let mut logs = Vec::new();
        for row in rows {
            logs.push(row?);
        }
        Ok(logs)
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let val = conn
            .query_row(
                "SELECT value_json FROM app_settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(val)
    }

    pub fn set_setting(&self, key: &str, value_json: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO app_settings (key, value_json, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
            params![key, value_json, now],
        )?;
        Ok(())
    }
}

fn map_account_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccountSummary> {
    let status_str: String = row.get(5)?;
    Ok(AccountSummary {
        id: row.get(0)?,
        tool_id: row.get(1)?,
        label: row.get(2)?,
        uid_masked: row.get(3)?,
        email_masked: row.get(4)?,
        status: parse_account_status(&status_str),
        is_active_on_client: row.get::<_, i64>(6)? != 0,
        has_local_snapshot: row.get::<_, i64>(7)? != 0,
        has_api_token: row.get::<_, i64>(8)? != 0,
        latest_quota: None,
        last_claimed_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn fetch_latest_quota(
    conn: &Connection,
    account_id: &str,
) -> Result<Option<QuotaSnapshot>, StorageError> {
    let row: Option<(Option<String>, String, String, String)> = conn
        .query_row(
            "SELECT plan_name, claim_status, buckets_json, fetched_at
             FROM quota_snapshots WHERE account_id = ?1 ORDER BY fetched_at DESC LIMIT 1",
            params![account_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;

    match row {
        Some((plan_name, claim_str, buckets_json, fetched_at)) => {
            let buckets: Vec<QuotaBucket> = serde_json::from_str(&buckets_json)?;
            Ok(Some(QuotaSnapshot {
                plan_name,
                claim_status: parse_claim_status(&claim_str),
                buckets,
                fetched_at,
            }))
        }
        None => Ok(None),
    }
}

fn parse_account_status(s: &str) -> AccountStatus {
    match s {
        "active" => AccountStatus::Active,
        "expired" => AccountStatus::Expired,
        "risk_paused" => AccountStatus::RiskPaused,
        "disabled" => AccountStatus::Disabled,
        _ => AccountStatus::Active,
    }
}

fn serialize_account_status(s: &AccountStatus) -> &'static str {
    match s {
        AccountStatus::Active => "active",
        AccountStatus::Expired => "expired",
        AccountStatus::RiskPaused => "risk_paused",
        AccountStatus::Disabled => "disabled",
    }
}

fn parse_claim_status(s: &str) -> ClaimStatus {
    match s {
        "available" => ClaimStatus::Available,
        "claimed" => ClaimStatus::Claimed,
        "unsupported" => ClaimStatus::Unsupported,
        _ => ClaimStatus::Unknown,
    }
}

fn serialize_claim_status(s: &ClaimStatus) -> &'static str {
    match s {
        ClaimStatus::Unknown => "unknown",
        ClaimStatus::Available => "available",
        ClaimStatus::Claimed => "claimed",
        ClaimStatus::Unsupported => "unsupported",
    }
}

fn serialize_error_code(c: &UnifiedErrorCode) -> &'static str {
    match c {
        UnifiedErrorCode::AuthExpired => "AUTH_EXPIRED",
        UnifiedErrorCode::RiskTriggered => "RISK_TRIGGERED",
        UnifiedErrorCode::RateLimited => "RATE_LIMITED",
        UnifiedErrorCode::NetworkError => "NETWORK_ERROR",
        UnifiedErrorCode::ClientRunning => "CLIENT_RUNNING",
        UnifiedErrorCode::ClientNotFound => "CLIENT_NOT_FOUND",
        UnifiedErrorCode::UnsupportedCapability => "UNSUPPORTED_CAPABILITY",
        UnifiedErrorCode::InvalidCredential => "INVALID_CREDENTIAL",
        UnifiedErrorCode::InternalError => "INTERNAL_ERROR",
    }
}

fn parse_error_code(s: &str) -> Option<UnifiedErrorCode> {
    match s {
        "AUTH_EXPIRED" => Some(UnifiedErrorCode::AuthExpired),
        "RISK_TRIGGERED" => Some(UnifiedErrorCode::RiskTriggered),
        "RATE_LIMITED" => Some(UnifiedErrorCode::RateLimited),
        "NETWORK_ERROR" => Some(UnifiedErrorCode::NetworkError),
        "CLIENT_RUNNING" => Some(UnifiedErrorCode::ClientRunning),
        "CLIENT_NOT_FOUND" => Some(UnifiedErrorCode::ClientNotFound),
        "UNSUPPORTED_CAPABILITY" => Some(UnifiedErrorCode::UnsupportedCapability),
        "INVALID_CREDENTIAL" => Some(UnifiedErrorCode::InvalidCredential),
        "INTERNAL_ERROR" => Some(UnifiedErrorCode::InternalError),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::CredentialPayload;
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    use std::collections::HashMap;

    fn temp_db_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("qwitch_test_{tag}_{}.db", Uuid::new_v4()))
    }

    #[test]
    fn database_upsert_merge_bind_switch_and_settings() {
        let db_path = temp_db_path("core");
        let db = QwitchDatabase::open(&db_path).expect("open db");
        let master_key = [42u8; 32];

        let mut files = HashMap::new();
        files.insert("auth.v1.dat".to_string(), BASE64.encode(b"v10encrypted"));

        let snap_env = CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid: "qoder_cn_abcdef123456".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::LocalSnapshot { files },
        };

        let created = db
            .upsert_account(&master_key, "Qoder 账号1", &snap_env)
            .expect("insert snap");
        assert!(created.has_local_snapshot);
        assert!(!created.has_api_token);

        let token_env = CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid: "qoder_cn_abcdef123456".to_string(),
            display_name: Some("Alice".to_string()),
            display_email: Some("alice@example.com".to_string()),
            payload: CredentialPayload::Token {
                access_token: "tok-merged-123".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };

        // 同 UID 再次 upsert 自动合并双轨凭据
        let upserted = db
            .upsert_account(&master_key, "Qoder 主力", &token_env)
            .expect("merge upsert");
        assert_eq!(upserted.id, created.id);
        assert!(upserted.has_local_snapshot);
        assert!(upserted.has_api_token);

        // 另一个账号使用 bind_account_credential 从快照升级到双轨
        let mut files2 = HashMap::new();
        files2.insert("auth.v1.dat".to_string(), BASE64.encode(b"v10other"));
        let snap2 = CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid: "qoder_cn_other999999".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::LocalSnapshot { files: files2 },
        };
        let acc2 = db.upsert_account(&master_key, "Qoder 副号", &snap2).expect("acc2");
        let bind_tok = CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid: "real_uid_9999".to_string(),
            display_name: Some("Bob".to_string()),
            display_email: Some("bob@example.com".to_string()),
            payload: CredentialPayload::Token {
                access_token: "tok-bob-456".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };
        let bound = db
            .bind_account_credential(&master_key, &acc2.id, None, &bind_tok)
            .expect("bind");
        assert!(bound.has_local_snapshot);
        assert!(bound.has_api_token);
        assert_eq!(bound.label, "Qoder 副号");

        // set_active_on_client 互斥切换
        db.set_active_on_client("qoder_cn", &created.id).expect("set active 1");
        db.set_active_on_client("qoder_cn", &acc2.id).expect("set active 2");
        let all = db.list_accounts(Some("qoder_cn")).expect("list");
        assert_eq!(all.iter().filter(|a| a.is_active_on_client).count(), 1);
        assert!(all.iter().find(|a| a.id == acc2.id).unwrap().is_active_on_client);

        // app_settings 读写
        db.set_setting("app.settings", r#"{"jitterMinMs":2000}"#)
            .expect("set setting");
        assert_eq!(
            db.get_setting("app.settings").expect("get setting").as_deref(),
            Some(r#"{"jitterMinMs":2000}"#)
        );

        let _ = std::fs::remove_file(&db_path);
    }
}
