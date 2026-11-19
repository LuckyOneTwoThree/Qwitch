pub mod crypto;

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;
use thiserror::Error;
use uuid::Uuid;

use crate::domain::{
    AccountStatus, AccountSummary, ActionLogCleanupRequest, ActionLogItem, ActionLogStats,
    ClaimStatus, CredentialEnvelope, LogScope, QuotaBucket, QuotaSnapshot, UnifiedErrorCode,
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

/// 默认展示名：优先官方昵称，其次脱敏邮箱/手机号/UID，绝不落明文 PII
pub fn default_account_label(envelope: &CredentialEnvelope) -> String {
    if let Some(name) = envelope
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return name.to_string();
    }
    if let Some(email) = envelope
        .display_email
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return crypto::mask_email(email);
    }
    if !envelope.uid.trim().is_empty() {
        return crypto::mask_uid(&envelope.uid);
    }
    format!("{} 账号", envelope.tool_id)
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
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA busy_timeout = 5000;

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
                created_at TEXT NOT NULL,
                archived_at TEXT
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
        let _ = conn.execute("ALTER TABLE action_logs ADD COLUMN archived_at TEXT", []);
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_logs_archived_id ON action_logs(archived_at, id DESC)",
            [],
        );
        Ok(())
    }

    pub fn upsert_account(
        &self,
        master_key: &[u8; 32],
        label_override: Option<&str>,
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
                     SET label = COALESCE(?1, label), uid_masked = ?2, email_masked = ?3,
                         status = CASE WHEN status IN ('disabled', 'risk_paused') THEN status ELSE 'active' END,
                         encrypted_credential = ?4, has_local_snapshot = ?5, has_api_token = ?6,
                         updated_at = ?7
                     WHERE id = ?8",
                    params![
                        label_override,
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
                let default_label = default_account_label(envelope);
                let label = label_override.unwrap_or(&default_label);
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

        // 双方均为真实 UID 且不一致 → 凭据属于另一个账号，拒绝串号绑定
        let old_uid_real =
            !old_env.uid.trim().is_empty() && !CredentialEnvelope::is_fallback_uid(&old_env.uid);
        let incoming_uid_real =
            !incoming.uid.trim().is_empty() && !CredentialEnvelope::is_fallback_uid(&incoming.uid);
        if old_uid_real && incoming_uid_real && old_env.uid != incoming.uid {
            return Err(StorageError::Crypto(format!(
                "凭据归属不匹配：目标账号 {}，实际授权账号 {}。已拒绝绑定，请在对应账号卡片上操作或导入为新账号。",
                crypto::mask_uid(&old_env.uid),
                crypto::mask_uid(&incoming.uid)
            )));
        }

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
                 status = CASE WHEN status IN ('disabled', 'risk_paused') THEN status ELSE 'active' END,
                 encrypted_credential = ?5,
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
            "SELECT a.id, a.tool_id, a.label, a.uid_masked, a.email_masked, a.status,
                    a.is_active_on_client, a.has_local_snapshot, a.has_api_token,
                    a.last_claimed_at, a.updated_at,
                    q.plan_name, q.claim_status, q.buckets_json, q.fetched_at
             FROM accounts a
             LEFT JOIN quota_snapshots q ON q.id = (
                 SELECT q2.id FROM quota_snapshots q2
                 WHERE q2.account_id = a.id
                 ORDER BY q2.fetched_at DESC, q2.rowid DESC
                 LIMIT 1
             )
             WHERE a.id = ?1",
        )?;

        stmt.query_row(params![account_id], map_account_row)
            .optional()?
            .ok_or_else(|| StorageError::NotFound(account_id.to_string()))
    }

    pub fn list_accounts(
        &self,
        tool_id: Option<&str>,
    ) -> Result<Vec<AccountSummary>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut results = Vec::new();

        if let Some(tid) = tool_id {
            let mut stmt = conn.prepare(
                "SELECT a.id, a.tool_id, a.label, a.uid_masked, a.email_masked, a.status,
                        a.is_active_on_client, a.has_local_snapshot, a.has_api_token,
                        a.last_claimed_at, a.updated_at,
                        q.plan_name, q.claim_status, q.buckets_json, q.fetched_at
                 FROM accounts a
                 LEFT JOIN quota_snapshots q ON q.id = (
                     SELECT q2.id FROM quota_snapshots q2
                     WHERE q2.account_id = a.id
                     ORDER BY q2.fetched_at DESC, q2.rowid DESC
                     LIMIT 1
                 )
                 WHERE a.tool_id = ?1
                 ORDER BY a.updated_at DESC",
            )?;
            let rows = stmt.query_map(params![tid], map_account_row)?;
            for row in rows {
                results.push(row?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT a.id, a.tool_id, a.label, a.uid_masked, a.email_masked, a.status,
                        a.is_active_on_client, a.has_local_snapshot, a.has_api_token,
                        a.last_claimed_at, a.updated_at,
                        q.plan_name, q.claim_status, q.buckets_json, q.fetched_at
                 FROM accounts a
                 LEFT JOIN quota_snapshots q ON q.id = (
                     SELECT q2.id FROM quota_snapshots q2
                     WHERE q2.account_id = a.id
                     ORDER BY q2.fetched_at DESC, q2.rowid DESC
                     LIMIT 1
                 )
                 ORDER BY a.tool_id ASC, a.updated_at DESC",
            )?;
            let rows = stmt.query_map([], map_account_row)?;
            for row in rows {
                results.push(row?);
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
                // 解密失败不等于凭据过期：可能是主密钥不匹配或数据损坏，
                // 静默标记 Expired 会掩盖密钥事故并误导用户重新授权
                return Err(StorageError::Crypto(format!(
                    "{} ({}): 凭据解密失败（主密钥不匹配或数据损坏），请检查密钥状态后重试 ({e})",
                    summary.label, summary.uid_masked
                )));
            }
        };
        Ok((summary, envelope))
    }

    /// 取一条账号密文样本，用于无指纹旧库的主密钥匹配验证
    pub fn first_credential_blob(&self) -> Option<Vec<u8>> {
        let conn = self.conn.lock().ok()?;
        conn.query_row(
            "SELECT encrypted_credential FROM accounts LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    /// 启动时校验主密钥指纹；首次生成前必须确认当前密钥能解密既有数据，
    /// 避免把错误密钥"祝福"为合法密钥
    pub fn verify_or_init_key_fingerprint(
        &self,
        data_dir: &Path,
        master_key: &[u8; 32],
    ) -> Result<(), StorageError> {
        let fingerprint = crypto::key_fingerprint(master_key);
        let path = data_dir.join("key_id");
        if let Ok(existing) = std::fs::read_to_string(&path) {
            if existing.trim() != fingerprint {
                return Err(StorageError::Crypto(
                    "主密钥与本地密钥指纹 (key_id) 不匹配：当前密钥无法解密既有数据。\
                     已拒绝启动，请恢复原钥匙串条目或密钥备份。"
                        .into(),
                ));
            }
            return Ok(());
        }

        let sample: Option<Vec<u8>> = {
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            conn.query_row(
                "SELECT encrypted_credential FROM accounts LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?
        };
        if let Some(blob) = sample {
            crypto::decrypt_credential(master_key, &blob).map_err(|_| {
                StorageError::Crypto(
                    "主密钥无法解密既有账号凭据且缺少密钥指纹记录。\
                     已拒绝启动，请恢复原钥匙串条目或密钥备份。"
                        .into(),
                )
            })?;
        }

        std::fs::write(&path, fingerprint.as_bytes())
            .map_err(|e| StorageError::Crypto(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
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

    pub fn update_account_label(
        &self,
        account_id: &str,
        label: &str,
    ) -> Result<AccountSummary, StorageError> {
        let trimmed = label.trim();
        if trimmed.is_empty() {
            return Err(StorageError::Crypto("账号备注不能为空".into()));
        }
        {
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            let now = Utc::now().to_rfc3339();
            let affected = conn.execute(
                "UPDATE accounts SET label = ?1, updated_at = ?2 WHERE id = ?3",
                params![trimmed, now, account_id],
            )?;
            if affected == 0 {
                return Err(StorageError::NotFound(account_id.to_string()));
            }
        }
        self.get_account_summary(account_id)
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
        // 单账号最多保留最近 20 条额度快照，防止长期运行导致数据库膨胀
        let _ = conn.execute(
            "DELETE FROM quota_snapshots
             WHERE account_id = ?1
               AND id NOT IN (
                   SELECT id FROM quota_snapshots
                   WHERE account_id = ?1
                   ORDER BY fetched_at DESC, rowid DESC
                   LIMIT 20
               )",
            params![account_id],
        );
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
        let message_sanitized = crypto::sanitize_log_message(message_sanitized);

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
        scope: LogScope,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ActionLogItem>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let scope_clause = match scope {
            LogScope::Active => "WHERE archived_at IS NULL",
            LogScope::Archived => "WHERE archived_at IS NOT NULL",
            LogScope::All => "",
        };
        let sql = format!(
            "SELECT id, account_id, tool_id, action_type, result_status, error_code, message_sanitized, created_at, archived_at
             FROM action_logs {scope_clause} ORDER BY id DESC LIMIT ?1 OFFSET ?2"
        );
        let mut stmt = conn.prepare(&sql)?;

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
                archived_at: row.get(8)?,
            })
        })?;

        let mut logs = Vec::new();
        for row in rows {
            logs.push(row?);
        }
        Ok(logs)
    }

    pub fn get_action_log_stats(&self) -> Result<ActionLogStats, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let stats = conn.query_row(
            "SELECT
                COALESCE(SUM(CASE WHEN archived_at IS NULL THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN archived_at IS NOT NULL THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN archived_at IS NULL AND result_status = 'failed' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN archived_at IS NULL AND (result_status = 'skipped' OR (action_type = 'query' AND result_status = 'success')) THEN 1 ELSE 0 END), 0)
             FROM action_logs",
            [],
            |row| {
                Ok(ActionLogStats {
                    active_count: row.get::<_, i64>(0)?.max(0) as u64,
                    archived_count: row.get::<_, i64>(1)?.max(0) as u64,
                    failed_active_count: row.get::<_, i64>(2)?.max(0) as u64,
                    noise_active_count: row.get::<_, i64>(3)?.max(0) as u64,
                })
            },
        )?;
        Ok(stats)
    }

    /// 按筛选条件批量归档活跃日志（保留审计记录但移出默认主视图）
    pub fn archive_action_logs(
        &self,
        req: &ActionLogCleanupRequest,
    ) -> Result<usize, StorageError> {
        let mut normalized = req.clone();
        normalized.scope = Some(req.scope.unwrap_or(LogScope::Active));
        let now = Utc::now().to_rfc3339();
        self.execute_log_mutation(
            "UPDATE action_logs SET archived_at = ?",
            Some(Value::Text(now)),
            &normalized,
        )
    }

    /// 将已归档日志恢复至活跃视图
    pub fn unarchive_action_logs(
        &self,
        req: &ActionLogCleanupRequest,
    ) -> Result<usize, StorageError> {
        let mut normalized = req.clone();
        normalized.scope = Some(req.scope.unwrap_or(LogScope::Archived));
        self.execute_log_mutation(
            "UPDATE action_logs SET archived_at = NULL",
            None,
            &normalized,
        )
    }

    /// 按筛选条件永久删除操作日志（未指定 scope 时默认仅活跃日志，防止空请求清空全部）
    pub fn delete_action_logs(&self, req: &ActionLogCleanupRequest) -> Result<usize, StorageError> {
        let mut normalized = req.clone();
        normalized.scope = Some(req.scope.unwrap_or(LogScope::Active));
        self.execute_log_mutation("DELETE FROM action_logs", None, &normalized)
    }

    /// 按保留策略自动将超期或超出上限的活跃日志转入归档
    pub fn apply_log_retention(
        &self,
        max_age_days: u32,
        max_active_count: u32,
    ) -> Result<usize, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let mut affected = 0usize;

        if max_age_days > 0 {
            let cutoff = (now - Duration::days(max_age_days as i64)).to_rfc3339();
            affected += conn.execute(
                "UPDATE action_logs SET archived_at = ?1 WHERE archived_at IS NULL AND created_at < ?2",
                params![now_str, cutoff],
            )?;
        }

        if max_active_count > 0 {
            affected += conn.execute(
                "UPDATE action_logs
                 SET archived_at = ?1
                 WHERE archived_at IS NULL
                   AND id NOT IN (
                     SELECT id FROM action_logs WHERE archived_at IS NULL ORDER BY id DESC LIMIT ?2
                   )",
                params![now_str, max_active_count as i64],
            )?;
        }

        Ok(affected)
    }

    fn execute_log_mutation(
        &self,
        sql_prefix: &str,
        leading_param: Option<Value>,
        req: &ActionLogCleanupRequest,
    ) -> Result<usize, StorageError> {
        if let Some(ref ids) = req.ids {
            if ids.is_empty() {
                return Ok(0);
            }
        }

        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut params: Vec<Value> = Vec::new();
        if let Some(p) = leading_param {
            params.push(p);
        }

        let mut clauses: Vec<String> = Vec::new();
        match req.scope.unwrap_or(LogScope::All) {
            LogScope::Active => clauses.push("archived_at IS NULL".to_string()),
            LogScope::Archived => clauses.push("archived_at IS NOT NULL".to_string()),
            LogScope::All => {}
        }

        if let Some(days) = req.older_than_days {
            if days > 0 {
                let cutoff = (Utc::now() - Duration::days(days as i64)).to_rfc3339();
                clauses.push("created_at < ?".to_string());
                params.push(Value::Text(cutoff));
            }
        }

        if let Some(ref action_type) = req.action_type {
            let trimmed = action_type.trim();
            if !trimmed.is_empty() && trimmed != "all" {
                clauses.push("action_type = ?".to_string());
                params.push(Value::Text(trimmed.to_string()));
            }
        }

        if let Some(ref result_status) = req.result_status {
            let trimmed = result_status.trim();
            if !trimmed.is_empty() && trimmed != "all" {
                clauses.push("result_status = ?".to_string());
                params.push(Value::Text(trimmed.to_string()));
            }
        }

        if req.redundant_only {
            clauses.push(
                "(result_status = 'skipped' OR (action_type = 'query' AND result_status = 'success'))"
                    .to_string(),
            );
        }

        if let Some(ref ids) = req.ids {
            let mut total = 0usize;
            for chunk in ids.chunks(400) {
                let mut chunk_clauses = clauses.clone();
                let mut chunk_params = params.clone();
                let placeholders = vec!["?"; chunk.len()].join(", ");
                chunk_clauses.push(format!("id IN ({placeholders})"));
                for id in chunk {
                    chunk_params.push(Value::Integer(*id));
                }
                let sql = format!("{sql_prefix} WHERE {}", chunk_clauses.join(" AND "));
                total += conn.execute(&sql, params_from_iter(chunk_params))?;
            }
            return Ok(total);
        }

        let sql = if clauses.is_empty() {
            sql_prefix.to_string()
        } else {
            format!("{sql_prefix} WHERE {}", clauses.join(" AND "))
        };
        let count = conn.execute(&sql, params_from_iter(params))?;
        Ok(count)
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
    let last_claimed_at: Option<String> = row.get(9)?;
    let updated_at: String = row.get(10)?;
    let plan_name: Option<String> = row.get(11)?;
    let claim_str_opt: Option<String> = row.get(12)?;
    let buckets_json_opt: Option<String> = row.get(13)?;
    let fetched_at_opt: Option<String> = row.get(14)?;

    let latest_quota = match (claim_str_opt, buckets_json_opt, fetched_at_opt) {
        (Some(claim_str), Some(buckets_json), Some(fetched_at)) => {
            let buckets: Vec<QuotaBucket> = serde_json::from_str(&buckets_json).unwrap_or_default();
            let raw_claim_status = parse_claim_status(&claim_str);
            let row_tool_id: String = row.get(1)?;
            let normalized_claim_status = normalize_daily_claim_status(
                raw_claim_status,
                &fetched_at,
                last_claimed_at.as_deref(),
                &row_tool_id,
            );
            Some(QuotaSnapshot {
                plan_name,
                claim_status: normalized_claim_status,
                buckets,
                fetched_at,
            })
        }
        _ => None,
    };

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
        latest_quota,
        last_claimed_at,
        updated_at,
    })
}

