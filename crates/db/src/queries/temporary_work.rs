//! Temporary work: the lifetime option of a workflow or a team made for one
//! piece of work (`temporary_work`, migration 0182). A temporary thing runs
//! once; once that run has ended and its outcome has reached the owner, the
//! thing is deleted. This module only reads and writes the rows; the server
//! finishes the work.

use rusqlite::params;

use super::engine::EngineRun;
use crate::{DbErrExt, OptionalExt, Store};
use types::NeboError;

/// What a piece of temporary work is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporaryKind {
    Workflow,
    Team,
}

impl TemporaryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TemporaryKind::Workflow => "workflow",
            TemporaryKind::Team => "team",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "workflow" => Some(TemporaryKind::Workflow),
            "team" => Some(TemporaryKind::Team),
            _ => None,
        }
    }
}

/// One piece of temporary work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporaryWork {
    pub kind: TemporaryKind,
    /// The employee whose workflow it is; empty for a team.
    pub agent_id: String,
    /// The workflow's binding name, or the team's id.
    pub name: String,
    /// The session that started it, woken with the outcome.
    pub report_to: String,
    /// The one run it started, once it has.
    pub run_id: Option<String>,
    pub created_at: i64,
}

/// Whether a run may start for a piece of work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemporaryClaim {
    /// Saved work: it runs as often as its trigger fires.
    NotTemporary,
    /// Temporary work, and this is its one run.
    Claimed,
    /// Temporary work that already started its one run.
    AlreadyRan(String),
}

const COLUMNS: &str = "kind, agent_id, name, report_to, run_id, created_at";

fn row_to_work(r: &rusqlite::Row<'_>) -> rusqlite::Result<TemporaryWork> {
    let kind: String = r.get(0)?;
    Ok(TemporaryWork {
        kind: TemporaryKind::parse(&kind).unwrap_or(TemporaryKind::Workflow),
        agent_id: r.get(1)?,
        name: r.get(2)?,
        report_to: r.get(3)?,
        run_id: r.get(4)?,
        created_at: r.get(5)?,
    })
}

