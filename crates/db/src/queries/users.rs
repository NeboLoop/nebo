use rusqlite::params;

use crate::models::User;
use crate::Store;
use types::NeboError;

impl Store {
    pub fn get_user_by_id(&self, id: &str) -> Result<Option<User>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, email, password_hash, name, avatar_url, email_verified,
                        email_verify_token, email_verify_expires,
                        password_reset_token, password_reset_expires,
                        created_at, updated_at, role
                 FROM users WHERE id = ?1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;

        let user = stmt
            .query_row(params![id], |row| {
                Ok(User {
                    id: row.get(0)?,
                    email: row.get(1)?,
                    password_hash: row.get(2)?,
                    name: row.get(3)?,
                    avatar_url: row.get(4)?,
                    email_verified: row.get(5)?,
                    email_verify_token: row.get(6)?,
                    email_verify_expires: row.get(7)?,
                    password_reset_token: row.get(8)?,
                    password_reset_expires: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    role: row.get(12)?,
                })
            })
            .optional()
            .map_err(|e| NeboError::Database(e.to_string()))?;

        Ok(user)
    }

    pub fn get_user_by_email(&self, email: &str) -> Result<Option<User>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, email, password_hash, name, avatar_url, email_verified,
                        email_verify_token, email_verify_expires,
                        password_reset_token, password_reset_expires,
                        created_at, updated_at, role
                 FROM users WHERE LOWER(email) = LOWER(?1)",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;

        let user = stmt
            .query_row(params![email], |row| {
                Ok(User {
                    id: row.get(0)?,
                    email: row.get(1)?,
                    password_hash: row.get(2)?,
                    name: row.get(3)?,
                    avatar_url: row.get(4)?,
                    email_verified: row.get(5)?,
                    email_verify_token: row.get(6)?,
                    email_verify_expires: row.get(7)?,
                    password_reset_token: row.get(8)?,
                    password_reset_expires: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    role: row.get(12)?,
                })
            })
            .optional()
            .map_err(|e| NeboError::Database(e.to_string()))?;

        Ok(user)
    }

    pub fn create_user(
        &self,
        id: &str,
        email: &str,
        password_hash: &str,
        name: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO users (id, email, password_hash, name, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%s','now'), strftime('%s','now'))",
            params![id, email, password_hash, name],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn check_email_exists(&self, email: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM users WHERE LOWER(email) = LOWER(?1)",
                params![email],
                |row| row.get(0),
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(count > 0)
    }

    pub fn update_user_password(&self, user_id: &str, password_hash: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE users SET password_hash = ?2, password_reset_token = NULL,
                              password_reset_expires = NULL, updated_at = strftime('%s','now')
             WHERE id = ?1",
            params![user_id, password_hash],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_user(
        &self,
        user_id: &str,
        name: Option<&str>,
        email: Option<&str>,
        avatar_url: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE users SET
                name = COALESCE(?2, name),
                email = COALESCE(?3, email),
                avatar_url = COALESCE(?4, avatar_url),
                updated_at = strftime('%s','now')
             WHERE id = ?1",
            params![user_id, name, email, avatar_url],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_user(&self, user_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM users WHERE id = ?1", params![user_id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_password_reset_token(
        &self,
        user_id: &str,
        token: &str,
        expires: i64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE users SET password_reset_token = ?2, password_reset_expires = ?3,
                              updated_at = strftime('%s','now')
             WHERE id = ?1",
            params![user_id, token, expires],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn get_user_by_password_reset_token(&self, token: &str) -> Result<Option<User>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, email, password_hash, name, avatar_url, email_verified,
                        email_verify_token, email_verify_expires,
                        password_reset_token, password_reset_expires,
                        created_at, updated_at, role
                 FROM users WHERE password_reset_token = ?1
                 AND password_reset_expires > strftime('%s','now')",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;

        let user = stmt
            .query_row(params![token], |row| {
                Ok(User {
                    id: row.get(0)?,
                    email: row.get(1)?,
                    password_hash: row.get(2)?,
                    name: row.get(3)?,
                    avatar_url: row.get(4)?,
                    email_verified: row.get(5)?,
                    email_verify_token: row.get(6)?,
                    email_verify_expires: row.get(7)?,
                    password_reset_token: row.get(8)?,
                    password_reset_expires: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    role: row.get(12)?,
                })
            })
            .optional()
            .map_err(|e| NeboError::Database(e.to_string()))?;

        Ok(user)
    }

    pub fn count_users(&self) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn has_admin_user(&self) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM users WHERE role = 'admin'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(count > 0)
    }
}

/// Extension trait for optional query results.
trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
