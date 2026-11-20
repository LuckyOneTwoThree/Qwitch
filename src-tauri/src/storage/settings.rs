//! 应用设置 KV 域：`app_settings` 表的读写。

use super::*;
use chrono::Utc;
use rusqlite::params;

impl QwitchDatabase {
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
