use std::collections::HashMap;

use rusqlite::params;

use crate::Store;
use types::NeboError;

/// A channel binding row from the database.
#[derive(Debug, Clone)]
pub struct ChannelBindingRow {
    pub agent_id: String,
    pub plugin_slug: String,
    pub is_enabled: bool,
    /// Per-agent env var overrides (e.g., bot token, app token).
    pub config: HashMap<String, String>,
}

impl Store {
    /// List all channel bindings for a specific agent.
    pub fn list_channel_bindings_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<ChannelBindingRow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT agent_id, plugin_slug, is_enabled, config
                 FROM channel_bindings WHERE agent_id = ?1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![agent_id], |row| {
                let config_str: String = row.get(3)?;
                let config: HashMap<String, String> =
                    serde_json::from_str(&config_str).unwrap_or_default();
                Ok(ChannelBindingRow {
                    agent_id: row.get(0)?,
                    plugin_slug: row.get(1)?,
                    is_enabled: row.get::<_, i32>(2)? != 0,
                    config,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| NeboError::Database(e.to_string()))?);
        }
        Ok(result)
    }

    /// List all enabled channel bindings across all agents.
    pub fn list_enabled_channel_bindings(&self) -> Result<Vec<ChannelBindingRow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT agent_id, plugin_slug, is_enabled, config
                 FROM channel_bindings WHERE is_enabled = 1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                let config_str: String = row.get(3)?;
                let config: HashMap<String, String> =
                    serde_json::from_str(&config_str).unwrap_or_default();
                Ok(ChannelBindingRow {
                    agent_id: row.get(0)?,
                    plugin_slug: row.get(1)?,
                    is_enabled: row.get::<_, i32>(2)? != 0,
                    config,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| NeboError::Database(e.to_string()))?);
        }
        Ok(result)
    }

    /// Enable a channel plugin for an agent (upsert).
    pub fn enable_channel_binding(
        &self,
        agent_id: &str,
        plugin_slug: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO channel_bindings (agent_id, plugin_slug, is_enabled)
             VALUES (?1, ?2, 1)
             ON CONFLICT (agent_id, plugin_slug)
             DO UPDATE SET is_enabled = 1",
            params![agent_id, plugin_slug],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Disable a channel plugin for an agent.
    pub fn disable_channel_binding(
        &self,
        agent_id: &str,
        plugin_slug: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE channel_bindings SET is_enabled = 0
             WHERE agent_id = ?1 AND plugin_slug = ?2",
            params![agent_id, plugin_slug],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Save per-agent channel config (env var overrides).
    pub fn set_channel_binding_config(
        &self,
        agent_id: &str,
        plugin_slug: &str,
        config: &HashMap<String, String>,
    ) -> Result<(), NeboError> {
        let config_json =
            serde_json::to_string(config).map_err(|e| NeboError::Internal(e.to_string()))?;
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO channel_bindings (agent_id, plugin_slug, is_enabled, config)
             VALUES (?1, ?2, 1, ?3)
             ON CONFLICT (agent_id, plugin_slug)
             DO UPDATE SET config = ?3",
            params![agent_id, plugin_slug, config_json],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get per-agent channel config.
    pub fn get_channel_binding_config(
        &self,
        agent_id: &str,
        plugin_slug: &str,
    ) -> Result<HashMap<String, String>, NeboError> {
        let conn = self.conn()?;
        let config_str: String = conn
            .query_row(
                "SELECT config FROM channel_bindings WHERE agent_id = ?1 AND plugin_slug = ?2",
                params![agent_id, plugin_slug],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "{}".to_string());
        let config: HashMap<String, String> =
            serde_json::from_str(&config_str).unwrap_or_default();
        Ok(config)
    }

    /// Delete a channel binding.
    pub fn delete_channel_binding(
        &self,
        agent_id: &str,
        plugin_slug: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM channel_bindings WHERE agent_id = ?1 AND plugin_slug = ?2",
            params![agent_id, plugin_slug],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}