fn cst_offset() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("UTC+8 fixed offset must be valid")
}

pub fn is_same_cst_day(iso_str: &str, now_cst_date: NaiveDate) -> bool {
    DateTime::parse_from_rfc3339(iso_str)
        .map(|dt| dt.with_timezone(&cst_offset()).date_naive() == now_cst_date)
        .unwrap_or(false)
}

pub fn normalize_daily_claim_status(
    raw_status: ClaimStatus,
    fetched_at: &str,
    last_claimed_at: Option<&str>,
    tool_id: &str,
) -> ClaimStatus {
    let today_cst = Utc::now().with_timezone(&cst_offset()).date_naive();
    normalize_daily_claim_status_on_date(
        raw_status,
        fetched_at,
        last_claimed_at,
        today_cst,
        tool_id,
    )
}

fn normalize_daily_claim_status_on_date(
    raw_status: ClaimStatus,
    fetched_at: &str,
    last_claimed_at: Option<&str>,
    today_cst: NaiveDate,
    tool_id: &str,
) -> ClaimStatus {
    // 每日重置型工具（Trae/WorkBuddy）以 last_claimed_at 为权威；
    // Qoder 活动按周期而非自然日，不适用"今日已领即 Claimed"覆盖
    let daily_reset_tool = matches!(tool_id, "trae" | "trae_cn" | "workbuddy");
    let claimed_today = last_claimed_at
        .map(|ts| is_same_cst_day(ts, today_cst))
        .unwrap_or(false);
    match raw_status {
        ClaimStatus::Unsupported | ClaimStatus::Unknown => raw_status,
        ClaimStatus::Claimed => {
            if is_same_cst_day(fetched_at, today_cst) || claimed_today {
                ClaimStatus::Claimed
            } else {
                ClaimStatus::Unknown
            }
        }
        ClaimStatus::Available => {
            if daily_reset_tool && claimed_today {
                ClaimStatus::Claimed
            } else if is_same_cst_day(fetched_at, today_cst) {
                ClaimStatus::Available
            } else {
                ClaimStatus::Unknown
            }
        }
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
            .upsert_account(&master_key, Some("Qoder 账号1"), &snap_env)
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
            .upsert_account(&master_key, Some("Qoder 主力"), &token_env)
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
        let acc2 = db
            .upsert_account(&master_key, Some("Qoder 副号"), &snap2)
            .expect("acc2");
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
        db.set_active_on_client("qoder_cn", &created.id)
            .expect("set active 1");
        db.set_active_on_client("qoder_cn", &acc2.id)
            .expect("set active 2");
        let all = db.list_accounts(Some("qoder_cn")).expect("list");
        assert_eq!(all.iter().filter(|a| a.is_active_on_client).count(), 1);
        assert!(
            all.iter()
                .find(|a| a.id == acc2.id)
                .unwrap()
                .is_active_on_client
        );

        // app_settings 读写
        db.set_setting("app.settings", r#"{"jitterMinMs":2000}"#)
            .expect("set setting");
        assert_eq!(
            db.get_setting("app.settings")
                .expect("get setting")
                .as_deref(),
            Some(r#"{"jitterMinMs":2000}"#)
        );

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn action_logs_archive_retention_restore_and_delete() {
        let db_path = temp_db_path("logs");
        let db = QwitchDatabase::open(&db_path).expect("open db");

        db.append_action_log(None, "trae_cn", "query", "success", None, "查额成功 1")
            .unwrap();
        db.append_action_log(None, "trae_cn", "claim", "skipped", None, "今日已签")
            .unwrap();
        db.append_action_log(None, "workbuddy", "claim", "success", None, "签到 +10")
            .unwrap();
        db.append_action_log(
            None,
            "qoder_cn",
            "query",
            "failed",
            Some(&UnifiedErrorCode::AuthExpired),
            "会话过期",
        )
        .unwrap();

        let stats = db.get_action_log_stats().unwrap();
        assert_eq!(stats.active_count, 4);
        assert_eq!(stats.archived_count, 0);
        assert_eq!(stats.failed_active_count, 1);
        assert_eq!(stats.noise_active_count, 2);

        // 仅归档冗余日常日志（query/success + skipped），保留签到成功与异常失败
        let archived_redundant = db
            .archive_action_logs(&ActionLogCleanupRequest {
                redundant_only: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(archived_redundant, 2);

        let active = db.list_action_logs(LogScope::Active, 50, 0).unwrap();
        assert_eq!(active.len(), 2);
        let archived = db.list_action_logs(LogScope::Archived, 50, 0).unwrap();
        assert_eq!(archived.len(), 2);
        assert!(archived.iter().all(|item| item.archived_at.is_some()));

        // 按最大活跃条数策略保留最新 1 条，将较旧的 1 条自动归档
        let retained = db.apply_log_retention(0, 1).unwrap();
        assert_eq!(retained, 1);
        assert_eq!(
            db.list_action_logs(LogScope::Active, 50, 0).unwrap().len(),
            1
        );
        assert_eq!(
            db.list_action_logs(LogScope::Archived, 50, 0)
                .unwrap()
                .len(),
            3
        );

        // 从归档恢复单条并彻底清空剩余已归档日志
        let restore_id = archived[0].id;
        let restored = db
            .unarchive_action_logs(&ActionLogCleanupRequest {
                ids: Some(vec![restore_id]),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(restored, 1);
        assert_eq!(
            db.list_action_logs(LogScope::Active, 50, 0).unwrap().len(),
            2
        );

        let deleted = db
            .delete_action_logs(&ActionLogCleanupRequest {
                scope: Some(LogScope::Archived),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(deleted, 2);
        assert_eq!(
            db.list_action_logs(LogScope::Archived, 50, 0)
                .unwrap()
                .len(),
            0
        );

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn quota_snapshot_join_retention_and_rename_account() {
        let db_path = temp_db_path("snapshots");
        let db = QwitchDatabase::open(&db_path).expect("open db");
        let master_key = [9u8; 32];

        let env = CredentialEnvelope {
            tool_id: "trae_cn".to_string(),
            uid: "u_snap_123456".to_string(),
            display_name: Some("SnapUser".to_string()),
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "tok-1".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };
        let acc = db
            .upsert_account(&master_key, Some("初始名称"), &env)
            .unwrap();

        let renamed = db.update_account_label(&acc.id, "新备注名").unwrap();
        assert_eq!(renamed.label, "新备注名");

        for i in 0..25 {
            let snap = QuotaSnapshot {
                plan_name: Some(format!("Plan-{i}")),
                claim_status: ClaimStatus::Available,
                buckets: vec![QuotaBucket {
                    key: "k".into(),
                    label: "额度".into(),
                    used: i as f64,
                    total: 100.0,
                    remaining: (100 - i) as f64,
                    unit: "Credits".into(),
                    reset_at: None,
                }],
                fetched_at: format!("2026-11-18T12:{i:02}:00Z"),
            };
            db.save_quota_snapshot(&acc.id, &snap).unwrap();
        }

        let listed = db.list_accounts(Some("trae_cn")).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0]
                .latest_quota
                .as_ref()
                .and_then(|q| q.plan_name.as_deref()),
            Some("Plan-24")
        );

        let conn = db.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM quota_snapshots WHERE account_id = ?1",
                params![acc.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 20);
        drop(conn);

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn cross_day_claim_status_resets_stale_snapshot_and_keeps_same_cst_day() {
        let db_path = temp_db_path("cross_day_claim");
        let db = QwitchDatabase::open(&db_path).expect("open db");
        let master_key = [7u8; 32];

        let env = CredentialEnvelope {
            tool_id: "trae_cn".to_string(),
            uid: "u_cross_day_1".to_string(),
            display_name: Some("CrossDayUser".to_string()),
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "tok-cross".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };
        let acc = db
            .upsert_account(&master_key, Some("跨天账号"), &env)
            .unwrap();

        // 保存一条昨天 UTC+8 的已签到快照，经过数据库读取后今日应归零为 Unknown
        let yesterday_snap = QuotaSnapshot {
            plan_name: Some("Pro".into()),
            claim_status: ClaimStatus::Claimed,
            buckets: vec![],
            fetched_at: "2020-01-01T10:00:00+08:00".into(),
        };
        db.save_quota_snapshot(&acc.id, &yesterday_snap).unwrap();
        let loaded_yesterday = db.get_account_summary(&acc.id).unwrap();
        assert_eq!(
            loaded_yesterday
                .latest_quota
                .as_ref()
                .map(|q| &q.claim_status),
            Some(&ClaimStatus::Unknown)
        );

        // 保存一条今日 UTC+8 的已签到快照，应保持 Claimed
        let today_now_rfc3339 = Utc::now().to_rfc3339();
        let today_snap = QuotaSnapshot {
            plan_name: Some("Pro".into()),
            claim_status: ClaimStatus::Claimed,
            buckets: vec![],
            fetched_at: today_now_rfc3339.clone(),
        };
        db.save_quota_snapshot(&acc.id, &today_snap).unwrap();
        let loaded_today = db.get_account_summary(&acc.id).unwrap();
        assert_eq!(
            loaded_today.latest_quota.as_ref().map(|q| &q.claim_status),
            Some(&ClaimStatus::Claimed)
        );

        // 直接测试跨天 boundary（UTC 16:30 即 UTC+8 次日 00:30 vs 同日 15:30 即 UTC+8 当日 23:30）
        let today_cst = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        assert!(is_same_cst_day("2026-11-17T16:30:00Z", today_cst));
        assert!(!is_same_cst_day("2026-11-17T15:30:00Z", today_cst));
        assert_eq!(
            normalize_daily_claim_status_on_date(
                ClaimStatus::Claimed,
                "2026-11-17T15:30:00Z",
                Some("2026-11-17T15:00:00Z"),
                today_cst,
                "trae_cn"
            ),
            ClaimStatus::Unknown
        );
        assert_eq!(
            normalize_daily_claim_status_on_date(
                ClaimStatus::Claimed,
                "2026-11-17T16:30:00Z",
                None,
                today_cst,
                "trae_cn"
            ),
            ClaimStatus::Claimed
        );
        // 每日重置型工具：今日已领 + 服务端缓存仍返回 Available → 权威口径 Claimed
        assert_eq!(
            normalize_daily_claim_status_on_date(
                ClaimStatus::Available,
                "2026-11-18T02:00:00Z",
                Some("2026-11-18T01:00:00Z"),
                today_cst,
                "workbuddy"
            ),
            ClaimStatus::Claimed
        );
        // Qoder 活动按周期：今日已领不覆盖新出现的 Available 活动
        assert_eq!(
            normalize_daily_claim_status_on_date(
                ClaimStatus::Available,
                "2026-11-18T02:00:00Z",
                Some("2026-11-18T01:00:00Z"),
                today_cst,
                "qoder_cn"
            ),
            ClaimStatus::Available
        );

        let _ = std::fs::remove_file(&db_path);
    }

    fn temp_data_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("qwitch_fp_test_{tag}_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn token_envelope(uid: &str) -> CredentialEnvelope {
        CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid: uid.to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "tok-fingerprint-test".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        }
    }

    #[test]
    fn key_fingerprint_guards_wrong_key_and_never_blesses_it() {
        let dir = temp_data_dir("guard");
        let db_path = dir.join("qwitch.db");
        let db = QwitchDatabase::open(&db_path).expect("open");
        let key_a = [1u8; 32];
        let key_b = [2u8; 32];

        // 空库：首次写入指纹，重复校验通过
        db.verify_or_init_key_fingerprint(&dir, &key_a)
            .expect("init empty");
        assert!(dir.join("key_id").exists());
        db.verify_or_init_key_fingerprint(&dir, &key_a)
            .expect("same key");

        // 指纹不匹配 → 拒绝
        assert!(db.verify_or_init_key_fingerprint(&dir, &key_b).is_err());

        // 解密失败不再静默标记 Expired
        let acc = db
            .upsert_account(&key_a, Some("指纹测试"), &token_envelope("fp_user_1"))
            .expect("insert");
        let err = db
            .get_account_credential(&key_b, &acc.id)
            .expect_err("wrong key must fail");
        assert!(matches!(err, StorageError::Crypto(_)));
        let summary = db.get_account_summary(&acc.id).expect("summary");
        assert_eq!(
            summary.status,
            AccountStatus::Active,
            "解密失败不得改写账号状态"
        );

        let _ = std::fs::remove_dir_all(&dir);

        // 已有账号但无指纹记录：错误密钥不得被"祝福"写入指纹
        let dir = temp_data_dir("nobless");
        let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open");
        db.upsert_account(&key_a, Some("无指纹"), &token_envelope("fp_user_2"))
            .expect("insert");
        assert!(db.verify_or_init_key_fingerprint(&dir, &key_b).is_err());
        assert!(!dir.join("key_id").exists(), "错误密钥不得写入指纹");
        db.verify_or_init_key_fingerprint(&dir, &key_a)
            .expect("correct key blesses");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reimport_preserves_custom_label_and_paused_status_and_rejects_uid_mismatch() {
        let db_path = temp_db_path("protect");
        let db = QwitchDatabase::open(&db_path).expect("open");
        let key = [21u8; 32];

        let env_a = token_envelope("real_uid_aaa111");
        let acc = db
            .upsert_account(&key, Some("主力号"), &env_a)
            .expect("insert");
        assert_eq!(acc.label, "主力号");

        // 显式停用后重新导入（无显式备注）：备注与停用状态均不被覆盖
        db.update_account_status(&acc.id, AccountStatus::Disabled)
            .expect("disable");
        let mut env_a2 = token_envelope("real_uid_aaa111");
        env_a2.display_name = Some("官方昵称".to_string());
        let again = db.upsert_account(&key, None, &env_a2).expect("reimport");
        assert_eq!(again.id, acc.id);
        assert_eq!(again.label, "主力号", "默认展示名不得覆盖用户自定义备注");
        assert_eq!(
            again.status,
            AccountStatus::Disabled,
            "重新导入不得复活已停用账号"
        );

        // 显式备注仍可覆盖
        let renamed = db
            .upsert_account(&key, Some("新备注"), &env_a2)
            .expect("rename via import");
        assert_eq!(renamed.label, "新备注");

        // risk_paused 同样保护
        db.update_account_status(&acc.id, AccountStatus::RiskPaused)
            .expect("pause");
        let paused = db
            .upsert_account(&key, None, &env_a2)
            .expect("reimport paused");
        assert_eq!(paused.status, AccountStatus::RiskPaused);

        // A 卡绑定 B 凭据：双方真实 UID 不一致必须拒绝
        let env_b = token_envelope("real_uid_bbb222");
        let err = db
            .bind_account_credential(&key, &acc.id, None, &env_b)
            .expect_err("must reject");
        assert!(err.to_string().contains("凭据归属不匹配"));
        let summary = db.get_account_summary(&acc.id).expect("summary");
        assert_eq!(
            summary.uid_masked,
            crypto::mask_uid("real_uid_aaa111"),
            "拒绝后原账号身份不变"
        );

        // fallback UID 目标允许被真实 UID 升级
        let fallback_env = token_envelope("wb_manual_temp123");
        let fb = db
            .upsert_account(&key, None, &fallback_env)
            .expect("fallback insert");
        let bound = db
            .bind_account_credential(&key, &fb.id, None, &env_b)
            .expect("fallback bind ok");
        assert_eq!(bound.uid_masked, crypto::mask_uid("real_uid_bbb222"));

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn default_label_never_contains_plain_pii() {
        let mut env = token_envelope("user_plain_uid_99");
        env.display_name = None;
        env.display_email = Some("alice@example.com".to_string());
        assert_eq!(default_account_label(&env), "al***@example.com");

        env.display_email = Some("13812345678".to_string());
        assert_eq!(default_account_label(&env), "138****5678");

        env.display_email = None;
        assert_eq!(
            default_account_label(&env),
            crypto::mask_uid("user_plain_uid_99")
        );
    }

    #[test]
    fn delete_logs_empty_request_defaults_to_active_scope() {
        let db_path = temp_db_path("delscope");
        let db = QwitchDatabase::open(&db_path).expect("open");
        db.append_action_log(None, "trae_cn", "import", "success", None, "活跃日志")
            .unwrap();
        db.append_action_log(None, "trae_cn", "claim", "success", None, "待归档日志")
            .unwrap();
        db.archive_action_logs(&ActionLogCleanupRequest {
            action_type: Some("claim".to_string()),
            ..Default::default()
        })
        .expect("archive");

        let deleted = db
            .delete_action_logs(&ActionLogCleanupRequest::default())
            .expect("delete");
        assert_eq!(deleted, 1, "空请求默认只删除活跃日志，不得清空归档");
        let archived = db
            .list_action_logs(LogScope::Archived, 100, 0)
            .expect("list");
        assert_eq!(archived.len(), 1);

        let _ = std::fs::remove_file(&db_path);
    }
}