impl Store {
    /// Make a workflow or a team temporary, reporting to `report_to`. A
    /// second call keeps the run it already started.
    pub fn mark_temporary(&self, kind: TemporaryKind, agent_id: &str, name: &str, report_to: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO temporary_work (kind, agent_id, name, report_to) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(kind, agent_id, name) DO UPDATE SET report_to = excluded.report_to",
            params![kind.as_str(), agent_id, name, report_to],
        )
        .db_err("mark_temporary")?;
        Ok(())
    }

    /// Saved from now on (promoted, or deleted): no longer temporary.
    pub fn unmark_temporary(&self, kind: TemporaryKind, agent_id: &str, name: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM temporary_work WHERE kind = ?1 AND agent_id = ?2 AND name = ?3",
            params![kind.as_str(), agent_id, name],
        )
        .db_err("unmark_temporary")?;
        Ok(())
    }

    pub fn temporary_work(&self, kind: TemporaryKind, agent_id: &str, name: &str) -> Result<Option<TemporaryWork>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM temporary_work WHERE kind = ?1 AND agent_id = ?2 AND name = ?3"),
            params![kind.as_str(), agent_id, name],
            row_to_work,
        )
        .optional()
        .db_err("temporary_work")
    }

    /// Every piece of temporary work, oldest first.
    pub fn list_temporary_work(&self) -> Result<Vec<TemporaryWork>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!("SELECT {COLUMNS} FROM temporary_work ORDER BY created_at, name"))
            .db_err("list_temporary_work")?;
        let rows = stmt.query_map([], row_to_work).db_err("list_temporary_work")?;
        rows.collect::<Result<Vec<_>, _>>().db_err("list_temporary_work")
    }

    /// A run is starting for this work: temporary work gets its one run
    /// here, atomically, and never a second.
    pub fn claim_temporary_run(&self, kind: TemporaryKind, agent_id: &str, name: &str, run_id: &str) -> Result<TemporaryClaim, NeboError> {
        let conn = self.conn()?;
        let claimed = conn
            .execute(
                "UPDATE temporary_work SET run_id = ?4 WHERE kind = ?1 AND agent_id = ?2 AND name = ?3 AND run_id IS NULL",
                params![kind.as_str(), agent_id, name, run_id],
            )
            .db_err("claim_temporary_run")?;
        if claimed == 1 {
            return Ok(TemporaryClaim::Claimed);
        }
        let ran: Option<Option<String>> = conn
            .query_row(
                "SELECT run_id FROM temporary_work WHERE kind = ?1 AND agent_id = ?2 AND name = ?3",
                params![kind.as_str(), agent_id, name],
                |r| r.get(0),
            )
            .optional()
            .db_err("claim_temporary_run read")?;
        Ok(match ran {
            None => TemporaryClaim::NotTemporary,
            Some(run) => TemporaryClaim::AlreadyRan(run.unwrap_or_default()),
        })
    }

    /// A claimed run that never started (its record could not be written):
    /// the work may claim its one run again.
    pub fn release_temporary_run(&self, kind: TemporaryKind, agent_id: &str, name: &str, run_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE temporary_work SET run_id = NULL WHERE kind = ?1 AND agent_id = ?2 AND name = ?3 AND run_id = ?4",
            params![kind.as_str(), agent_id, name, run_id],
        )
        .db_err("release_temporary_run")?;
        Ok(())
    }

    /// Temporary work whose one run has ended (done, failed or cancelled),
    /// with that run: the work to finish.
    pub fn ended_temporary_work(&self) -> Result<Vec<(TemporaryWork, EngineRun)>, NeboError> {
        let work: Vec<TemporaryWork> = {
            let conn = self.conn()?;
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {} FROM temporary_work t JOIN engine_runs r ON r.id = t.run_id
                     WHERE r.state IN ('done', 'failed', 'cancelled') ORDER BY t.created_at",
                    COLUMNS.split(", ").map(|c| format!("t.{c}")).collect::<Vec<_>>().join(", ")
                ))
                .db_err("ended_temporary_work")?;
            let rows = stmt.query_map([], row_to_work).db_err("ended_temporary_work")?;
            rows.collect::<Result<Vec<_>, _>>().db_err("ended_temporary_work")?
        };
        let mut out = Vec::with_capacity(work.len());
        for w in work {
            if let Some(run) = self.engine_get_run(w.run_id.as_deref().unwrap_or_default())? {
                out.push((w, run));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-temp-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    /// Temporary work runs once: the first run claims it, a second never
    /// starts; saved work is not held back. Once its run ends, it is listed
    /// to finish.
    #[test]
    fn temporary_work_runs_once_and_is_finished_when_its_run_ends() {
        let s = store();
        assert_eq!(s.claim_temporary_run(TemporaryKind::Workflow, "ava", "report", "r0").unwrap(), TemporaryClaim::NotTemporary);
        s.mark_temporary(TemporaryKind::Workflow, "ava", "report", "agent:ava:web").unwrap();
        s.engine_create_run(&crate::NewRun { id: "r1", kind: "workflow", session_key: "k", agent_id: "ava", lane: "main", ..Default::default() }).unwrap();
        assert_eq!(s.claim_temporary_run(TemporaryKind::Workflow, "ava", "report", "r1").unwrap(), TemporaryClaim::Claimed);
        assert_eq!(s.claim_temporary_run(TemporaryKind::Workflow, "ava", "report", "r2").unwrap(), TemporaryClaim::AlreadyRan("r1".into()));
        assert!(s.ended_temporary_work().unwrap().is_empty(), "still running");
        s.engine_set_run_state("r1", "done", 10, None).unwrap();
        let ended = s.ended_temporary_work().unwrap();
        assert_eq!(ended.len(), 1);
        assert_eq!((ended[0].0.name.as_str(), ended[0].1.id.as_str(), ended[0].0.report_to.as_str()), ("report", "r1", "agent:ava:web"));
        s.unmark_temporary(TemporaryKind::Workflow, "ava", "report").unwrap();
        assert!(s.temporary_work(TemporaryKind::Workflow, "ava", "report").unwrap().is_none());
    }
}
