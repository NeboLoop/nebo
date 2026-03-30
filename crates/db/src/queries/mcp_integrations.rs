use rusqlite::params;

use crate::models::{McpCredentialFull, McpIntegration, McpIntegrationOAuth, McpOAuthConfig};
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

    /// Save OAuth flow state (PKCE verifier, endpoints, client creds) during OAuth initiation.
    pub fn set_mcp_oauth_state(
        &self,
        id: &str,
        state: &str,
        pkce_verifier: &str,
        client_id: &str,
        client_secret: Option<&str>,
        authorization_endpoint: &str,
        token_endpoint: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE mcp_integrations SET
                oauth_state = ?1,
                oauth_pkce_verifier = ?2,
                oauth_client_id = ?3,
                oauth_client_secret = ?4,
                oauth_authorization_endpoint = ?5,
                oauth_token_endpoint = ?6,
                updated_at = unixepoch()
             WHERE id = ?7",
            params![state, pkce_verifier, client_id, client_secret, authorization_endpoint, token_endpoint, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Look up an integration by OAuth state parameter (for callback validation).
    pub fn get_mcp_integration_by_oauth_state(&self, state: &str) -> Result<Option<McpIntegrationOAuth>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT id, name, server_url, auth_type, oauth_state, oauth_pkce_verifier,
                    oauth_client_id, oauth_client_secret, oauth_token_endpoint
             FROM mcp_integrations WHERE oauth_state = ?1",
            params![state],
            |row| {
                Ok(McpIntegrationOAuth {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    server_url: row.get(2)?,
                    auth_type: row.get(3)?,
                    oauth_state: row.get(4)?,
                    oauth_pkce_verifier: row.get(5)?,
                    oauth_client_id: row.get(6)?,
                    oauth_client_secret: row.get(7)?,
                    oauth_token_endpoint: row.get(8)?,
                })
            },
        ) {
            Ok(i) => Ok(Some(i)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    /// Clear OAuth flow state after successful callback (prevent replay).
    pub fn clear_mcp_oauth_state(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE mcp_integrations SET oauth_state = NULL, oauth_pkce_verifier = NULL, updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Store encrypted OAuth tokens in mcp_integration_credentials.
    pub fn store_mcp_credentials(
        &self,
        integration_id: &str,
        credential_type: &str,
        credential_value: &str,
        refresh_token: Option<&str>,
        expires_at: Option<i64>,
        scopes: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let id = uuid::Uuid::new_v4().to_string();
        // Delete existing credentials of this type for this integration
        conn.execute(
            "DELETE FROM mcp_integration_credentials WHERE integration_id = ?1 AND credential_type = ?2",
            params![integration_id, credential_type],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        conn.execute(
            "INSERT INTO mcp_integration_credentials (id, integration_id, credential_type, credential_value, refresh_token, expires_at, scopes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, integration_id, credential_type, credential_value, refresh_token, expires_at, scopes],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get OAuth access token for an integration (encrypted).
    pub fn get_mcp_credential(&self, integration_id: &str, credential_type: &str) -> Result<Option<(String, Option<String>)>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT credential_value, refresh_token FROM mcp_integration_credentials
             WHERE integration_id = ?1 AND credential_type = ?2
             ORDER BY rowid DESC LIMIT 1",
            params![integration_id, credential_type],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        ) {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    /// Get full OAuth credential including expiry (for token refresh decisions).
    pub fn get_mcp_credential_full(&self, integration_id: &str, credential_type: &str) -> Result<Option<McpCredentialFull>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT credential_value, refresh_token, expires_at, scopes
             FROM mcp_integration_credentials
             WHERE integration_id = ?1 AND credential_type = ?2
             ORDER BY rowid DESC LIMIT 1",
            params![integration_id, credential_type],
            |row| Ok(McpCredentialFull {
                credential_value: row.get(0)?,
                refresh_token: row.get(1)?,
                expires_at: row.get(2)?,
                scopes: row.get(3)?,
            }),
        ) {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    /// Get OAuth config needed for token refresh (client_id, client_secret, token_endpoint).
    pub fn get_mcp_oauth_config(&self, integration_id: &str) -> Result<Option<McpOAuthConfig>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT oauth_client_id, oauth_client_secret, oauth_token_endpoint
             FROM mcp_integrations WHERE id = ?1",
            params![integration_id],
            |row| Ok(McpOAuthConfig {
                oauth_client_id: row.get(0)?,
                oauth_client_secret: row.get(1)?,
                oauth_token_endpoint: row.get(2)?,
            }),
        ) {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
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
