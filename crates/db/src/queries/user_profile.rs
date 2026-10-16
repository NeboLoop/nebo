use rusqlite::params;

use crate::Store;
use crate::models::{RosterOrder, UserPreference, UserProfile};
use types::NeboError;

impl Store {
    /// Get or create the local user ID. In this single-user local app,
    /// the onboarding flow may call profile/terms endpoints before the
    /// formal setup creates a user, so we ensure one exists.
    pub fn ensure_local_user_id(&self) -> Result<String, NeboError> {
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
                    tool_permissions, terms_accepted_at, account_type, approved_commands,
                    onboarding_focus, onboarding_hire
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
                    account_type: row.get(16)?,
                    approved_commands: row.get(17)?,
                    onboarding_focus: row.get(18)?,
                    onboarding_hire: row.get(19)?,
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
        account_type: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let user_id = self.ensure_local_user_id()?;

        // Ensure profile exists (created_at/updated_at are NOT NULL: without
        // them the IGNORE swallows the insert and no field below is kept).
        conn.execute(
            "INSERT OR IGNORE INTO user_profiles (user_id, created_at, updated_at) VALUES (?1, unixepoch(), unixepoch())",
            params![user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;

        if let Some(v) = display_name {
            conn.execute("UPDATE user_profiles SET display_name = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = bio {
            conn.execute(
                "UPDATE user_profiles SET bio = ?1, updated_at = unixepoch() WHERE user_id = ?2",
                params![v, user_id],
            )
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
            conn.execute(
                "UPDATE user_profiles SET goals = ?1, updated_at = unixepoch() WHERE user_id = ?2",
                params![v, user_id],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = context {
            conn.execute("UPDATE user_profiles SET context = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
                .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        if let Some(v) = account_type {
            conn.execute("UPDATE user_profiles SET account_type = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
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

    /// Where the guided first-run chat is (`step`), and the answers later
    /// steps read back. `None` leaves an answer as it is.
    pub fn set_onboarding(&self, step: &str, focus: Option<&str>, hire: Option<&str>) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let user_id = self.ensure_local_user_id()?;
        conn.execute(
            "INSERT OR IGNORE INTO user_profiles (user_id, created_at, updated_at) VALUES (?1, unixepoch(), unixepoch())",
            params![user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        conn.execute(
            "UPDATE user_profiles SET onboarding_step = ?1,
                    onboarding_focus = COALESCE(?2, onboarding_focus),
                    onboarding_hire = COALESCE(?3, onboarding_hire),
                    updated_at = unixepoch()
             WHERE user_id = ?4",
            params![step, focus, hire, user_id],
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
                    language, theme, updated_at, inapp_notifications, start_page, locale,
                    roster_order
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
                    start_page: row.get(8)?,
                    locale: row.get(9)?,
                    roster_order: roster_order_of(row.get(10)?),
                })
            },
        ) {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    /// Keep the owner's roster order (what is pinned, and the order of the
    /// teams and the employees), replacing the one before.
    pub fn set_roster_order(&self, order: &RosterOrder) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let user_id = self.ensure_local_user_id()?;
        let json = serde_json::to_string(order).map_err(|e| NeboError::Internal(e.to_string()))?;
        conn.execute("INSERT OR IGNORE INTO user_preferences (user_id) VALUES (?1)", params![user_id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        conn.execute(
            "UPDATE user_preferences SET roster_order = ?1, updated_at = unixepoch() WHERE user_id = ?2",
            params![json, user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Keep the account's full locale tag ("es-MX"): the region the
    /// language alone does not carry.
    pub fn set_user_locale(&self, tag: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let user_id = self.ensure_local_user_id()?;
        conn.execute("INSERT OR IGNORE INTO user_preferences (user_id) VALUES (?1)", params![user_id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        conn.execute(
            "UPDATE user_preferences SET locale = ?1, updated_at = unixepoch() WHERE user_id = ?2",
            params![tag, user_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_user_preferences(
        &self,
        theme: Option<&str>,
        language: Option<&str>,
        timezone: Option<&str>,
        email_notifications: Option<bool>,
        inapp_notifications: Option<bool>,
        start_page: Option<&str>,
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
        if let Some(v) = start_page {
            conn.execute("UPDATE user_preferences SET start_page = ?1, updated_at = unixepoch() WHERE user_id = ?2", params![v, user_id])
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

/// The stored roster order; none yet, or one that no longer reads, is the
/// empty order (every list A to Z) rather than a preferences read that fails.
fn roster_order_of(raw: Option<String>) -> RosterOrder {
    raw.and_then(|json| match serde_json::from_str(&json) {
        Ok(order) => Some(order),
        Err(e) => {
            tracing::warn!(error = %e, "roster order unreadable; using A to Z");
            None
        }
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use crate::Store;
    use crate::models::{RosterItem, RosterKind, RosterOrder};

    fn temp_store() -> Store {
        let path = std::env::temp_dir().join(format!(
            "nebo-roster-order-test-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Store::new(path.to_str().unwrap()).unwrap()
    }

    fn ids<'a>(rows: &[(&'a str, &'a str)], order: Vec<usize>) -> Vec<&'a str> {
        order.into_iter().map(|i| rows[i].0).collect()
    }

    /// The roster order is kept on the bot and read back as stored; the
    /// primary is first of its list wherever it was put; rows the order does
    /// not name (a new hire) go after the named ones, A to Z.
    #[test]
    fn roster_order_persists_primary_first_new_at_bottom() {
        let store = temp_store();
        assert_eq!(store.get_user_preferences().unwrap().map(|p| p.roster_order).unwrap_or_default(), RosterOrder::default());

        let saved = RosterOrder {
            pinned: vec![
                RosterItem { kind: RosterKind::Team, id: "sales".into() },
                RosterItem { kind: RosterKind::Employee, id: "assistant".into() },
                RosterItem { kind: RosterKind::Team, id: "sales".into() },
            ],
            teams: vec!["month-end".into(), "launch".into()],
            employees: vec!["lead".into(), "assistant".into(), "copy".into()],
        }
        .normalized();
        store.set_roster_order(&saved).unwrap();
        let order = store.get_user_preferences().unwrap().unwrap().roster_order;
        assert_eq!(order, saved);
        assert_eq!(order.pinned.len(), 2, "a pin is kept once");

        // The primary leads the Pinned list though it was pinned second.
        assert_eq!(order.pin_position(RosterKind::Employee, "assistant", "assistant"), Some(0));
        assert_eq!(order.pin_position(RosterKind::Team, "sales", "assistant"), Some(1));
        assert_eq!(order.pin_position(RosterKind::Employee, "copy", "assistant"), None);

        // Employees: the primary first, the saved order, then a new hire.
        let people = [("copy", "Copywriter"), ("zed", "Zed"), ("lead", "Lead Finder"), ("assistant", "Jetson"), ("new", "Aaron")];
        assert_eq!(
            ids(&people, RosterOrder::arrange(&order.employees, Some("assistant"), &people)),
            ["assistant", "lead", "copy", "new", "zed"],
        );
        // Teams: the saved order, then the new team; never-ordered is A to Z.
        let teams = [("launch", "Launch Crew"), ("new-team", "Brand Refresh"), ("month-end", "Month End")];
        assert_eq!(ids(&teams, RosterOrder::arrange(&order.teams, None, &teams)), ["month-end", "launch", "new-team"]);
        assert_eq!(ids(&teams, RosterOrder::arrange(&[], None, &teams)), ["new-team", "launch", "month-end"]);
    }
}
