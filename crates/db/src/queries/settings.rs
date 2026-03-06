use rusqlite::params;

use crate::models::Setting;
use crate::Store;
use types::NeboError;

impl Store {
    pub fn get_settings(&self) -> Result<Option<Setting>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT id, autonomous_mode, auto_approve_read, auto_approve_write,
                    auto_approve_bash, heartbeat_interval_minutes, comm_enabled,
                    comm_plugin, developer_mode, auto_update, updated_at
             FROM settings WHERE id = 1",
            [],
            |row| {
                Ok(Setting {
                    id: row.get(0)?,
                    autonomous_mode: row.get(1)?,
                    auto_approve_read: row.get(2)?,
                    auto_approve_write: row.get(3)?,
                    auto_approve_bash: row.get(4)?,
                    heartbeat_interval_minutes: row.get(5)?,
                    comm_enabled: row.get(6)?,
                    comm_plugin: row.get(7)?,
                    developer_mode: row.get(8)?,
                    auto_update: row.get(9)?,
                    updated_at: row.get(10)?,
                })
            },
        ) {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    pub fn update_settings(
        &self,
        autonomous_mode: Option<bool>,
        auto_approve_read: Option<bool>,
        auto_approve_write: Option<bool>,
        auto_approve_bash: Option<bool>,
        heartbeat_interval_minutes: Option<i64>,
        comm_enabled: Option<bool>,
        comm_plugin: Option<&str>,
        developer_mode: Option<bool>,
        auto_update: Option<bool>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        // Ensure settings row exists
        conn.execute(
            "INSERT OR IGNORE INTO settings (id) VALUES (1)",
            [],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;

        let mut updates = Vec::new();
        let mut param_idx = 1u32;

        macro_rules! maybe_set {
            ($field:expr, $col:expr) => {
                if $field.is_some() {
                    updates.push(format!("{} = ?{}", $col, param_idx));
                    #[allow(unused_assignments)]
                    { param_idx += 1; }
                }
            };
        }

        maybe_set!(autonomous_mode, "autonomous_mode");
        maybe_set!(auto_approve_read, "auto_approve_read");
        maybe_set!(auto_approve_write, "auto_approve_write");
        maybe_set!(auto_approve_bash, "auto_approve_bash");
        maybe_set!(heartbeat_interval_minutes, "heartbeat_interval_minutes");
        maybe_set!(comm_enabled, "comm_enabled");
        maybe_set!(comm_plugin, "comm_plugin");
        maybe_set!(developer_mode, "developer_mode");
        maybe_set!(auto_update, "auto_update");

        if updates.is_empty() {
            return Ok(());
        }

        updates.push(format!("updated_at = unixepoch()"));

        let sql = format!("UPDATE settings SET {} WHERE id = 1", updates.join(", "));
        let mut stmt = conn.prepare(&sql).map_err(|e| NeboError::Database(e.to_string()))?;

        // Build params dynamically
        let mut idx = 1;
        if let Some(v) = autonomous_mode {
            stmt.raw_bind_parameter(idx, v as i64).map_err(|e| NeboError::Database(e.to_string()))?;
            idx += 1;
        }
        if let Some(v) = auto_approve_read {
            stmt.raw_bind_parameter(idx, v as i64).map_err(|e| NeboError::Database(e.to_string()))?;
            idx += 1;
        }
        if let Some(v) = auto_approve_write {
            stmt.raw_bind_parameter(idx, v as i64).map_err(|e| NeboError::Database(e.to_string()))?;
            idx += 1;
        }
        if let Some(v) = auto_approve_bash {
            stmt.raw_bind_parameter(idx, v as i64).map_err(|e| NeboError::Database(e.to_string()))?;
            idx += 1;
        }
        if let Some(v) = heartbeat_interval_minutes {
            stmt.raw_bind_parameter(idx, v).map_err(|e| NeboError::Database(e.to_string()))?;
            idx += 1;
        }
        if let Some(v) = comm_enabled {
            stmt.raw_bind_parameter(idx, v as i64).map_err(|e| NeboError::Database(e.to_string()))?;
            idx += 1;
        }
        if let Some(v) = comm_plugin {
            stmt.raw_bind_parameter(idx, v).map_err(|e| NeboError::Database(e.to_string()))?;
            idx += 1;
        }
        if let Some(v) = developer_mode {
            stmt.raw_bind_parameter(idx, v as i64).map_err(|e| NeboError::Database(e.to_string()))?;
            idx += 1;
        }
        if let Some(v) = auto_update {
            stmt.raw_bind_parameter(idx, v as i64).map_err(|e| NeboError::Database(e.to_string()))?;
            let _ = idx + 1;
        }

        stmt.raw_execute().map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Read a plugin setting by plugin name and key.
    /// Used for bot_id migration from Go's plugin_settings table.
    pub fn get_plugin_setting(&self, plugin_name: &str, key: &str) -> Result<Option<String>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT ps.setting_value FROM plugin_settings ps
             JOIN plugin_registry pr ON ps.plugin_id = pr.id
             WHERE pr.name = ?1 AND ps.setting_key = ?2",
            params![plugin_name, key],
            |row| row.get(0),
        ) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    /// Write a plugin setting by plugin name and key.
    pub fn set_plugin_setting(&self, plugin_name: &str, key: &str, value: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO plugin_settings (id, plugin_id, setting_key, setting_value)
             VALUES (hex(randomblob(16)), (SELECT id FROM plugin_registry WHERE name = ?1), ?2, ?3)
             ON CONFLICT(plugin_id, setting_key) DO UPDATE
             SET setting_value = excluded.setting_value, updated_at = unixepoch()",
            params![plugin_name, key, value],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn create_user_preferences(&self, user_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR IGNORE INTO user_preferences (user_id) VALUES (?1)",
            params![user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}
