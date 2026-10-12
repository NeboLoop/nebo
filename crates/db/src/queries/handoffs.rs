//! Hand-offs (migration 0208): the trace of work one employee passes to
//! another — who sent what to whom, whether it is still going, and what came
//! back. Written by the two doors work crosses between employees (a message
//! on the coworker rail, an assignment); kept after it ends.

use rusqlite::params;
use serde::Serialize;

use crate::{DbErrExt, Store};
use types::NeboError;

/// Longest ask a row keeps (characters); the whole request is in the
/// receiving conversation.
pub const HANDOFF_ASK_CAP: usize = 1000;
/// Longest result or error a row keeps (characters); the whole answer is in
/// the receiving conversation and reached the sender whole.
pub const HANDOFF_RESULT_CAP: usize = 2000;

/// Statuses a hand-off still going is in.
pub const HANDOFF_LIVE: [&str; 2] = ["queued", "running"];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub id: String,
    /// The hand-off the sending conversation was itself working on.
    pub parent_id: Option<String>,
    /// `message` | `assignment`
    pub kind: String,
    /// The employee that handed the work on ("" = the main employee).
    pub from_agent_id: String,
    pub to_agent_id: String,
    /// The team a team post's hand-off is in; "" otherwise.
    pub team_id: String,
    pub sender_session: String,
    pub sender_run_id: Option<String>,
    pub receiver_session: String,
    pub receiver_run_id: Option<String>,
    pub ask: String,
    /// queued | running | done | failed | stopped
    pub status: String,
    pub result: String,
    pub error: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

/// A hand-off as it starts.
pub struct NewHandoff<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub from_agent_id: &'a str,
    pub to_agent_id: &'a str,
    pub team_id: &'a str,
    pub sender_session: &'a str,
    pub sender_run_id: Option<&'a str>,
    pub receiver_session: &'a str,
    pub receiver_run_id: Option<&'a str>,
    pub ask: &'a str,
    /// `queued` or `running`.
    pub status: &'a str,
}

/// Which hand-offs [`Store::list_handoffs`] reads. Every field narrows; the
/// default reads every one.
#[derive(Debug, Clone, Default)]
pub struct HandoffQuery<'a> {
    /// Handed on from this conversation.
    pub from_session: Option<&'a str>,
    /// Worked in this conversation.
    pub into_session: Option<&'a str>,
    /// Sent or received by this employee.
    pub agent_id: Option<&'a str>,
    /// Children of this hand-off.
    pub parent_id: Option<&'a str>,
    /// Only those still going.
    pub live: bool,
    /// At most this many, newest first (0 = no limit).
    pub limit: usize,
}

const COLUMNS: &str = "id, parent_id, kind, from_agent_id, to_agent_id, team_id, sender_session, sender_run_id, \
     receiver_session, receiver_run_id, ask, status, result, error, created_at, started_at, finished_at";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Handoff> {
    Ok(Handoff {
        id: r.get(0)?,
        parent_id: r.get(1)?,
        kind: r.get(2)?,
        from_agent_id: r.get(3)?,
        to_agent_id: r.get(4)?,
        team_id: r.get(5)?,
        sender_session: r.get(6)?,
        sender_run_id: r.get(7)?,
        receiver_session: r.get(8)?,
        receiver_run_id: r.get(9)?,
        ask: r.get(10)?,
        status: r.get(11)?,
        result: r.get(12)?,
        error: r.get(13)?,
        created_at: r.get(14)?,
        started_at: r.get(15)?,
        finished_at: r.get(16)?,
    })
}

fn cap(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => s[..at].to_string(),
        None => s.to_string(),
    }
}

fn is_final(status: &str) -> bool {
    matches!(status, "done" | "failed" | "stopped")
}

