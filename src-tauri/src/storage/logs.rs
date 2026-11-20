//! 操作日志域：action_logs 的增删改查、统计、归档/保留策略与批量变更。
//! 通过独立 `impl QwitchDatabase` 块拆分，字段/存储结构仍在 `storage::mod` 定义。

use super::*;
use rusqlite::{params, params_from_iter};

impl QwitchDatabase {
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
        before_id: Option<i64>,
    ) -> Result<Vec<ActionLogItem>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut clauses: Vec<&'static str> = Vec::new();
        match scope {
            LogScope::Active => clauses.push("archived_at IS NULL"),
            LogScope::Archived => clauses.push("archived_at IS NOT NULL"),
            LogScope::All => {}
        }
        if before_id.is_some() {
            clauses.push("id < ?");
        }
        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        let clamped_limit = limit.clamp(1, 10_000) as i64;
        let sql = if before_id.is_some() {
            format!(
                "SELECT id, account_id, tool_id, action_type, result_status, error_code, message_sanitized, created_at, archived_at
                 FROM action_logs {where_sql} ORDER BY id DESC LIMIT ?"
            )
        } else {
            format!(
                "SELECT id, account_id, tool_id, action_type, result_status, error_code, message_sanitized, created_at, archived_at
                 FROM action_logs {where_sql} ORDER BY id DESC LIMIT ? OFFSET ?"
            )
        };
        let mut stmt = conn.prepare(&sql)?;

        let mut query_params: Vec<Value> = Vec::new();
        if let Some(cursor) = before_id {
            query_params.push(Value::Integer(cursor));
            query_params.push(Value::Integer(clamped_limit));
        } else {
            query_params.push(Value::Integer(clamped_limit));
            query_params.push(Value::Integer(offset as i64));
        }
        let rows = stmt.query_map(params_from_iter(query_params), |row| {
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
            // 归档日志自动清底：归档超过保留窗口的记录直接清除，防止归档无限增长
            let _ = conn.execute(
                "DELETE FROM action_logs WHERE archived_at IS NOT NULL AND archived_at < ?1",
                params![cutoff],
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

        const MAX_ARCHIVED_LOGS: i64 = 5_000;
        let _ = conn.execute(
            "DELETE FROM action_logs
             WHERE archived_at IS NOT NULL
               AND id NOT IN (
                 SELECT id FROM action_logs WHERE archived_at IS NOT NULL ORDER BY id DESC LIMIT ?1
               )",
            params![MAX_ARCHIVED_LOGS],
        )?;

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
}
