use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::storage::{QwitchDatabase, StorageError};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredPluginState {
    pub plugin_id: String,
    pub kind: String,
    pub source: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_hash: Option<String>,
    pub overrides_builtin: bool,
    pub installed_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredCapabilityProbe {
    pub egress_plugin_id: String,
    pub model_id: String,
    pub probe_json: String,
    pub verified_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_summary: Option<String>,
}

impl QwitchDatabase {
    pub fn list_plugin_states(&self) -> Result<Vec<StoredPluginState>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT plugin_id, kind, source, enabled, pinned_hash, overrides_builtin, installed_at, updated_at
             FROM plugin_states
             ORDER BY plugin_id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StoredPluginState {
                plugin_id: row.get(0)?,
                kind: row.get(1)?,
                source: row.get(2)?,
                enabled: row.get::<_, i64>(3)? != 0,
                pinned_hash: row.get(4)?,
                overrides_builtin: row.get::<_, i64>(5)? != 0,
                installed_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn upsert_plugin_state(&self, state: &StoredPluginState) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.execute(
            "INSERT INTO plugin_states
             (plugin_id, kind, source, enabled, pinned_hash, overrides_builtin, installed_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(plugin_id) DO UPDATE SET
               kind = excluded.kind,
               source = excluded.source,
               enabled = excluded.enabled,
               pinned_hash = excluded.pinned_hash,
               overrides_builtin = excluded.overrides_builtin,
               updated_at = excluded.updated_at",
            params![
                state.plugin_id,
                state.kind,
                state.source,
                if state.enabled { 1 } else { 0 },
                state.pinned_hash,
                if state.overrides_builtin { 1 } else { 0 },
                state.installed_at,
                state.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn set_plugin_enabled(
        &self,
        plugin_id: &str,
        kind: &str,
        source: &str,
        enabled: bool,
    ) -> Result<(), StorageError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.execute(
            "INSERT INTO plugin_states
             (plugin_id, kind, source, enabled, pinned_hash, overrides_builtin, installed_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, NULL, 0, ?5, ?5)
             ON CONFLICT(plugin_id) DO UPDATE SET
               enabled = excluded.enabled,
               updated_at = excluded.updated_at",
            params![plugin_id, kind, source, if enabled { 1 } else { 0 }, now],
        )?;
        Ok(())
    }

    pub fn delete_plugin_state(&self, plugin_id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.execute(
            "DELETE FROM plugin_states WHERE plugin_id = ?1",
            params![plugin_id],
        )?;
        Ok(())
    }

    pub fn get_capability_probe(
        &self,
        egress_plugin_id: &str,
        model_id: &str,
    ) -> Result<Option<StoredCapabilityProbe>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.query_row(
            "SELECT egress_plugin_id, model_id, probe_json, verified_at, error_summary
             FROM plugin_capability_probes
             WHERE egress_plugin_id = ?1 AND model_id = ?2",
            params![egress_plugin_id, model_id],
            |row| {
                Ok(StoredCapabilityProbe {
                    egress_plugin_id: row.get(0)?,
                    model_id: row.get(1)?,
                    probe_json: row.get(2)?,
                    verified_at: row.get(3)?,
                    error_summary: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_capability_probes(
        &self,
        egress_plugin_id: Option<&str>,
    ) -> Result<Vec<StoredCapabilityProbe>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut out = Vec::new();
        if let Some(pid) = egress_plugin_id {
            let mut stmt = conn.prepare(
                "SELECT egress_plugin_id, model_id, probe_json, verified_at, error_summary
                 FROM plugin_capability_probes
                 WHERE egress_plugin_id = ?1
                 ORDER BY model_id ASC",
            )?;
            let rows = stmt.query_map(params![pid], |row| {
                Ok(StoredCapabilityProbe {
                    egress_plugin_id: row.get(0)?,
                    model_id: row.get(1)?,
                    probe_json: row.get(2)?,
                    verified_at: row.get(3)?,
                    error_summary: row.get(4)?,
                })
            })?;
            for row in rows {
                out.push(row?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT egress_plugin_id, model_id, probe_json, verified_at, error_summary
                 FROM plugin_capability_probes
                 ORDER BY egress_plugin_id ASC, model_id ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(StoredCapabilityProbe {
                    egress_plugin_id: row.get(0)?,
                    model_id: row.get(1)?,
                    probe_json: row.get(2)?,
                    verified_at: row.get(3)?,
                    error_summary: row.get(4)?,
                })
            })?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    pub fn upsert_capability_probe(
        &self,
        probe: &StoredCapabilityProbe,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.execute(
            "INSERT INTO plugin_capability_probes
             (egress_plugin_id, model_id, probe_json, verified_at, error_summary)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(egress_plugin_id, model_id) DO UPDATE SET
               probe_json = excluded.probe_json,
               verified_at = excluded.verified_at,
               error_summary = excluded.error_summary",
            params![
                probe.egress_plugin_id,
                probe.model_id,
                probe.probe_json,
                probe.verified_at,
                probe.error_summary,
            ],
        )?;
        Ok(())
    }

    pub fn delete_capability_probe(
        &self,
        egress_plugin_id: &str,
        model_id: &str,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.execute(
            "DELETE FROM plugin_capability_probes WHERE egress_plugin_id = ?1 AND model_id = ?2",
            params![egress_plugin_id, model_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_state_and_probe_crud_roundtrip() {
        let path =
            std::env::temp_dir().join(format!("qwitch_plugin_db_{}.db", uuid::Uuid::new_v4()));
        let db = QwitchDatabase::open(&path).unwrap();

        let state = StoredPluginState {
            plugin_id: "codex_sub".into(),
            kind: "egress".into(),
            source: "external".into(),
            enabled: true,
            pinned_hash: Some("abc123".into()),
            overrides_builtin: false,
            installed_at: "2026-11-28T00:00:00Z".into(),
            updated_at: "2026-11-28T00:00:00Z".into(),
        };
        db.upsert_plugin_state(&state).unwrap();
        let list = db.list_plugin_states().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], state);

        db.set_plugin_enabled("codex_sub", "egress", "external", false)
            .unwrap();
        assert!(!db.list_plugin_states().unwrap()[0].enabled);

        let probe = StoredCapabilityProbe {
            egress_plugin_id: "codex_sub".into(),
            model_id: "gpt-5.4".into(),
            probe_json: r#"{"streaming":true,"tools":true}"#.into(),
            verified_at: "2026-11-28T00:00:00Z".into(),
            error_summary: None,
        };
        db.upsert_capability_probe(&probe).unwrap();
        assert_eq!(
            db.get_capability_probe("codex_sub", "gpt-5.4").unwrap(),
            Some(probe)
        );
        assert_eq!(
            db.list_capability_probes(Some("codex_sub")).unwrap().len(),
            1
        );

        db.delete_capability_probe("codex_sub", "gpt-5.4").unwrap();
        assert!(db
            .get_capability_probe("codex_sub", "gpt-5.4")
            .unwrap()
            .is_none());

        db.delete_plugin_state("codex_sub").unwrap();
        assert!(db.list_plugin_states().unwrap().is_empty());

        let _ = std::fs::remove_file(&path);
    }
}
