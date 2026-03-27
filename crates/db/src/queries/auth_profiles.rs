use rusqlite::params;

use crate::models::AuthProfile;
use crate::Store;
use types::NeboError;

impl Store {
    pub fn create_auth_profile(
        &self,
        id: &str,
        name: &str,
        provider: &str,
        api_key: &str,
        model: Option<&str>,
        base_url: Option<&str>,
        priority: i64,
        is_active: i64,
        auth_type: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<AuthProfile, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO auth_profiles (id, name, provider, api_key, model, base_url, priority, is_active, auth_type, metadata, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, unixepoch(), unixepoch()) RETURNING *",
            params![id, name, provider, api_key, model, base_url, priority, is_active, auth_type, metadata],
            row_to_auth_profile,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_auth_profile(&self, id: &str) -> Result<Option<AuthProfile>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM auth_profiles WHERE id = ?1",
            params![id],
            row_to_auth_profile,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_auth_profile_by_name(&self, name: &str) -> Result<Option<AuthProfile>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM auth_profiles WHERE name = ?1",
            params![name],
            row_to_auth_profile,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_auth_profiles(&self) -> Result<Vec<AuthProfile>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM auth_profiles ORDER BY provider, priority DESC")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_auth_profile)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// List active auth profiles by provider, filtering out profiles in cooldown.
    /// Use this for **request-level profile selection** (choosing which profile handles a request).
    pub fn list_active_auth_profiles_by_provider(
        &self,
        provider: &str,
    ) -> Result<Vec<AuthProfile>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM auth_profiles
                 WHERE provider = ?1 AND is_active = 1
                 AND (cooldown_until IS NULL OR cooldown_until < unixepoch())
                 ORDER BY
                     CASE auth_type WHEN 'oauth' THEN 0 WHEN 'token' THEN 1 ELSE 2 END,
                     priority DESC, last_used_at ASC, error_count ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![provider], row_to_auth_profile)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// List active auth profiles by provider, ignoring cooldown.
    /// Use this for **loading providers** (building API clients, marketplace queries).
    pub fn list_all_active_auth_profiles_by_provider(
        &self,
        provider: &str,
    ) -> Result<Vec<AuthProfile>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM auth_profiles
                 WHERE provider = ?1 AND is_active = 1
                 ORDER BY
                     CASE auth_type WHEN 'oauth' THEN 0 WHEN 'token' THEN 1 ELSE 2 END,
                     priority DESC, last_used_at ASC, error_count ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![provider], row_to_auth_profile)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_best_auth_profile(
        &self,
        provider: &str,
    ) -> Result<Option<AuthProfile>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM auth_profiles
             WHERE provider = ?1 AND is_active = 1
             AND (cooldown_until IS NULL OR cooldown_until < unixepoch())
             ORDER BY
                 CASE auth_type WHEN 'oauth' THEN 0 WHEN 'token' THEN 1 ELSE 2 END,
                 priority DESC, last_used_at ASC
             LIMIT 1",
            params![provider],
            row_to_auth_profile,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_auth_profile_usage(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE auth_profiles SET last_used_at = unixepoch(), usage_count = usage_count + 1, error_count = 0, updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_auth_profile_error(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE auth_profiles SET error_count = error_count + 1, updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_auth_profile_cooldown(
        &self,
        id: &str,
        cooldown_until: i64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE auth_profiles SET cooldown_until = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, cooldown_until],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn toggle_auth_profile(&self, id: &str, is_active: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE auth_profiles SET is_active = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, is_active],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_auth_profile(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM auth_profiles WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_auth_profile(
        &self,
        id: &str,
        name: &str,
        api_key: &str,
        model: Option<&str>,
        base_url: Option<&str>,
        priority: i64,
        auth_type: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE auth_profiles SET name = ?2, api_key = ?3, model = ?4, base_url = ?5, priority = ?6, auth_type = ?7, metadata = ?8, updated_at = unixepoch() WHERE id = ?1",
            params![id, name, api_key, model, base_url, priority, auth_type, metadata],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Update only the api_key (token) for auth profiles matching a provider.
    /// Used for NeboLoop JWT rotation — gateway rotates the token on each AUTH_OK.
    pub fn update_auth_profile_token_by_provider(
        &self,
        provider: &str,
        api_key: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE auth_profiles SET api_key = ?2, updated_at = unixepoch() WHERE provider = ?1",
            params![provider, api_key],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}

fn row_to_auth_profile(row: &rusqlite::Row) -> rusqlite::Result<AuthProfile> {
    Ok(AuthProfile {
        id: row.get("id")?,
        name: row.get("name")?,
        provider: row.get("provider")?,
        api_key: row.get("api_key")?,
        model: row.get("model")?,
        base_url: row.get("base_url")?,
        priority: row.get("priority")?,
        is_active: row.get("is_active")?,
        cooldown_until: row.get("cooldown_until")?,
        last_used_at: row.get("last_used_at")?,
        usage_count: row.get("usage_count")?,
        error_count: row.get("error_count")?,
        metadata: row.get("metadata")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        auth_type: row.get("auth_type")?,
    })
}

trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
