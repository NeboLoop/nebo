use rusqlite::params;

use crate::models::EntityConfig;
use crate::Store;
use types::NeboError;

fn row_to_entity_config(row: &rusqlite::Row) -> rusqlite::Result<EntityConfig> {
    Ok(EntityConfig {
        id: row.get("id")?,
        entity_type: row.get("entity_type")?,
        entity_id: row.get("entity_id")?,
        heartbeat_enabled: row.get("heartbeat_enabled")?,
        heartbeat_interval_minutes: row.get("heartbeat_interval_minutes")?,
        heartbeat_content: row.get("heartbeat_content")?,
        heartbeat_window_start: row.get("heartbeat_window_start")?,
        heartbeat_window_end: row.get("heartbeat_window_end")?,
        permissions: row.get("permissions")?,
        resource_grants: row.get("resource_grants")?,
        model_preference: row.get("model_preference")?,
        personality_snippet: row.get("personality_snippet")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

impl Store {
    /// Get entity config by type and id.
    pub fn get_entity_config(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<Option<EntityConfig>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT * FROM entity_config WHERE entity_type = ?1 AND entity_id = ?2",
            params![entity_type, entity_id],
            row_to_entity_config,
        ) {
            Ok(c) => Ok(Some(c)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    /// Upsert entity config. NULL fields in the patch clear the override (inherit).
    pub fn upsert_entity_config(
        &self,
        entity_type: &str,
        entity_id: &str,
        patch: &serde_json::Value,
    ) -> Result<EntityConfig, NeboError> {
        let conn = self.conn()?;

        // Ensure row exists
        conn.execute(
            "INSERT OR IGNORE INTO entity_config (entity_type, entity_id) VALUES (?1, ?2)",
            params![entity_type, entity_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;

        let columns = [
            ("heartbeat_enabled", "heartbeatEnabled"),
            ("heartbeat_interval_minutes", "heartbeatIntervalMinutes"),
            ("heartbeat_content", "heartbeatContent"),
            ("heartbeat_window_start", "heartbeatWindowStart"),
            ("heartbeat_window_end", "heartbeatWindowEnd"),
            ("permissions", "permissions"),
            ("resource_grants", "resourceGrants"),
            ("model_preference", "modelPreference"),
            ("personality_snippet", "personalitySnippet"),
        ];

        for (col, json_key) in &columns {
            if let Some(val) = patch.get(json_key) {
                if val.is_null() {
                    // Clear override: set to NULL
                    let sql = format!(
                        "UPDATE entity_config SET {} = NULL, updated_at = unixepoch() WHERE entity_type = ?1 AND entity_id = ?2",
                        col
                    );
                    conn.execute(&sql, params![entity_type, entity_id])
                        .map_err(|e| NeboError::Database(e.to_string()))?;
                } else if let Some(s) = val.as_str() {
                    let sql = format!(
                        "UPDATE entity_config SET {} = ?1, updated_at = unixepoch() WHERE entity_type = ?2 AND entity_id = ?3",
                        col
                    );
                    conn.execute(&sql, params![s, entity_type, entity_id])
                        .map_err(|e| NeboError::Database(e.to_string()))?;
                } else if let Some(n) = val.as_i64() {
                    let sql = format!(
                        "UPDATE entity_config SET {} = ?1, updated_at = unixepoch() WHERE entity_type = ?2 AND entity_id = ?3",
                        col
                    );
                    conn.execute(&sql, params![n, entity_type, entity_id])
                        .map_err(|e| NeboError::Database(e.to_string()))?;
                } else if val.is_boolean() {
                    let b = val.as_bool().unwrap_or(false) as i64;
                    let sql = format!(
                        "UPDATE entity_config SET {} = ?1, updated_at = unixepoch() WHERE entity_type = ?2 AND entity_id = ?3",
                        col
                    );
                    conn.execute(&sql, params![b, entity_type, entity_id])
                        .map_err(|e| NeboError::Database(e.to_string()))?;
                } else if val.is_object() || val.is_array() {
                    // Store JSON objects/arrays as string
                    let s = val.to_string();
                    let sql = format!(
                        "UPDATE entity_config SET {} = ?1, updated_at = unixepoch() WHERE entity_type = ?2 AND entity_id = ?3",
                        col
                    );
                    conn.execute(&sql, params![s, entity_type, entity_id])
                        .map_err(|e| NeboError::Database(e.to_string()))?;
                }
            }
        }

        // Return the updated row
        self.get_entity_config(entity_type, entity_id)?
            .ok_or_else(|| NeboError::Database("entity_config row missing after upsert".into()))
    }

    /// Delete entity config (reset to inherited defaults).
    pub fn delete_entity_config(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM entity_config WHERE entity_type = ?1 AND entity_id = ?2",
            params![entity_type, entity_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// List all entities with heartbeat explicitly enabled.
    pub fn list_heartbeat_entities(&self) -> Result<Vec<EntityConfig>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM entity_config WHERE heartbeat_enabled = 1")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_entity_config)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }
}
