//! `schedule_follow_ups`: the Inbox pointer a one-shot owes the owner once
//! its request is in his conversation, kept until the turn that takes the
//! request in ends (migration 0255).

use rusqlite::params;

use crate::Store;
use types::NeboError;

/// One request's follow-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleFollowUp {
    /// The hidden request row in the owner's conversation.
    pub request_id: String,
    pub session_key: String,
    pub chat_id: String,
    pub job_id: i64,
    pub job_name: String,
    pub agent_id: String,
    /// When the request was written.
    pub asked_at: i64,
    /// pending | asked_again | pointed | unanswered.
    pub state: String,
    pub asked_again_at: Option<i64>,
    pub answer_id: Option<String>,
    pub created_at: i64,
}

/// What a follow-up is kept from.
pub struct NewScheduleFollowUp<'a> {
    pub request_id: &'a str,
    pub session_key: &'a str,
    pub chat_id: &'a str,
    pub job_id: i64,
    pub job_name: &'a str,
    pub agent_id: &'a str,
    pub asked_at: i64,
}

const COLUMNS: &str = "request_id, session_key, chat_id, job_id, job_name, agent_id, asked_at, state, asked_again_at, answer_id, created_at";

fn row_to_follow_up(r: &rusqlite::Row) -> rusqlite::Result<ScheduleFollowUp> {
    Ok(ScheduleFollowUp {
        request_id: r.get(0)?,
        session_key: r.get(1)?,
        chat_id: r.get(2)?,
        job_id: r.get(3)?,
        job_name: r.get(4)?,
        agent_id: r.get(5)?,
        asked_at: r.get(6)?,
        state: r.get(7)?,
        asked_again_at: r.get(8)?,
        answer_id: r.get(9)?,
        created_at: r.get(10)?,
    })
}

impl Store {
    /// The follow-up for a request, kept once: a second call returns the
    /// row as it is.
    pub fn keep_schedule_follow_up(&self, f: &NewScheduleFollowUp<'_>) -> Result<ScheduleFollowUp, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR IGNORE INTO schedule_follow_ups (request_id, session_key, chat_id, job_id, job_name, agent_id, asked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![f.request_id, f.session_key, f.chat_id, f.job_id, f.job_name, f.agent_id, f.asked_at],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        conn.query_row(&format!("SELECT {COLUMNS} FROM schedule_follow_ups WHERE request_id = ?1"), params![f.request_id], row_to_follow_up)
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// The follow-ups not yet settled (pending or asked again), oldest
    /// first; only `session_key`'s when given.
    pub fn open_schedule_follow_ups(&self, session_key: Option<&str>) -> Result<Vec<ScheduleFollowUp>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {COLUMNS} FROM schedule_follow_ups
                 WHERE state IN ('pending', 'asked_again') AND (?1 IS NULL OR session_key = ?1)
                 ORDER BY created_at, rowid"
            ))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![session_key], row_to_follow_up)
            .map_err(|e| NeboError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(rows)
    }

    /// Mark a pending follow-up asked again (`at`). True for the one call
    /// that moved it; false when it was already asked again or settled.
    pub fn ask_schedule_follow_up_again(&self, request_id: &str, at: i64) -> Result<bool, NeboError> {
        let n = self
            .conn()?
            .execute(
                "UPDATE schedule_follow_ups SET state = 'asked_again', asked_again_at = ?2 WHERE request_id = ?1 AND state = 'pending'",
                params![request_id, at],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(n == 1)
    }

    /// Settle an open follow-up: `answer_id` Some = pointed at it, None =
    /// unanswered. True for the one call that settled it.
    pub fn settle_schedule_follow_up(&self, request_id: &str, answer_id: Option<&str>, at: i64) -> Result<bool, NeboError> {
        let n = self
            .conn()?
            .execute(
                "UPDATE schedule_follow_ups SET state = CASE WHEN ?2 IS NULL THEN 'unanswered' ELSE 'pointed' END, answer_id = ?2, settled_at = ?3
                 WHERE request_id = ?1 AND state IN ('pending', 'asked_again')",
                params![request_id, answer_id, at],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(n == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_follow_up_is_kept_once_asked_again_once_and_settled_once() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(&dir.path().join("f.db").to_string_lossy()).unwrap();
        let new = NewScheduleFollowUp { request_id: "r1", session_key: "agent:a:web", chat_id: "c", job_id: 7, job_name: "oven", agent_id: "a", asked_at: 100 };
        let kept = s.keep_schedule_follow_up(&new).unwrap();
        assert_eq!((kept.state.as_str(), kept.job_name.as_str()), ("pending", "oven"));
        assert_eq!(s.keep_schedule_follow_up(&new).unwrap(), kept, "kept once");
        assert_eq!(s.open_schedule_follow_ups(Some("agent:a:web")).unwrap().len(), 1);
        assert!(s.open_schedule_follow_ups(Some("other")).unwrap().is_empty());
        assert!(s.ask_schedule_follow_up_again("r1", 200).unwrap());
        assert!(!s.ask_schedule_follow_up_again("r1", 201).unwrap(), "asked again once");
        assert!(s.settle_schedule_follow_up("r1", Some("m9"), 300).unwrap());
        assert!(!s.settle_schedule_follow_up("r1", None, 301).unwrap(), "settled once");
        assert!(s.open_schedule_follow_ups(None).unwrap().is_empty());
        assert_eq!(s.keep_schedule_follow_up(&new).unwrap().answer_id.as_deref(), Some("m9"));
    }
}
