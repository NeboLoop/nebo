use rusqlite::params;
use serde::{Deserialize, Serialize};
use types::NeboError;

use crate::{OptionalExt, Store};

/// A session's agreed-goal row. `status` and `source` are the agent's
/// vocabulary (`harness::goal`); the store keeps them as text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGoal {
    pub session_id: String,
    pub condition: String,
    pub source: String,
    pub status: String,
    pub turns: i64,
    pub last_reason: Option<String>,
    /// Conditions the owner declined, oldest first.
    pub declined: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn row_to_goal(row: &rusqlite::Row) -> rusqlite::Result<SessionGoal> {
    let declined: String = row.get("declined")?;
    Ok(SessionGoal {
        session_id: row.get("session_id")?,
        condition: row.get("condition")?,
        source: row.get("source")?,
        status: row.get("status")?,
        turns: row.get("turns")?,
        last_reason: row.get("last_reason")?,
        declined: serde_json::from_str(&declined).unwrap_or_default(),
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

impl Store {
    pub fn get_session_goal(&self, session_id: &str) -> Result<Option<SessionGoal>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM session_goals WHERE session_id = ?1",
            params![session_id],
            row_to_goal,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Set the session's goal, replacing any earlier one: the check count and
    /// the last reason start over; the declined list is kept.
    pub fn put_session_goal(
        &self,
        session_id: &str,
        condition: &str,
        source: &str,
        status: &str,
    ) -> Result<SessionGoal, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO session_goals (session_id, condition, source, status, turns, last_reason)
             VALUES (?1, ?2, ?3, ?4, 0, NULL)
             ON CONFLICT(session_id) DO UPDATE SET
                condition = excluded.condition, source = excluded.source,
                status = excluded.status, turns = 0, last_reason = NULL,
                updated_at = unixepoch()
             RETURNING *",
            params![session_id, condition, source, status],
            row_to_goal,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Record the goal's new status; `last_reason` replaces the stored one
    /// when given, and `bump_turns` counts one more unmet check.
    pub fn update_session_goal_status(
        &self,
        session_id: &str,
        status: &str,
        last_reason: Option<&str>,
        bump_turns: bool,
    ) -> Result<Option<SessionGoal>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "UPDATE session_goals SET status = ?2,
                last_reason = COALESCE(?3, last_reason),
                turns = turns + ?4, updated_at = unixepoch()
             WHERE session_id = ?1 RETURNING *",
            params![session_id, status, last_reason, bump_turns as i64],
            row_to_goal,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Remember a condition the owner declined, whether or not the session
    /// has a goal.
    pub fn add_session_goal_decline(
        &self,
        session_id: &str,
        condition: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO session_goals (session_id, declined) VALUES (?1, json_array(?2))
             ON CONFLICT(session_id) DO UPDATE SET
                declined = json_insert(declined, '$[#]', ?2), updated_at = unixepoch()",
            params![session_id, condition],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Forget the session's goal and its declines (a new conversation).
    pub fn delete_session_goal(&self, session_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM session_goals WHERE session_id = ?1",
            params![session_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    fn store_with_session(id: &str) -> Store {
        let path = std::env::temp_dir().join(format!("nebo-goals-{}.db", uuid::Uuid::new_v4()));
        let store = Store::new(&path.to_string_lossy()).expect("store");
        store
            .create_session(id, Some("agent:a:web"), None, None, None)
            .expect("session");
        store
    }

    #[test]
    fn a_goal_is_set_checked_replaced_and_keeps_its_declines() {
        let store = store_with_session("s1");
        assert!(store.get_session_goal("s1").unwrap().is_none());

        store
            .add_session_goal_decline("s1", "ship the site")
            .unwrap();
        let g = store
            .put_session_goal("s1", "all tests pass", "owner_command", "active")
            .unwrap();
        assert_eq!(
            (g.condition.as_str(), g.status.as_str(), g.turns),
            ("all tests pass", "active", 0)
        );
        assert_eq!(g.declined, vec!["ship the site".to_string()]);

        let g = store
            .update_session_goal_status("s1", "active", Some("\"2 failing\""), true)
            .unwrap()
            .unwrap();
        assert_eq!(
            (g.turns, g.last_reason.as_deref()),
            (1, Some("\"2 failing\""))
        );
        let g = store
            .update_session_goal_status("s1", "cleared", None, false)
            .unwrap()
            .unwrap();
        assert_eq!(
            (g.turns, g.last_reason.as_deref()),
            (1, Some("\"2 failing\"")),
            "a status change keeps the reason"
        );

        store
            .add_session_goal_decline("s1", "and deploy it")
            .unwrap();
        let g = store
            .put_session_goal("s1", "the report is sent", "owner_command", "active")
            .unwrap();
        assert_eq!(
            (g.turns, g.last_reason),
            (0, None),
            "a new goal starts its count over"
        );
        assert_eq!(
            g.declined,
            vec!["ship the site".to_string(), "and deploy it".to_string()]
        );

        assert!(
            store
                .update_session_goal_status("nope", "met", None, false)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_deleted_session_takes_its_goal() {
        let store = store_with_session("s2");
        store
            .put_session_goal("s2", "done", "owner_command", "active")
            .unwrap();
        store.delete_session("s2").unwrap();
        assert!(store.get_session_goal("s2").unwrap().is_none());

        let store = store_with_session("s3");
        store
            .put_session_goal("s3", "done", "owner_command", "active")
            .unwrap();
        store.delete_session_goal("s3").unwrap();
        assert!(store.get_session_goal("s3").unwrap().is_none());
    }
}
