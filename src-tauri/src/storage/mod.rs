pub mod crypto;
mod logs;
pub mod model_discovery;
pub mod pools;
mod settings;

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use rusqlite::{params, types::Value, Connection, OptionalExtension};
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
    #[error("{0}")]
    Validation(String),
    #[error("Internal lock poisoned")]
    LockPoisoned,
}

/// 幂等添加列：仅吞掉「列已存在」错误，其余真实错误向上传播。
fn exec_add_column(conn: &rusqlite::Connection, sql: &str) -> Result<(), StorageError> {
    match conn.execute(sql, []) {
        Ok(_) => Ok(()),
        Err(e) => {
            if e.to_string().contains("duplicate column name") {
                Ok(())
            } else {
                Err(StorageError::Sqlite(e))
            }
        }
    }
}

pub struct QwitchDatabase {
    conn: Mutex<Connection>,
    db_path: std::path::PathBuf,
}

/// 默认展示名：优先官方昵称，其次脱敏邮箱/手机号/UID，绝不落明文 PII
pub fn default_account_label(envelope: &CredentialEnvelope) -> String {
    if let Some(name) = envelope
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !name.contains(' ')
            && name
                .split_once('@')
                .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
        {
            return crypto::mask_email(name);
        }
        if name.len() == 11 && name.starts_with('1') && name.chars().all(|c| c.is_ascii_digit()) {
            return crypto::mask_phone(name);
        }
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
            db_path: db_path.to_path_buf(),
        };
        db.init_schema()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for suffix in ["", "-wal", "-shm"] {
                let p = std::path::PathBuf::from(format!("{}{suffix}", db_path.display()));
                if p.exists() {
                    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
                }
            }
        }
        Ok(db)
    }

    /// 执行一次 PASSIVE checkpoint（不阻塞读者，不强制截断）。
    ///
    /// 由调用方按周期或事件驱动触发；失败不影响业务（返回 bool 供日志）。
    pub fn checkpoint_wal_passive(&self) -> bool {
        let Ok(conn) = self.conn.lock() else {
            return false;
        };
        conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")
            .is_ok()
    }

    /// 退出前 TRUNCATE checkpoint：把 WAL 内容全部合并回主库并清空 -wal，
    /// 让下次启动直接读主库，也避免残留的 -wal 长期占用磁盘。
    pub fn checkpoint_wal_truncate(&self) -> bool {
        let Ok(conn) = self.conn.lock() else {
            return false;
        };
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .is_ok()
    }

    fn init_schema(&self) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA busy_timeout = 5000;
            -- WAL 增长治理：限制单次 WAL 文件大小，配合周期性 PASSIVE checkpoint，
            -- 避免长时间运行后 -wal 无限增长（实测曾达主库体积的 20 倍）。
            PRAGMA journal_size_limit = 4194304;
            -- 每 1000 页（约 4MB，默认页大小 4096）触发一次自动 checkpoint
            PRAGMA wal_autocheckpoint = 1000;
            -- 删除的记录所在页用 0 覆写，避免已删除的凭据/日志残留在空闲页
            PRAGMA secure_delete = ON;

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
                device_id_masked TEXT,
                device_type TEXT,
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
        exec_add_column(
            &conn,
            "ALTER TABLE accounts ADD COLUMN has_local_snapshot INTEGER NOT NULL DEFAULT 1",
        )?;
        exec_add_column(
            &conn,
            "ALTER TABLE accounts ADD COLUMN has_api_token INTEGER NOT NULL DEFAULT 1",
        )?;
        exec_add_column(
            &conn,
            "ALTER TABLE accounts ADD COLUMN device_id_masked TEXT",
        )?;
        exec_add_column(&conn, "ALTER TABLE accounts ADD COLUMN device_type TEXT")?;
        exec_add_column(&conn, "ALTER TABLE action_logs ADD COLUMN archived_at TEXT")?;
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_logs_archived_id ON action_logs(archived_at, id DESC)",
            [],
        );
        // 支撑日志统计的 COUNT(*) 子查询（archived_at + result_status 覆盖扫描）
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_logs_archived_status ON action_logs(archived_at, result_status)",
            [],
        );

        // v0.6 号池网关与 Agent 指向表（增量扩展，不影响已有 4 张核心表）
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS account_pools (
                id TEXT PRIMARY KEY,
                slug TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                scene_tag TEXT NOT NULL DEFAULT '自定义',
                strategy TEXT NOT NULL DEFAULT 'quota_first',
                affinity_mode TEXT NOT NULL DEFAULT 'auto_cache',
                skip_active_client_account INTEGER NOT NULL DEFAULT 1,
                cooldown_seconds INTEGER NOT NULL DEFAULT 60,
                auto_refresh_on_401 INTEGER NOT NULL DEFAULT 1,
                fallback_api_key_enabled INTEGER NOT NULL DEFAULT 1,
                enabled INTEGER NOT NULL DEFAULT 1,
                today_requests INTEGER NOT NULL DEFAULT 0,
                today_failovers INTEGER NOT NULL DEFAULT 0,
                cache_hit_rate INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS pool_model_policies (
                pool_id TEXT NOT NULL,
                model_id TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                fallback_model_id TEXT,
                claude_alias_id TEXT,
                PRIMARY KEY (pool_id, model_id),
                FOREIGN KEY (pool_id) REFERENCES account_pools(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS pool_members (
                id TEXT PRIMARY KEY,
                pool_id TEXT NOT NULL,
                account_id TEXT NOT NULL,
                supported_models_json TEXT NOT NULL,
                tier TEXT NOT NULL DEFAULT 't1_primary',
                priority INTEGER NOT NULL DEFAULT 0,
                enabled INTEGER NOT NULL DEFAULT 1,
                last_latency_ms INTEGER,
                FOREIGN KEY (pool_id) REFERENCES account_pools(id) ON DELETE CASCADE,
                FOREIGN KEY (account_id) REFERENCES accounts(id) ON DELETE CASCADE,
                UNIQUE(pool_id, account_id)
            );

            CREATE TABLE IF NOT EXISTS agent_bindings (
                agent_id TEXT PRIMARY KEY,
                mounted_pool_ids_json TEXT NOT NULL DEFAULT '[]',
                default_model_key TEXT,
                binding_token_hash TEXT NOT NULL DEFAULT '',
                options_json TEXT NOT NULL DEFAULT '{}',
                status TEXT NOT NULL DEFAULT 'unbound',
                last_hit_at TEXT,
                last_checked_at TEXT,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS agent_config_stashes (
                agent_id TEXT NOT NULL,
                file_path TEXT NOT NULL,
                original_existed INTEGER NOT NULL,
                original_content_encrypted BLOB,
                captured_at TEXT NOT NULL,
                PRIMARY KEY (agent_id, file_path)
            );

            CREATE TABLE IF NOT EXISTS gateway_request_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trace_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                pool_id TEXT NOT NULL,
                pool_name TEXT NOT NULL,
                incoming_protocol TEXT NOT NULL,
                upstream_protocol TEXT NOT NULL,
                requested_model TEXT NOT NULL,
                final_account_label TEXT NOT NULL,
                final_tool_id TEXT NOT NULL,
                final_model TEXT NOT NULL,
                status TEXT NOT NULL,
                cache_affinity_hit INTEGER NOT NULL DEFAULT 0,
                ttft_ms INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                attempts_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS account_discovered_models (
                account_id   TEXT PRIMARY KEY,
                tool_id      TEXT NOT NULL,
                models_json  TEXT NOT NULL,
                updated_at   TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_pool_members_pool ON pool_members(pool_id, priority ASC);
            CREATE INDEX IF NOT EXISTS idx_pool_members_account ON pool_members(account_id);
            CREATE INDEX IF NOT EXISTS idx_gateway_logs_id ON gateway_request_logs(id DESC);
            ",
        )?;
        exec_add_column(
            &conn,
            "ALTER TABLE account_pools ADD COLUMN stats_date TEXT",
        )?;
        Ok(())
    }

    pub fn upsert_account(
        &self,
        master_key: &[u8; 32],
        label_override: Option<&str>,
        envelope: &CredentialEnvelope,
    ) -> Result<AccountSummary, StorageError> {
        let now = Utc::now().to_rfc3339();
        let uid_hash = crypto::compute_uid_hash_hmac(master_key, &envelope.tool_id, &envelope.uid);

        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;

        let existing: Option<(String, String, Vec<u8>)> = conn
            .query_row(
                "SELECT id, label, encrypted_credential FROM accounts WHERE tool_id = ?1 AND uid_hash = ?2",
                params![envelope.tool_id, uid_hash],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        let account_id = match existing {
            Some((id, stored_label, old_blob)) => {
                let aad = crypto::credential_aad(&envelope.tool_id, &id);
                let old_env = crypto::decrypt_credential(master_key, &aad, &old_blob).ok();
                let merged = match &old_env {
                    Some(old_env) => old_env.merge_from(envelope),
                    None => envelope.clone(),
                };
                let uid_masked = crypto::mask_uid(&merged.uid);
                let email_masked = merged.display_email.as_deref().map(crypto::mask_email);
                let encrypted_cred = crypto::encrypt_credential(master_key, &aad, &merged)?;
                let (has_snap, has_tok) = merged.capability_flags();
                let (dev_masked, dev_type) = merged
                    .device_identity()
                    .map(|(m, t)| (Some(m), t))
                    .unwrap_or((None, None));

                // 若旧 label 只是“自动生成”（等于旧默认昵称/UID脱敏），允许在本次拿到更准确的
                // 昵称/邮箱后刷新；用户自定义的备注（与之不同）则保持不变。
                let new_default = default_account_label(&merged);
                let label_was_auto = old_env
                    .as_ref()
                    .map(|e| stored_label == default_account_label(e))
                    .unwrap_or(false);
                let effective_label: String = match label_override {
                    Some(lo) => lo.to_string(),
                    None => {
                        if label_was_auto && !new_default.is_empty() {
                            new_default
                        } else {
                            stored_label
                        }
                    }
                };

                let (_, incoming_has_token) = envelope.capability_flags();
                let status_sql = if incoming_has_token {
                    "CASE WHEN status IN ('disabled', 'risk_paused') THEN status ELSE 'active' END"
                } else {
                    "CASE WHEN status IN ('disabled', 'risk_paused', 'expired') THEN status ELSE 'active' END"
                };
                let update_sql = format!(
                    "UPDATE accounts
                     SET label = ?1, uid_masked = ?2, email_masked = ?3,
                         status = {status_sql},
                         encrypted_credential = ?4, has_local_snapshot = ?5, has_api_token = ?6,
                         device_id_masked = ?7, device_type = ?8, updated_at = ?9
                     WHERE id = ?10"
                );
                conn.execute(
                    &update_sql,
                    params![
                        effective_label,
                        uid_masked,
                        email_masked,
                        encrypted_cred,
                        has_snap as i64,
                        has_tok as i64,
                        dev_masked,
                        dev_type,
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
                let aad = crypto::credential_aad(&envelope.tool_id, &id);
                let encrypted_cred = crypto::encrypt_credential(master_key, &aad, envelope)?;
                let (has_snap, has_tok) = envelope.capability_flags();
                let (dev_masked, dev_type) = envelope
                    .device_identity()
                    .map(|(m, t)| (Some(m), t))
                    .unwrap_or((None, None));

                conn.execute(
                    "INSERT INTO accounts (
                        id, tool_id, label, uid_masked, uid_hash, email_masked,
                        status, is_active_on_client, has_local_snapshot, has_api_token,
                        device_id_masked, device_type,
                        encrypted_credential, last_claimed_at, created_at, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 0, ?7, ?8, ?9, ?10, ?11, NULL, ?12, ?12)",
                    params![
                        id,
                        envelope.tool_id,
                        label,
                        uid_masked,
                        uid_hash,
                        email_masked,
                        has_snap as i64,
                        has_tok as i64,
                        dev_masked,
                        dev_type,
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

        let old_env = crypto::decrypt_credential(
            master_key,
            &crypto::credential_aad(&incoming.tool_id, account_id),
            &old_blob,
        )?;

        // 双方均为真实 UID 且不一致 → 凭据属于另一个账号，拒绝串号绑定
        let old_uid_real =
            !old_env.uid.trim().is_empty() && !CredentialEnvelope::is_fallback_uid(&old_env.uid);
        let incoming_uid_real =
            !incoming.uid.trim().is_empty() && !CredentialEnvelope::is_fallback_uid(&incoming.uid);
        if old_uid_real && incoming_uid_real && old_env.uid != incoming.uid {
            let incoming_name = incoming
                .display_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("未知账号");
            return Err(StorageError::Crypto(format!(
                "凭据归属不匹配：目标账号 {existing_label}（{}），本机当前账号 {incoming_name}（{}）。已拒绝绑定，请先在客户端切换到目标账号，或在对应账号卡片上操作。",
                crypto::mask_uid(&old_env.uid),
                crypto::mask_uid(&incoming.uid)
            )));
        }

        let merged = old_env.merge_from(incoming);
        let uid_hash = crypto::compute_uid_hash_hmac(master_key, &merged.tool_id, &merged.uid);

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
        let aad = crypto::credential_aad(&merged.tool_id, account_id);
        let encrypted_cred = crypto::encrypt_credential(master_key, &aad, &merged)?;
        let (has_snap, has_tok) = merged.capability_flags();
        let (dev_masked, dev_type) = merged
            .device_identity()
            .map(|(m, t)| (Some(m), t))
            .unwrap_or((None, None));
        let incoming_has_token = incoming.capability_flags().1;
        // 旧的自动生成 label（等于旧默认昵称/脱敏 UID）允许在拿到更准昵称后刷新；自定义备注保持。
        let label_was_auto = existing_label == default_account_label(&old_env);
        let new_default = default_account_label(&merged);
        let effective_label: String = match label_override.map(str::trim).filter(|s| !s.is_empty())
        {
            Some(lo) => lo.to_string(),
            None => {
                if label_was_auto && !new_default.is_empty() {
                    new_default
                } else {
                    existing_label.clone()
                }
            }
        };

        let status_sql = if incoming_has_token {
            "CASE WHEN status = 'disabled' THEN 'disabled' ELSE 'active' END"
        } else {
            "CASE WHEN status IN ('disabled', 'risk_paused', 'expired') THEN status ELSE 'active' END"
        };
        let update_sql = format!(
            "UPDATE accounts
             SET label = ?1, uid_masked = ?2, uid_hash = ?3, email_masked = ?4,
                 status = {status_sql},
                 encrypted_credential = ?5,
                 has_local_snapshot = ?6, has_api_token = ?7,
                 device_id_masked = ?8, device_type = ?9, updated_at = ?10
             WHERE id = ?11"
        );
        conn.execute(
            &update_sql,
            params![
                effective_label,
                uid_masked,
                uid_hash,
                email_masked,
                encrypted_cred,
                has_snap as i64,
                has_tok as i64,
                dev_masked,
                dev_type,
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
                    q.plan_name, q.claim_status, q.buckets_json, q.fetched_at, a.device_id_masked, a.device_type
             FROM accounts a
             LEFT JOIN quota_snapshots q ON q.id = (
                 SELECT q2.id FROM quota_snapshots q2
                 WHERE q2.account_id = a.id
                 ORDER BY julianday(q2.fetched_at) DESC, q2.fetched_at DESC, q2.rowid DESC
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
                        q.plan_name, q.claim_status, q.buckets_json, q.fetched_at, a.device_id_masked, a.device_type
                 FROM accounts a
                 LEFT JOIN quota_snapshots q ON q.id = (
                     SELECT q2.id FROM quota_snapshots q2
                     WHERE q2.account_id = a.id
                     ORDER BY julianday(q2.fetched_at) DESC, q2.fetched_at DESC, q2.rowid DESC
                     LIMIT 1
                 )
                 WHERE a.tool_id = ?1
                 ORDER BY a.is_active_on_client DESC, julianday(a.created_at) ASC, a.rowid ASC",
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
                        q.plan_name, q.claim_status, q.buckets_json, q.fetched_at, a.device_id_masked, a.device_type
                 FROM accounts a
                 LEFT JOIN quota_snapshots q ON q.id = (
                     SELECT q2.id FROM quota_snapshots q2
                     WHERE q2.account_id = a.id
                     ORDER BY julianday(q2.fetched_at) DESC, q2.fetched_at DESC, q2.rowid DESC
                     LIMIT 1
                 )
                 ORDER BY a.tool_id ASC, a.is_active_on_client DESC, julianday(a.created_at) ASC, a.rowid ASC",
            )?;
            let rows = stmt.query_map([], map_account_row)?;
            for row in rows {
                results.push(row?);
            }
        }

        Ok(results)
    }

    /// 按真实 UID 查找已存在的账号（用于关联本机前的归属预检）
    pub fn find_account_id_by_uid(
        &self,
        master_key: &[u8; 32],
        tool_id: &str,
        uid: &str,
    ) -> Result<Option<(String, String)>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let uid_hash = crypto::compute_uid_hash_hmac(master_key, tool_id, uid);
        let found = conn
            .query_row(
                "SELECT id, label FROM accounts WHERE tool_id = ?1 AND uid_hash = ?2",
                params![tool_id, uid_hash],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(found)
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

        let aad = crypto::credential_aad(&summary.tool_id, account_id);
        let envelope = match crypto::decrypt_credential(master_key, &aad, &blob) {
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

    /// 一次性迁移：旧版明文 PII 标签（含复合标签）与历史日志分页脱敏；
    /// 仅在存在待迁移记录时创建 0600 临时备份，事务提交后清理明文备份文件并写入完成标记
    /// 将历史无盐 `SHA-256(tool_id:uid)` 的 `uid_hash` 一次性平滑升级为
    /// `HMAC-SHA256(master_key, tool_id:uid)`。
    ///
    /// 以 `app_settings.storage.uid_hash_algo` 标记保证幂等：新装库（无账号）
    /// 首次也会打标，避免后续误重算；旧库逐行解密 `encrypted_credential` 取真实
    /// uid 重算哈希（新旧哈希对同一 (tool_id, uid) 为双射，不会引入唯一冲突）。
    pub fn migrate_uid_hash(&self, master_key: &[u8; 32]) -> Result<(), StorageError> {
        const FLAG: &str = "storage.uid_hash_algo";
        const ALGO: &str = "hmac-sha256-v1";
        if self.get_setting(FLAG)?.as_deref() == Some(ALGO) {
            return Ok(());
        }

        let updates: Vec<(String, String)> = {
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            let mut stmt =
                conn.prepare("SELECT id, tool_id, uid_hash, encrypted_credential FROM accounts")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Vec<u8>>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;

            let mut updates = Vec::new();
            for (id, tool_id, stored_hash, blob) in rows {
                let env = match crypto::decrypt_credential(
                    master_key,
                    &crypto::credential_aad(&tool_id, &id),
                    &blob,
                ) {
                    Ok(env) => env,
                    // 无法解密的密文保持原样，交由主密钥校验/恢复流程处理
                    Err(_) => continue,
                };
                let new_hash = crypto::compute_uid_hash_hmac(master_key, &tool_id, &env.uid);
                if stored_hash != new_hash {
                    updates.push((id, new_hash));
                }
            }
            updates
        };

        if !updates.is_empty() {
            let mut conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            let tx = conn.transaction()?;
            {
                let mut upd = tx.prepare("UPDATE accounts SET uid_hash = ?1 WHERE id = ?2")?;
                for (id, new_hash) in &updates {
                    upd.execute(params![new_hash, id])?;
                }
            }
            tx.commit()?;
        }
        self.set_setting(FLAG, ALGO)?;
        Ok(())
    }

    /// 旧格式（无 AAD）密文的账号 id 列表设置键。
    ///
    /// 迁移时解密失败的行无法重封装，会**永久**停留在「解密忽略 AAD」的兼容路径上，
    /// 即跨账号密文移植仍可成功。此处显式记录这些行，供启动告警与后续重试，
    /// 而不是静默 `continue` 后假装迁移完成。
    const LEGACY_CRED_IDS_KEY: &str = "storage.cred_aad_legacy_ids";

    /// 返回仍未升级到 AAD 格式的账号 id（供启动告警与 UI 提示）。
    pub fn legacy_aad_account_ids(&self) -> Vec<String> {
        self.get_setting(Self::LEGACY_CRED_IDS_KEY)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str::<Vec<String>>(&json).ok())
            .unwrap_or_default()
    }

    pub fn migrate_credential_aad(&self, master_key: &[u8; 32]) -> Result<(), StorageError> {
        const FLAG: &str = "storage.cred_aad";
        const ALGO: &str = "aad-v1";
        let already_done = self.get_setting(FLAG)?.as_deref() == Some(ALGO);
        // 已完成迁移也要重试此前失败的行（可能因临时密钥问题失败）
        if already_done && self.legacy_aad_account_ids().is_empty() {
            return Ok(());
        }

        let (updates, failed_ids): (Vec<(String, Vec<u8>)>, Vec<String>) = {
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            let mut stmt =
                conn.prepare("SELECT id, tool_id, encrypted_credential FROM accounts")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Vec<u8>>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;

            let mut updates = Vec::new();
            let mut failed_ids = Vec::new();
            for (id, tool_id, blob) in rows {
                if crypto::is_aad_sealed(&blob) {
                    continue;
                }
                let aad = crypto::credential_aad(&tool_id, &id);
                let env = match crypto::decrypt_credential(master_key, &aad, &blob) {
                    Ok(env) => env,
                    Err(_) => {
                        // 记录而非静默跳过：该行仍处于「AAD 可被绕过」状态
                        failed_ids.push(id);
                        continue;
                    }
                };
                let new_blob = crypto::encrypt_credential(master_key, &aad, &env)?;
                updates.push((id, new_blob));
            }
            (updates, failed_ids)
        };

        if !updates.is_empty() {
            let mut conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            let tx = conn.transaction()?;
            {
                let mut upd =
                    tx.prepare("UPDATE accounts SET encrypted_credential = ?1 WHERE id = ?2")?;
                for (id, blob) in &updates {
                    upd.execute(params![blob, id])?;
                }
            }
            tx.commit()?;
        }
        self.set_setting(FLAG, ALGO)?;
        // 记录仍未升级的行：这些行仍可被跨账号密文移植解密，
        // 启动时会据此告警，且下次启动继续尝试重封装。
        let failed_json = serde_json::to_string(&failed_ids)?;
        self.set_setting(Self::LEGACY_CRED_IDS_KEY, &failed_json)?;
        Ok(())
    }

    pub fn migrate_legacy_pii(&self, master_key: &[u8; 32]) -> Result<(), StorageError> {
        const FLAG: &str = "migration.pii_v1";
        if self.get_setting(FLAG)?.is_some() {
            return Ok(());
        }

        let (account_updates, log_updates) = {
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            let mut acc_stmt =
                conn.prepare("SELECT id, tool_id, label, encrypted_credential FROM accounts")?;
            let acc_rows = acc_stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Vec<u8>>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;

            let mut acc_updates = Vec::new();
            for (id, tool_id, label, blob) in acc_rows {
                let mut new_label = crypto::sanitize_log_message(&label);
                if let Ok(env) = crypto::decrypt_credential(
                    master_key,
                    &crypto::credential_aad(&tool_id, &id),
                    &blob,
                ) {
                    if !env.uid.trim().is_empty() && new_label == env.uid {
                        new_label = crypto::mask_uid(&env.uid);
                    }
                }
                if new_label != label {
                    acc_updates.push((id, new_label));
                }
            }

            let mut lg_updates = Vec::new();
            let mut last_id = 0i64;
            let mut log_stmt = conn.prepare(
                "SELECT id, message_sanitized FROM action_logs WHERE id > ?1 ORDER BY id ASC LIMIT 500",
            )?;
            loop {
                let batch = log_stmt
                    .query_map(params![last_id], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                if batch.is_empty() {
                    break;
                }
                for (id, msg) in batch {
                    last_id = id;
                    let sanitized = crypto::sanitize_log_message(&msg);
                    if sanitized != msg {
                        lg_updates.push((id, sanitized));
                    }
                }
            }

            (acc_updates, lg_updates)
        };

        if account_updates.is_empty() && log_updates.is_empty() {
            self.set_setting(FLAG, "\"done\"")?;
            return Ok(());
        }

        let backup = self.db_path.with_extension("db.pre-pii-migration.bak");
        let created_backup = if !backup.exists() && self.db_path.exists() {
            // 改用 SQLite 在线 Backup API 生成一致性快照（含 WAL 状态），
            // 避免 WAL 模式下直接 fs::copy 主库文件造成快照不一致/丢 WAL 页。
            let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            let mut dest = Connection::open(&backup)
                .map_err(|e| StorageError::Crypto(format!("创建迁移备份库失败: {e}")))?;
            {
                let backup_op = rusqlite::backup::Backup::new(&conn, &mut dest)
                    .map_err(|e| StorageError::Crypto(format!("迁移前备份初始化失败: {e}")))?;
                backup_op
                    .run_to_completion(5, std::time::Duration::from_millis(20), None)
                    .map_err(|e| StorageError::Crypto(format!("迁移前备份数据库失败: {e}")))?;
            }
            drop(dest);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o600));
            }
            true
        } else {
            false
        };

        // 单事务原子提交数据迁移与完成标记
        let mut conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let tx = conn.transaction()?;
        for (id, new_label) in account_updates {
            tx.execute(
                "UPDATE accounts SET label = ?1 WHERE id = ?2",
                params![new_label, id],
            )?;
        }
        for (id, sanitized) in log_updates {
            tx.execute(
                "UPDATE action_logs SET message_sanitized = ?1 WHERE id = ?2",
                params![sanitized, id],
            )?;
        }
        let now = Utc::now().to_rfc3339();
        tx.execute(
            "INSERT INTO app_settings (key, value_json, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
            params![FLAG, "\"done\"", now],
        )?;
        tx.commit()?;
        drop(conn);

        if created_backup || backup.exists() {
            let _ = std::fs::remove_file(&backup);
        }
        Ok(())
    }

    /// 取最近若干条账号密文样本，用于主密钥多样本容错校验。
    /// 查询失败必须向上传播（Fail-Closed），不得与"没有账号"混淆
    /// 取若干条凭据密文样本及其 AAD（`tool_id:account_id`），用于启动期主密钥校验。
    pub fn credential_sample_blobs(
        &self,
        limit: usize,
    ) -> Result<Vec<crypto::AadBlob>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let lim = limit.max(1) as i64;
        let mut stmt = conn.prepare(
            "SELECT id, tool_id, encrypted_credential FROM accounts ORDER BY updated_at DESC, rowid DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![lim], |row| {
                let id: String = row.get(0)?;
                let tool_id: String = row.get(1)?;
                let blob: Vec<u8> = row.get(2)?;
                Ok((crypto::credential_aad(&tool_id, &id), blob))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 严格解析 `key_id` 文件内容。
    ///
    /// 只有「32 位小写十六进制」才被视为有效指纹；空文件、截断内容、编码损坏、
    /// 写入中途断电留下的半截数据一律返回 `None`，按「指纹缺失」处理。
    /// 这避免了把损坏文件误判成「指纹冲突」而拒绝启动（密钥其实完全正确）。
    fn parse_key_fingerprint(raw: &str) -> Option<String> {
        let trimmed = raw.trim();
        if trimmed.len() == 32 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
            Some(trimmed.to_ascii_lowercase())
        } else {
            None
        }
    }

    /// 原子写入 `key_id`：同目录临时文件 + `fsync` + `rename`。
    ///
    /// 直接 `fs::write` 在断电/崩溃时会留下空文件或截断内容，随后被误判为指纹冲突。
    /// 临时文件以 `0600` 创建，`rename` 在同一文件系统内是原子的，因此读者要么看到
    /// 旧内容，要么看到完整新内容，不会看到中间态。
    fn write_key_fingerprint_atomic(
        data_dir: &Path,
        fingerprint: &str,
    ) -> Result<(), StorageError> {
        use std::io::Write;

        let path = data_dir.join("key_id");
        let tmp = data_dir.join(format!(".key_id.tmp-{}", Uuid::new_v4()));

        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }

        let write_result = (|| -> std::io::Result<()> {
            let mut file = opts.open(&tmp)?;
            file.write_all(fingerprint.as_bytes())?;
            file.sync_all()?;
            Ok(())
        })();

        if let Err(e) = write_result {
            let _ = std::fs::remove_file(&tmp);
            return Err(StorageError::Crypto(format!(
                "写入密钥指纹临时文件失败: {e}"
            )));
        }

        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(StorageError::Crypto(format!("替换密钥指纹文件失败: {e}")));
        }

        // rename 后同步目录项，确保崩溃后目录条目也落盘（Unix 有效，Windows 忽略）
        #[cfg(unix)]
        {
            if let Ok(dir) = std::fs::File::open(data_dir) {
                let _ = dir.sync_all();
            }
        }

        Ok(())
    }

    /// 启动时校验主密钥指纹与多条密文样本；首次生成或清理孤儿 key_id 前
    /// 必须确认是否存在无法解密的既有账号数据，避免把错误密钥"祝福"为合法密钥
    pub fn verify_or_init_key_fingerprint(
        &self,
        data_dir: &Path,
        master_key: &[u8; 32],
    ) -> Result<(), StorageError> {
        let fingerprint = crypto::key_fingerprint(master_key);
        let path = data_dir.join("key_id");
        let samples = self.credential_sample_blobs(5)?;
        let has_accounts = !samples.is_empty();
        let any_sample_decrypts = samples
            .iter()
            .any(|(aad, blob)| crypto::decrypt_credential(master_key, aad, blob).is_ok());
        let data_dir_str = data_dir.display();

        // 严格解析既有指纹：空文件/截断/非 hex 一律视为「缺失」而非「冲突」，
        // 否则一次写入中断就会让正确的主密钥永久无法启动。
        let existing = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| Self::parse_key_fingerprint(&raw));

        if let Some(existing) = existing {
            if existing == fingerprint {
                if has_accounts && !any_sample_decrypts {
                    return Err(StorageError::Crypto(format!(
                        "key_id 指纹匹配但当前主密钥无法解密任何抽样账号凭据（指纹与数据不一致）。\
                         已拒绝启动，请检查 {data_dir_str}/key_id、{data_dir_str}/master.key 或系统钥匙串后恢复完整备份。"
                    )));
                }
                return Ok(());
            }
            if !has_accounts {
                Self::write_key_fingerprint_atomic(data_dir, &fingerprint)?;
                return Ok(());
            }
            if any_sample_decrypts {
                return Err(StorageError::Crypto(format!(
                    "当前主密钥可以解密既有凭据，但与本地指纹文件冲突：{data_dir_str}/key_id。\
                     已拒绝启动；若刚从备份重建数据库且确认主密钥正确，请清理该 key_id 文件后重试。"
                )));
            }
            return Err(StorageError::Crypto(format!(
                "主密钥与本地密钥指纹 (key_id) 及数据库凭据均不匹配。\
                 已拒绝启动，请从系统钥匙串或 {data_dir_str}/master.key 备份恢复原密钥。"
            )));
        }

        if has_accounts && !any_sample_decrypts {
            return Err(StorageError::Crypto(format!(
                "主密钥无法解密既有账号凭据且缺少密钥指纹记录。\
                 已拒绝启动，请恢复原系统钥匙串条目或 {data_dir_str}/master.key 备份。"
            )));
        }

        Self::write_key_fingerprint_atomic(data_dir, &fingerprint)?;
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

    /// 清除某工具下所有账号的“本机使用中”标记（当本机当前登录账号不在库中时使用）
    pub fn clear_active_on_client(&self, tool_id: &str) -> Result<usize, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let affected = conn.execute(
            "UPDATE accounts SET is_active_on_client = 0 WHERE tool_id = ?1 AND is_active_on_client != 0",
            params![tool_id],
        )?;
        Ok(affected)
    }

    /// 查询某工具当前被标记为“本机使用中”的账号 ID
    pub fn get_active_account_id(&self, tool_id: &str) -> Result<Option<String>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.query_row(
            "SELECT id FROM accounts WHERE tool_id = ?1 AND is_active_on_client != 0 LIMIT 1",
            params![tool_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
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
        let normalized_fetched_at = DateTime::parse_from_rfc3339(&snapshot.fetched_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now())
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        conn.execute(
            "INSERT INTO quota_snapshots (id, account_id, plan_name, claim_status, buckets_json, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                account_id,
                snapshot.plan_name,
                claim_str,
                buckets_json,
                normalized_fetched_at
            ],
        )?;
        // 单账号最多保留最近 20 条额度快照，防止长期运行导致数据库膨胀
        let _ = conn.execute(
            "DELETE FROM quota_snapshots
             WHERE account_id = ?1
               AND id NOT IN (
                   SELECT id FROM quota_snapshots
                   WHERE account_id = ?1
                   ORDER BY julianday(fetched_at) DESC, fetched_at DESC, rowid DESC
                   LIMIT 20
               )",
            params![account_id],
        );
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
        device_id_masked: row.get(15)?,
        device_type: row.get(16)?,
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

pub fn is_today_cst(iso_str: &str) -> bool {
    let today_cst = Utc::now().with_timezone(&cst_offset()).date_naive();
    is_same_cst_day(iso_str, today_cst)
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
    // 每日重置型工具（Trae/WorkBuddy/MiniMax/LobsterAI/豆包工作）以 last_claimed_at 为权威；
    // Qoder 活动按周期而非自然日，不适用"今日已领即 Claimed"覆盖
    let daily_reset_tool = matches!(
        tool_id,
        "trae" | "trae_cn" | "workbuddy" | "minimax_code_cn" | "lobsterai" | "doubaowork"
    );
    let claimed_today = last_claimed_at
        .map(|ts| is_same_cst_day(ts, today_cst))
        .unwrap_or(false);
    match raw_status {
        ClaimStatus::Unsupported => ClaimStatus::Unsupported,
        ClaimStatus::Unknown => {
            if daily_reset_tool && claimed_today {
                ClaimStatus::Claimed
            } else {
                ClaimStatus::Unknown
            }
        }
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
        UnifiedErrorCode::DeviceDailyLimitReached => "DEVICE_DAILY_LIMIT_REACHED",
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
        "DEVICE_DAILY_LIMIT_REACHED" => Some(UnifiedErrorCode::DeviceDailyLimitReached),
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
mod tests;
