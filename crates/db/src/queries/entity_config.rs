use rusqlite::params;

use crate::Store;
use crate::models::EntityConfig;
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
        allowed_paths: row.get("allowed_paths")?,
        pinned: row.get("pinned")?,
        multi_chat: row.get("multi_chat")?,
        operation_policy: row.get("operation_policy")?,
        learning_mode: row.get("learning_mode")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        last_heartbeat_at: row.get("last_heartbeat_at")?,
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
            ("model_preference", "modelPreference"),
            ("personality_snippet", "personalitySnippet"),
            ("pinned", "pinned"),
            ("multi_chat", "multiChat"),
            ("learning_mode", "learningMode"),
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

    /// Delete entity config (reset to inherited defaults). An employee with
    /// an outside door stays multi-chat: that is not a setting a reset
    /// returns to a default, it follows from the door.
    pub fn delete_entity_config(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<(), NeboError> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().map_err(|e| NeboError::Database(e.to_string()))?;
        tx.execute(
            "DELETE FROM entity_config WHERE entity_type = ?1 AND entity_id = ?2",
            params![entity_type, entity_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        tx.execute(
            "INSERT INTO entity_config (entity_type, entity_id, multi_chat)
             SELECT 'agent', a.id, 1 FROM agents a
             WHERE ?1 = 'agent' AND a.id = ?2
               AND (a.loop_exposed = 1
                    OR EXISTS (SELECT 1 FROM channel_bindings b WHERE b.agent_id = a.id AND b.is_enabled = 1))",
            params![entity_type, entity_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        tx.commit().map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// The doors on this computer through which someone outside reaches the
    /// employee: each enabled channel binding by plugin slug (the phone
    /// line's bridge is `phonecall`), and `loop` when it is exposed on the
    /// loop.
    pub fn outside_doors(&self, agent_id: &str) -> Result<Vec<String>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT plugin_slug FROM channel_bindings WHERE agent_id = ?1 AND is_enabled = 1
                 UNION ALL
                 SELECT 'loop' FROM agents WHERE id = ?1 AND loop_exposed = 1
                 ORDER BY 1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![agent_id], |r| r.get::<_, String>(0))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Make an employee multi-chat. Owner rule (09-25): an employee
    /// reachable from outside is a multi-chat employee, so every writer of a
    /// door calls this — the channel and loop writers here in the store, the
    /// NeboAI-side doors (a phone line, a webhook, a QR code) from their
    /// server paths.
    pub fn mark_multi_chat(&self, agent_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO entity_config (entity_type, entity_id, multi_chat) VALUES ('agent', ?1, 1)
             ON CONFLICT (entity_type, entity_id) DO UPDATE SET multi_chat = 1, updated_at = unixepoch()
             WHERE multi_chat IS NOT 1",
            params![agent_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// List all entities with heartbeat explicitly enabled.
    /// Every entity's config row of one type.
    pub fn list_entity_configs(&self, entity_type: &str) -> Result<Vec<EntityConfig>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM entity_config WHERE entity_type = ?1")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([entity_type], row_to_entity_config)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

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

    /// Update last heartbeat timestamp for an entity.
    pub fn update_heartbeat_at(
        &self,
        entity_type: &str,
        entity_id: &str,
        fired_at: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE entity_config SET last_heartbeat_at = ?1
             WHERE entity_type = ?2 AND entity_id = ?3",
            params![fired_at, entity_type, entity_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod outside_door_tests {
    use crate::Store;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-doors-test-{}.db", uuid::Uuid::new_v4()));
        let s = Store::new(&path.to_string_lossy()).expect("store");
        for id in ["desk", "scout", "quiet"] {
            s.create_agent(id, None, id, "", "", "{}", None, None).expect("agent row");
        }
        s
    }

    fn multi_chat(s: &Store, id: &str) -> Option<i64> {
        s.get_entity_config("agent", id).unwrap().and_then(|c| c.multi_chat)
    }

    /// Binding any door turns multi-chat on, through every writer of a
    /// binding: a channel (Slack, the phone line's bridge), exposure on the
    /// loop by the seed path and by the owner's save.
    #[test]
    fn binding_a_door_turns_multi_chat_on() {
        let s = store();
        s.enable_channel_binding("desk", "phonecall").unwrap();
        assert_eq!(multi_chat(&s, "desk"), Some(1));
        s.set_loop_exposed("scout", true).unwrap();
        assert_eq!(multi_chat(&s, "scout"), Some(1));
        let a = s.get_agent("quiet").unwrap().unwrap();
        s.update_agent("quiet", "quiet", "", "", &a.frontmatter, None, None, None, None, None, None, Some(true), None, None, None).unwrap();
        assert_eq!(multi_chat(&s, "quiet"), Some(1));
        assert_eq!(s.outside_doors("desk").unwrap(), vec!["phonecall".to_string()]);
        assert_eq!(s.outside_doors("scout").unwrap(), vec!["loop".to_string()]);
    }

    /// A switched-off channel is no longer a door; a reset of a bound
    /// employee's settings keeps multi-chat, an unbound one's does not.
    #[test]
    fn a_reset_keeps_multi_chat_while_a_door_is_bound() {
        let s = store();
        s.enable_channel_binding("desk", "slack").unwrap();
        s.upsert_entity_config("agent", "desk", &serde_json::json!({"modelPreference": "janus/fast"})).unwrap();
        s.delete_entity_config("agent", "desk").unwrap();
        let cfg = s.get_entity_config("agent", "desk").unwrap().expect("row kept for the bound employee");
        assert_eq!(cfg.multi_chat, Some(1));
        assert_eq!(cfg.model_preference, None, "everything else is reset");

        s.disable_channel_binding("desk", "slack").unwrap();
        assert!(s.outside_doors("desk").unwrap().is_empty());
        s.delete_entity_config("agent", "desk").unwrap();
        assert!(s.get_entity_config("agent", "desk").unwrap().is_none());
    }
}
