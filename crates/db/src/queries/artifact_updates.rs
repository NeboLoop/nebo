use rusqlite::params;

use crate::models::{ArtifactUpdateHistoryEntry, ArtifactUpdatePref, ArtifactUpdateSettings};
use crate::Store;
use types::NeboError;

impl Store {
    pub fn list_artifact_update_prefs(&self) -> Result<Vec<ArtifactUpdatePref>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT artifact_id, artifact_type, auto_update, local_version,
                        remote_version, last_checked_at, update_available
                 FROM artifact_update_prefs
                 ORDER BY artifact_type, artifact_id",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ArtifactUpdatePref {
                    artifact_id: row.get(0)?,
                    artifact_type: row.get(1)?,
                    name: None,
                    auto_update: row.get(2)?,
                    local_version: row.get(3)?,
                    remote_version: row.get(4)?,
                    last_checked_at: row.get(5)?,
                    update_available: row.get(6)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| NeboError::Database(e.to_string()))?);
        }
        Ok(results)
    }

    pub fn list_artifacts_with_updates(&self) -> Result<Vec<ArtifactUpdatePref>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT artifact_id, artifact_type, auto_update, local_version,
                        remote_version, last_checked_at, update_available, name
                 FROM artifact_update_prefs
                 WHERE update_available = 1
                 ORDER BY artifact_type, artifact_id",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ArtifactUpdatePref {
                    artifact_id: row.get(0)?,
                    artifact_type: row.get(1)?,
                    name: row.get(7)?,
                    auto_update: row.get(2)?,
                    local_version: row.get(3)?,
                    remote_version: row.get(4)?,
                    last_checked_at: row.get(5)?,
                    update_available: row.get(6)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| NeboError::Database(e.to_string()))?);
        }
        Ok(results)
    }

    pub fn upsert_artifact_update_pref(
        &self,
        artifact_id: &str,
        artifact_type: &str,
        local_version: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            // New installs default to MANUAL updates (auto_update = 0): the user is
            // notified and approves. On re-install/version-bump we DON'T touch
            // auto_update — the user's per-artifact opt-in choice is preserved.
            "INSERT INTO artifact_update_prefs (artifact_id, artifact_type, local_version, auto_update, last_checked_at)
             VALUES (?1, ?2, ?3, 0, unixepoch())
             ON CONFLICT(artifact_id, artifact_type) DO UPDATE
             SET local_version = excluded.local_version,
                 update_available = CASE WHEN remote_version != '' AND remote_version != excluded.local_version THEN 1 ELSE 0 END",
            params![artifact_id, artifact_type, local_version],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Remove an artifact's update-tracking row — call on uninstall so a
    /// no-longer-installed artifact doesn't leave an orphan that the checker
    /// keeps polling (and that could surface a phantom "update available").
    pub fn delete_artifact_update_pref(
        &self,
        artifact_id: &str,
        artifact_type: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM artifact_update_prefs WHERE artifact_id = ?1 AND artifact_type = ?2",
            params![artifact_id, artifact_type],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_artifact_remote_version(
        &self,
        artifact_id: &str,
        artifact_type: &str,
        remote_version: &str,
        has_update: bool,
        name: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE artifact_update_prefs
             SET remote_version = ?3, update_available = ?4, name = ?5, last_checked_at = unixepoch()
             WHERE artifact_id = ?1 AND artifact_type = ?2",
            params![artifact_id, artifact_type, remote_version, has_update as i64, name],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_artifact_auto_update(
        &self,
        artifact_id: &str,
        artifact_type: &str,
        enabled: bool,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE artifact_update_prefs SET auto_update = ?3
             WHERE artifact_id = ?1 AND artifact_type = ?2",
            params![artifact_id, artifact_type, enabled as i64],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Atomically claim an update for application. Returns true if the claim succeeded
    /// (update_available was 1 and is now 0). Returns false if another apply already claimed it.
    pub fn claim_artifact_update(
        &self,
        artifact_id: &str,
        artifact_type: &str,
    ) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE artifact_update_prefs SET update_available = 0
                 WHERE artifact_id = ?1 AND artifact_type = ?2 AND update_available = 1",
                params![artifact_id, artifact_type],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(changed > 0)
    }

    /// Re-mark an artifact as having an available update (e.g., after a failed apply).
    pub fn unclaim_artifact_update(
        &self,
        artifact_id: &str,
        artifact_type: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE artifact_update_prefs SET update_available = 1
             WHERE artifact_id = ?1 AND artifact_type = ?2",
            params![artifact_id, artifact_type],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn get_artifact_update_settings(&self) -> Result<ArtifactUpdateSettings, NeboError> {
        let conn = self.conn()?;
        let json_str: String = conn
            .query_row(
                "SELECT COALESCE(
                    (SELECT auto_update_artifacts FROM settings WHERE id = 1),
                    '{\"agents\":true,\"skills\":true,\"plugins\":true,\"connectors\":true,\"checkIntervalHours\":6}'
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        serde_json::from_str(&json_str).map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn set_artifact_update_settings(
        &self,
        settings: &ArtifactUpdateSettings,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let json_str =
            serde_json::to_string(settings).map_err(|e| NeboError::Database(e.to_string()))?;
        conn.execute(
            "UPDATE settings SET auto_update_artifacts = ?1 WHERE id = 1",
            params![json_str],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Append an entry to the artifact upgrade history log.
    pub fn record_artifact_update_history(
        &self,
        artifact_id: &str,
        artifact_type: &str,
        name: &str,
        from_version: &str,
        to_version: &str,
        status: &str,
        detail: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO artifact_update_history
                (artifact_id, artifact_type, name, from_version, to_version, status, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![artifact_id, artifact_type, name, from_version, to_version, status, detail],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Most-recent-first upgrade history (capped).
    pub fn list_artifact_update_history(
        &self,
        limit: i64,
    ) -> Result<Vec<ArtifactUpdateHistoryEntry>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, artifact_id, artifact_type, name, from_version, to_version,
                        status, detail, applied_at
                 FROM artifact_update_history
                 ORDER BY applied_at DESC, id DESC LIMIT ?1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit], |row| {
                Ok(ArtifactUpdateHistoryEntry {
                    id: row.get(0)?,
                    artifact_id: row.get(1)?,
                    artifact_type: row.get(2)?,
                    name: row.get(3)?,
                    from_version: row.get(4)?,
                    to_version: row.get(5)?,
                    status: row.get(6)?,
                    detail: row.get(7)?,
                    applied_at: row.get(8)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| NeboError::Database(e.to_string()))?);
        }
        Ok(out)
    }
}
