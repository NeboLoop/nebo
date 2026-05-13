use rusqlite::params;

use crate::Store;
use types::NeboError;

/// A cached license key row.
pub struct LicenseKeyRow {
    pub artifact_id: String,
    pub artifact_type: String,
    pub scope: String,
    pub encrypted_key: String,
    pub expires_at: i64,
}

impl Store {
    /// Get a cached license key if not expired.
    pub fn get_license_key(&self, artifact_id: &str) -> Result<Option<LicenseKeyRow>, NeboError> {
        let conn = self.conn()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        match conn.query_row(
            "SELECT artifact_id, artifact_type, scope, encrypted_key, expires_at
             FROM license_keys WHERE artifact_id = ?1 AND expires_at > ?2",
            params![artifact_id, now],
            |row| {
                Ok(LicenseKeyRow {
                    artifact_id: row.get(0)?,
                    artifact_type: row.get(1)?,
                    scope: row.get(2)?,
                    encrypted_key: row.get(3)?,
                    expires_at: row.get(4)?,
                })
            },
        ) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    /// Store or update a license key with TTL.
    pub fn upsert_license_key(
        &self,
        artifact_id: &str,
        artifact_type: &str,
        scope: &str,
        encrypted_key: &str,
        expires_at: i64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO license_keys (artifact_id, artifact_type, scope, encrypted_key, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(artifact_id) DO UPDATE SET
                artifact_type = excluded.artifact_type,
                scope = excluded.scope,
                encrypted_key = excluded.encrypted_key,
                expires_at = excluded.expires_at",
            params![artifact_id, artifact_type, scope, encrypted_key, expires_at],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Remove expired license key entries.
    pub fn gc_license_keys(&self) -> Result<u64, NeboError> {
        let conn = self.conn()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let deleted = conn
            .execute(
                "DELETE FROM license_keys WHERE expires_at <= ?1",
                params![now],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(deleted as u64)
    }

    /// Remove all license keys (logout / account switch).
    pub fn clear_all_license_keys(&self) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM license_keys", [])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// List all non-expired license key artifact IDs.
    pub fn list_license_key_artifact_ids(&self) -> Result<Vec<String>, NeboError> {
        let conn = self.conn()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let mut stmt = conn
            .prepare("SELECT artifact_id FROM license_keys WHERE expires_at > ?1")
            .map_err(|e| NeboError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![now], |row| row.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(|e| NeboError::Database(e.to_string()))?);
        }
        Ok(ids)
    }
}
