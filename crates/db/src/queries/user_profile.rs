use rusqlite::params;

use crate::models::{UserPreference, UserProfile};
use crate::Store;
use types::NeboError;

impl Store {
    /// Get or create the local user ID. In this single-user local app,
    /// the onboarding flow may call profile/terms endpoints before the
    /// formal setup creates a user, so we ensure one exists.
    fn ensure_local_user_id(&self) -> Result<String, NeboError> {
        let conn = self.conn()?;
        match conn.query_row("SELECT id FROM users LIMIT 1", [], |row| {
            row.get::<_, String>(0)
        }) {
            Ok(id) => Ok(id),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                let id = uuid::Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO users (id, email, password_hash, name, role, created_at, updated_at)
                     VALUES (?1, 'local@nebo.local', '', 'Local User', 'admin', strftime('%s','now'), strftime('%s','now'))",
                    params![id],
                )
                .map_err(|e| NeboError::Database(e.to_string()))?;
                Ok(id)
            }
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    /// Get the first user profile (single-user local app).
    pub fn get_user_profile(&self) -> Result<Option<UserProfile>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT user_id, display_name, bio, location, timezone, occupation,
                    interests, communication_style, goals, context,
                    onboarding_completed, onboarding_step, created_at, updated_at,
                    tool_permissions, terms_accepted_at
             FROM user_profiles LIMIT 1",
            [],
            |row| {
                Ok(UserProfile {
                    user_id: row.get(0)?,
                    display_name: row.get(1)?,
                    bio: row.get(2)?,
                    location: row.get(3)?,
                    timezone: row.get(4)?,
                    occupation: row.get(5)?,
                    interests: row.get(6)?,
                    communication_style: row.get(7)?,
                    goals: row.get(8)?,
                    context: row.get(9)?,
                    onboarding_completed: row.get(10)?,
                    onboarding_step: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                    tool_permissions: row.get(14)?,
                    terms_accepted_at: row.get(15)?,
                })
            },
        ) {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    pub fn update_user_profile(
        &self,
        display_name: Option<&str>,
        bio: Option<&str>,
        location: Option<&str>,
        timezone: Option<&str>,
        occupation: Option<&str>,
        interests: Option<&str>,
        communication_style: Option<&str>,
        goals: Option<&str>,
        context: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let user_id = self.ensure_local_user_id()?;

        // Ensure profile exists
        conn.execute(
            "INSERT OR IGNORE INTO user_profiles (user_id) VALUES (?1)",
            params![user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;

        if let Some(v) = display_name {
            conn.execute("UPDATE user_profiles SET display_name = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = bio {
            conn.execute("UPDATE user_profiles SET bio = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = location {
            conn.execute("UPDATE user_profiles SET location = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = timezone {
            conn.execute("UPDATE user_profiles SET timezone = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = occupation {
            conn.execute("UPDATE user_profiles SET occupation = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = interests {
            conn.execute("UPDATE user_profiles SET interests = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = communication_style {
            conn.execute("UPDATE user_profiles SET communication_style = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = goals {
            conn.execute("UPDATE user_profiles SET goals = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = context {
            conn.execute("UPDATE user_profiles SET context = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }

        Ok(())
    }

    pub fn set_onboarding_completed(&self, completed: bool) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let user_id = self.ensure_local_user_id()?;
        conn.execute(
            "INSERT OR IGNORE INTO user_profiles (user_id, created_at, updated_at) VALUES (?1, unixepoch(), unixepoch())",
            params![user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        let val: i64 = if completed { 1 } else { 0 };
        conn.execute(
            "UPDATE user_profiles SET onboarding_completed = ?1, updated_at = unixepoch() WHERE user_id = ?2",
            params![val, user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_tool_permissions(&self, permissions: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let user_id = self.ensure_local_user_id()?;
        conn.execute(
            "INSERT OR IGNORE INTO user_profiles (user_id, created_at, updated_at) VALUES (?1, unixepoch(), unixepoch())",
            params![user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        conn.execute(
            "UPDATE user_profiles SET tool_permissions = ?1, updated_at = unixepoch() WHERE user_id = ?2",
            params![permissions, user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn accept_terms(&self) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let user_id = self.ensure_local_user_id()?;
        conn.execute(
            "INSERT OR IGNORE INTO user_profiles (user_id, created_at, updated_at) VALUES (?1, unixepoch(), unixepoch())",
            params![user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        conn.execute(
            "UPDATE user_profiles SET terms_accepted_at = unixepoch(), updated_at = unixepoch() WHERE user_id = ?1",
            params![user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get the first user's preferences (single-user local app).
    pub fn get_user_preferences(&self) -> Result<Option<UserPreference>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT user_id, email_notifications, marketing_emails, timezone,
                    language, theme, updated_at, inapp_notifications
             FROM user_preferences LIMIT 1",
            [],
            |row| {
                Ok(UserPreference {
                    user_id: row.get(0)?,
                    email_notifications: row.get(1)?,
                    marketing_emails: row.get(2)?,
                    timezone: row.get(3)?,
                    language: row.get(4)?,
                    theme: row.get(5)?,
                    updated_at: row.get(6)?,
                    inapp_notifications: row.get(7)?,
                })
            },
        ) {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    pub fn update_user_preferences(
        &self,
        theme: Option<&str>,
        language: Option<&str>,
        timezone: Option<&str>,
        email_notifications: Option<bool>,
        inapp_notifications: Option<bool>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let user_id = self.ensure_local_user_id()?;

        // Ensure preferences exist
        conn.execute(
            "INSERT OR IGNORE INTO user_preferences (user_id) VALUES (?1)",
            params![user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;

        if let Some(v) = theme {
            conn.execute("UPDATE user_preferences SET theme = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = language {
            conn.execute("UPDATE user_preferences SET language = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = timezone {
            conn.execute("UPDATE user_preferences SET timezone = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = email_notifications {
            let val: i64 = if v { 1 } else { 0 };
            conn.execute("UPDATE user_preferences SET email_notifications = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![val, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = inapp_notifications {
            let val: i64 = if v { 1 } else { 0 };
            conn.execute("UPDATE user_preferences SET inapp_notifications = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![val, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }

        Ok(())
    }
}
