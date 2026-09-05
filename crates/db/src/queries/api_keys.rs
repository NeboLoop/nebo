use rusqlite::{OptionalExtension, params};

use crate::Store;
use crate::models::ApiKey;
use types::NeboError;

fn row_to_api_key(row: &rusqlite::Row) -> rusqlite::Result<ApiKey> {
    let models: String = row.get("models")?;
    let tools: String = row.get("tools")?;
    Ok(ApiKey {
        id: row.get("id")?,
        label: row.get("label")?,
        key_prefix: row.get("key_prefix")?,
        agent_id: row.get("agent_id")?,
        models: serde_json::from_str(&models).unwrap_or_default(),
        tools: serde_json::from_str(&tools).unwrap_or_default(),
        created_at: row.get("created_at")?,
        last_used_at: row.get("last_used_at")?,
        revoked_at: row.get("revoked_at")?,
    })
}

impl Store {
    pub fn create_api_key(
        &self,
        id: &str,
        label: &str,
        key_hash: &str,
        key_prefix: &str,
        agent_id: &str,
        models: &[String],
        tools: &[String],
    ) -> Result<ApiKey, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO api_keys (id, label, key_hash, key_prefix, agent_id, models, tools)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING *",
            params![
                id,
                label,
                key_hash,
                key_prefix,
                agent_id,
                serde_json::to_string(models).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(tools).unwrap_or_else(|_| "[]".into()),
            ],
            row_to_api_key,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Live keys for one employee, newest first.
    pub fn list_api_keys_for_agent(&self, agent_id: &str) -> Result<Vec<ApiKey>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM api_keys WHERE agent_id = ?1 AND revoked_at IS NULL ORDER BY created_at DESC")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![agent_id], row_to_api_key)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// The live key behind a presented secret — the ONE lookup the door uses.
    pub fn find_api_key_by_hash(&self, key_hash: &str) -> Result<Option<ApiKey>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM api_keys WHERE key_hash = ?1 AND revoked_at IS NULL",
            params![key_hash],
            row_to_api_key,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn touch_api_key(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE api_keys SET last_used_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn revoke_api_key(&self, id: &str, agent_id: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let n = conn
            .execute(
                "UPDATE api_keys SET revoked_at = unixepoch() WHERE id = ?1 AND agent_id = ?2 AND revoked_at IS NULL",
                params![id, agent_id],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(n > 0)
    }
}
