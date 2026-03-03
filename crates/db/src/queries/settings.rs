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
                    comm_plugin, developer_mode, updated_at
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
                    updated_at: row.get(9)?,
                })
            },
        ) {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
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
