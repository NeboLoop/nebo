use rusqlite::params;

use crate::models::McpIntegration;
use crate::Store;
use types::NeboError;

impl Store {
    pub fn list_mcp_integrations(&self) -> Result<Vec<McpIntegration>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, server_type, server_url, auth_type, is_enabled,
                        connection_status, last_connected_at, last_error, metadata,
                        created_at, updated_at, tool_count
                 FROM mcp_integrations ORDER BY name",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(McpIntegration {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    server_type: row.get(2)?,
                    server_url: row.get(3)?,
                    auth_type: row.get(4)?,
                    is_enabled: row.get(5)?,
                    connection_status: row.get(6)?,
                    last_connected_at: row.get(7)?,
                    last_error: row.get(8)?,
                    metadata: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    tool_count: row.get(12)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;

        let mut integrations = Vec::new();
        for row in rows {
            integrations.push(row.map_err(|e| NeboError::Database(e.to_string()))?);
        }
        Ok(integrations)
    }

    pub fn get_mcp_integration(&self, id: &str) -> Result<Option<McpIntegration>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT id, name, server_type, server_url, auth_type, is_enabled,
                    connection_status, last_connected_at, last_error, metadata,
                    created_at, updated_at, tool_count
             FROM mcp_integrations WHERE id = ?1",
            params![id],
            |row| {
                Ok(McpIntegration {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    server_type: row.get(2)?,
                    server_url: row.get(3)?,
                    auth_type: row.get(4)?,
                    is_enabled: row.get(5)?,
                    connection_status: row.get(6)?,
                    last_connected_at: row.get(7)?,
                    last_error: row.get(8)?,
                    metadata: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    tool_count: row.get(12)?,
                })
            },
        ) {
            Ok(i) => Ok(Some(i)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    pub fn create_mcp_integration(
        &self,
        id: &str,
        name: &str,
        server_type: &str,
        server_url: Option<&str>,
        auth_type: &str,
        metadata: Option<&str>,
    ) -> Result<McpIntegration, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO mcp_integrations (id, name, server_type, server_url, auth_type, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, name, server_type, server_url, auth_type, metadata],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;

        self.get_mcp_integration(id)?
            .ok_or_else(|| NeboError::Database("failed to retrieve created integration".into()))
    }

    pub fn update_mcp_integration(
        &self,
        id: &str,
        name: Option<&str>,
        server_url: Option<&str>,
        auth_type: Option<&str>,
        is_enabled: Option<bool>,
        metadata: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;

        if let Some(v) = name {
            conn.execute("UPDATE mcp_integrations SET name = ?1, updated_at = unixepoch() WHERE id = ?2", params![v, id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = server_url {
            conn.execute("UPDATE mcp_integrations SET server_url = ?1, updated_at = unixepoch() WHERE id = ?2", params![v, id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = auth_type {
            conn.execute("UPDATE mcp_integrations SET auth_type = ?1, updated_at = unixepoch() WHERE id = ?2", params![v, id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = is_enabled {
            let val: i64 = if v { 1 } else { 0 };
            conn.execute("UPDATE mcp_integrations SET is_enabled = ?1, updated_at = unixepoch() WHERE id = ?2", params![val, id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = metadata {
            conn.execute("UPDATE mcp_integrations SET metadata = ?1, updated_at = unixepoch() WHERE id = ?2", params![v, id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }

        Ok(())
    }

    pub fn delete_mcp_integration(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM mcp_integrations WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_mcp_server_type(&self, id: &str, server_type: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE mcp_integrations SET server_type = ?1, updated_at = unixepoch() WHERE id = ?2",
            params![server_type, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_mcp_connection_status(&self, id: &str, status: &str, tool_count: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE mcp_integrations SET connection_status = ?1, tool_count = ?2, last_connected_at = unixepoch(), updated_at = unixepoch() WHERE id = ?3",
            params![status, tool_count, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}
