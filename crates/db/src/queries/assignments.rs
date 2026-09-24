//! Assignments (R5): work handed from one seat to another as the assignee's
//! own work. The row is the record; the assignee's case (key
//! `case:assignment:<id>`) is the work; closing the case settles the row and
//! tells the assigner.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::Store;
use types::NeboError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assignment {
    pub id: String,
    pub assigner_agent_id: String,
    pub assigner_session_key: String,
    pub assignee_agent_id: String,
    pub subject: String,
    pub done_means: String,
    pub due: Option<String>,
    /// open | done | blocked | failed
    pub state: String,
    pub outcome: Option<String>,
    pub parent_run_id: Option<String>,
    pub case_key: String,
    pub created_at: i64,
    pub closed_at: Option<i64>,
}

pub struct NewAssignment<'a> {
    pub id: &'a str,
    pub assigner_agent_id: &'a str,
    pub assigner_session_key: &'a str,
    pub assignee_agent_id: &'a str,
    pub subject: &'a str,
    pub done_means: &'a str,
    pub due: Option<&'a str>,
    pub parent_run_id: Option<&'a str>,
    pub case_key: &'a str,
}

/// Characters of a standing outcome kept on its binding.
pub const STANDING_OUTCOME_CAP: usize = 300;

const COLS: &str = "id, assigner_agent_id, assigner_session_key, assignee_agent_id, subject, done_means, due, state, outcome, parent_run_id, case_key, created_at, closed_at";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Assignment> {
    Ok(Assignment {
        id: r.get(0)?,
        assigner_agent_id: r.get(1)?,
        assigner_session_key: r.get(2)?,
        assignee_agent_id: r.get(3)?,
        subject: r.get(4)?,
        done_means: r.get(5)?,
        due: r.get(6)?,
        state: r.get(7)?,
        outcome: r.get(8)?,
        parent_run_id: r.get(9)?,
        case_key: r.get(10)?,
        created_at: r.get(11)?,
        closed_at: r.get(12)?,
    })
}

impl Store {
    pub fn create_assignment(&self, a: &NewAssignment<'_>) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO assignments (id, assigner_agent_id, assigner_session_key, assignee_agent_id, subject, done_means, due, parent_run_id, case_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![a.id, a.assigner_agent_id, a.assigner_session_key, a.assignee_agent_id, a.subject, a.done_means, a.due, a.parent_run_id, a.case_key],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn get_assignment(&self, id: &str) -> Result<Option<Assignment>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!("SELECT {COLS} FROM assignments WHERE id = ?1"))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let mut rows = stmt.query_map(params![id], row).map_err(|e| NeboError::Database(e.to_string()))?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(|e| NeboError::Database(e.to_string()))?)),
            None => Ok(None),
        }
    }

    /// Close an assignment. Returns false when it was already closed, so a
    /// case settled twice tells the assigner once.
    pub fn close_assignment(&self, id: &str, state: &str, outcome: Option<&str>, now: i64) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let n = conn
            .execute(
                "UPDATE assignments SET state = ?2, outcome = ?3, closed_at = ?4 WHERE id = ?1 AND state = 'open'",
                params![id, state, outcome, now],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(n > 0)
    }

    /// A seat's assignments, as assigner or assignee. `open_only` hides the closed ones.
    pub fn list_assignments_for_agent(&self, agent_id: &str, open_only: bool) -> Result<Vec<Assignment>, NeboError> {
        let conn = self.conn()?;
        let sql = format!(
            "SELECT {COLS} FROM assignments WHERE (assigner_agent_id = ?1 OR assignee_agent_id = ?1){} ORDER BY created_at DESC LIMIT 200",
            if open_only { " AND state = 'open'" } else { "" }
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt.query_map(params![agent_id], row).map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| NeboError::Database(e.to_string()))
    }

    /// R7: a binding that cannot run says why, instead of vanishing. Empty
    /// reason clears it.
    pub fn set_agent_workflow_degraded_reason(&self, agent_id: &str, binding_name: &str, reason: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let value: Option<&str> = if reason.is_empty() { None } else { Some(reason) };
        conn.execute(
            "UPDATE agent_workflows SET degraded_reason = ?3 WHERE agent_id = ?1 AND binding_name = ?2",
            params![agent_id, binding_name, value],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn agent_workflow_degraded_reason(&self, agent_id: &str, binding_name: &str) -> Result<Option<String>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT degraded_reason FROM agent_workflows WHERE agent_id = ?1 AND binding_name = ?2",
            params![agent_id, binding_name],
            |r| r.get::<_, Option<String>>(0),
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// A binding's run ended with a standing outcome (the step evaluator or
    /// the employee said there is nothing to do, and why): record the reason
    /// on the binding the run belongs to. A run of no binding (a standalone
    /// workflow) records nothing. The reason is kept as its first line,
    /// capped at [`STANDING_OUTCOME_CAP`] characters.
    pub fn record_standing_outcome(&self, run_id: &str, reason: &str, at: i64) -> Result<(), NeboError> {
        let line = reason.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
        let outcome: String = line.chars().take(STANDING_OUTCOME_CAP).collect();
        let conn = self.conn()?;
        // A binding's runs carry `agent:<id>` as their workflow id and the
        // binding name (or `<binding>:<detail>`) as their trigger detail.
        conn.execute(
            "UPDATE agent_workflows SET last_outcome = ?2, last_outcome_at = ?3
             WHERE id = (
                 SELECT aw.id FROM workflow_runs w JOIN agent_workflows aw
                   ON w.workflow_id = 'agent:' || aw.agent_id
                  AND (w.trigger_detail = aw.binding_name OR w.trigger_detail LIKE aw.binding_name || ':%')
                 WHERE w.id = ?1
                 ORDER BY length(aw.binding_name) DESC LIMIT 1)",
            params![run_id, outcome, at],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// A binding's last standing outcome and when it was recorded.
    pub fn agent_workflow_last_outcome(&self, agent_id: &str, binding_name: &str) -> Result<Option<(String, i64)>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT last_outcome, last_outcome_at FROM agent_workflows WHERE agent_id = ?1 AND binding_name = ?2",
            params![agent_id, binding_name],
            |r| Ok(r.get::<_, Option<String>>(0)?.zip(r.get::<_, Option<i64>>(1)?)),
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }
}