impl Store {
    /// Record a hand-off as it starts. Its parent is the hand-off the
    /// sending conversation was itself working on (the newest one into it),
    /// so a chain reads as one tree.
    pub fn create_handoff(&self, h: &NewHandoff<'_>, now: i64) -> Result<Handoff, NeboError> {
        let started = (h.status == "running").then_some(now);
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO handoffs (id, parent_id, kind, from_agent_id, to_agent_id, team_id, sender_session, sender_run_id,
                 receiver_session, receiver_run_id, ask, status, created_at, started_at)
             VALUES (?1,
                 (SELECT p.id FROM handoffs p WHERE p.receiver_session = ?6 AND ?6 != '' ORDER BY p.created_at DESC, p.rowid DESC LIMIT 1),
                 ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                h.id,
                h.kind,
                h.from_agent_id,
                h.to_agent_id,
                h.team_id,
                h.sender_session,
                h.sender_run_id,
                h.receiver_session,
                h.receiver_run_id,
                cap(h.ask, HANDOFF_ASK_CAP),
                h.status,
                now,
                started,
            ],
        )
        .db_err("create_handoff")?;
        conn.query_row(&format!("SELECT {COLUMNS} FROM handoffs WHERE id = ?1"), params![h.id], row)
            .db_err("create_handoff read")
    }

    pub fn get_handoff(&self, id: &str) -> Result<Option<Handoff>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(&format!("SELECT {COLUMNS} FROM handoffs WHERE id = ?1"), params![id], row) {
            Ok(h) => Ok(Some(h)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e).db_err("get_handoff"),
        }
    }

    /// A queued hand-off's work has started.
    pub fn start_handoff(&self, id: &str, now: i64) -> Result<Option<Handoff>, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE handoffs SET status = 'running', started_at = COALESCE(started_at, ?2) WHERE id = ?1 AND status = 'queued'",
            params![id, now],
        )
        .db_err("start_handoff")?;
        drop(conn);
        self.get_handoff(id)
    }

    /// End one hand-off still going with `status` (done | failed | stopped).
    /// Returns the row when this call ended it; `None` when it had ended
    /// already, so an end is recorded once.
    pub fn finish_handoff(&self, id: &str, status: &str, result: &str, error: &str, now: i64) -> Result<Option<Handoff>, NeboError> {
        if !is_final(status) {
            return Err(NeboError::Internal(format!("not a final hand-off status: {status}")));
        }
        let n = self
            .conn()?
            .execute(
                "UPDATE handoffs SET status = ?2, result = ?3, error = ?4, finished_at = ?5
                 WHERE id = ?1 AND status IN ('queued', 'running')",
                params![id, status, cap(result, HANDOFF_RESULT_CAP), cap(error, HANDOFF_RESULT_CAP), now],
            )
            .db_err("finish_handoff")?;
        if n == 0 {
            return Ok(None);
        }
        self.get_handoff(id)
    }

    /// End every hand-off still going in conversation `receiver_session`
    /// with `status`. Returns the rows this call ended.
    pub fn finish_handoffs_into(
        &self,
        receiver_session: &str,
        status: &str,
        result: &str,
        error: &str,
        now: i64,
    ) -> Result<Vec<Handoff>, NeboError> {
        let live = self.list_handoffs(&HandoffQuery { into_session: Some(receiver_session), live: true, ..Default::default() })?;
        let mut ended = Vec::new();
        for h in live {
            if let Some(h) = self.finish_handoff(&h.id, status, result, error, now)? {
                ended.push(h);
            }
        }
        Ok(ended)
    }

    /// The hand-offs `q` names, newest first.
    pub fn list_handoffs(&self, q: &HandoffQuery<'_>) -> Result<Vec<Handoff>, NeboError> {
        let mut filters: Vec<String> = Vec::new();
        let mut args: Vec<String> = Vec::new();
        let arg = |v: &str, args: &mut Vec<String>| -> usize {
            args.push(v.to_string());
            args.len()
        };
        if let Some(s) = q.from_session {
            let n = arg(s, &mut args);
            filters.push(format!("sender_session = ?{n}"));
        }
        if let Some(s) = q.into_session {
            let n = arg(s, &mut args);
            filters.push(format!("receiver_session = ?{n}"));
        }
        if let Some(a) = q.agent_id {
            let n = arg(a, &mut args);
            filters.push(format!("(from_agent_id = ?{n} OR to_agent_id = ?{n})"));
        }
        if let Some(p) = q.parent_id {
            let n = arg(p, &mut args);
            filters.push(format!("parent_id = ?{n}"));
        }
        if q.live {
            filters.push(format!("status IN ('{}', '{}')", HANDOFF_LIVE[0], HANDOFF_LIVE[1]));
        }
        let filter = if filters.is_empty() { String::new() } else { format!("WHERE {}", filters.join(" AND ")) };
        let limit = if q.limit == 0 { String::new() } else { format!("LIMIT {}", q.limit) };
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!("SELECT {COLUMNS} FROM handoffs {filter} ORDER BY created_at DESC, rowid DESC {limit}"))
            .db_err("list_handoffs")?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), row)
            .db_err("list_handoffs")?;
        rows.collect::<Result<Vec<_>, _>>().db_err("list_handoffs")
    }

    /// Message hand-offs made before `booted` and still going: their runs
    /// died with the process that ran them, so they can never end on their
    /// own. Each is ended as stopped. (An assignment is a durable case and
    /// survives a restart.) Returns how many.
    pub fn stop_orphaned_handoffs(&self, booted: i64, now: i64) -> Result<usize, NeboError> {
        self.conn()?
            .execute(
                "UPDATE handoffs SET status = 'stopped', finished_at = ?2
                 WHERE kind = 'message' AND status IN ('queued', 'running') AND created_at < ?1",
                params![booted, now],
            )
            .db_err("stop_orphaned_handoffs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-handoffs-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).unwrap()
    }

    fn new<'a>(id: &'a str, from: &'a str, to: &'a str, sender: &'a str, receiver: &'a str) -> NewHandoff<'a> {
        NewHandoff {
            id,
            kind: "message",
            from_agent_id: from,
            to_agent_id: to,
            team_id: "",
            sender_session: sender,
            sender_run_id: Some("run-1"),
            receiver_session: receiver,
            receiver_run_id: None,
            ask: "Pull last month's invoices",
            status: "queued",
        }
    }

    /// A → B → C: the hand-off B makes while working A's is nested under it.
    #[test]
    fn a_chain_nests_under_the_handoff_its_sender_was_working() {
        let s = store();
        let ab = s.create_handoff(&new("h1", "a", "b", "agent:a:web", "agent:b:coworker:a"), 1).unwrap();
        assert_eq!(ab.parent_id, None);
        let bc = s.create_handoff(&new("h2", "b", "c", "agent:b:coworker:a", "agent:c:coworker:b"), 2).unwrap();
        assert_eq!(bc.parent_id.as_deref(), Some("h1"));
        let children = s.list_handoffs(&HandoffQuery { parent_id: Some("h1"), ..Default::default() }).unwrap();
        assert_eq!(children.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["h2"]);
    }

    /// queued → running → done, once; a later end changes nothing.
    #[test]
    fn a_handoff_ends_once() {
        let s = store();
        s.create_handoff(&new("h1", "a", "b", "agent:a:web", "agent:b:coworker:a"), 1).unwrap();
        let running = s.start_handoff("h1", 2).unwrap().unwrap();
        assert_eq!((running.status.as_str(), running.started_at), ("running", Some(2)));
        let ended = s.finish_handoffs_into("agent:b:coworker:a", "done", "Found 12 invoices.", "", 3).unwrap();
        assert_eq!(ended.len(), 1);
        assert_eq!((ended[0].status.as_str(), ended[0].result.as_str(), ended[0].finished_at), ("done", "Found 12 invoices.", Some(3)));
        assert!(s.finish_handoff("h1", "failed", "", "late", 4).unwrap().is_none(), "ended already");
        assert_eq!(s.get_handoff("h1").unwrap().unwrap().status, "done");
        assert!(s.finish_handoff("h1", "running", "", "", 5).is_err(), "only a final status ends one");
    }

    /// Failures and stops stay on record and leave the live list.
    #[test]
    fn failed_and_stopped_handoffs_stay_on_record() {
        let s = store();
        s.create_handoff(&new("h1", "a", "b", "agent:a:web", "agent:b:coworker:a"), 1).unwrap();
        s.create_handoff(&new("h2", "a", "c", "agent:a:web", "agent:c:coworker:a"), 2).unwrap();
        s.finish_handoff("h1", "failed", "", "Could not connect to Hermes.", 3).unwrap();
        assert_eq!(s.stop_orphaned_handoffs(2, 4).unwrap(), 0, "made at boot: not an orphan");
        assert_eq!(s.stop_orphaned_handoffs(3, 4).unwrap(), 1);
        let all = s.list_handoffs(&HandoffQuery { from_session: Some("agent:a:web"), ..Default::default() }).unwrap();
        assert_eq!(all.iter().map(|h| (h.id.as_str(), h.status.as_str())).collect::<Vec<_>>(), vec![("h2", "stopped"), ("h1", "failed")]);
        assert!(s.list_handoffs(&HandoffQuery { live: true, ..Default::default() }).unwrap().is_empty());
        let by_agent = s.list_handoffs(&HandoffQuery { agent_id: Some("c"), ..Default::default() }).unwrap();
        assert_eq!(by_agent.len(), 1);
    }

    /// An ask past the cap is kept to it, on a character boundary.
    #[test]
    fn a_long_ask_is_capped() {
        let s = store();
        let long = "é".repeat(HANDOFF_ASK_CAP + 50);
        let mut h = new("h1", "a", "b", "agent:a:web", "agent:b:coworker:a");
        h.ask = &long;
        let row = s.create_handoff(&h, 1).unwrap();
        assert_eq!(row.ask.chars().count(), HANDOFF_ASK_CAP);
    }
}
