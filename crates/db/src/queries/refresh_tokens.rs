use rusqlite::params;

use crate::models::RefreshToken;
use crate::Store;
use types::NeboError;

impl Store {
    pub fn create_refresh_token(
        &self,
        id: &str,
        user_id: &str,
        token_hash: &str,
        expires_at: i64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO refresh_tokens (id, user_id, token_hash, expires_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, user_id, token_hash, expires_at],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn get_refresh_token_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<RefreshToken>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, token_hash, expires_at, created_at
                 FROM refresh_tokens
                 WHERE token_hash = ?1 AND expires_at > strftime('%s','now')",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;

        match stmt.query_row(params![token_hash], |row| {
            Ok(RefreshToken {
                id: row.get(0)?,
                user_id: row.get(1)?,
                token_hash: row.get(2)?,
                expires_at: row.get(3)?,
                created_at: row.get(4)?,
            })
        }) {
            Ok(token) => Ok(Some(token)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    pub fn delete_refresh_token(&self, token_hash: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM refresh_tokens WHERE token_hash = ?1",
            params![token_hash],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}
