//! The workflow audit's records (`napp::workflow_check::audit_workflow`):
//! the last audit of each employee workflow, the tools picked for
//! open-ended steps, and the tools each step's runs called.

use std::collections::{BTreeSet, HashMap};

use rusqlite::params;

use crate::Store;
use crate::{DbErrExt, OptionalExt};
use types::NeboError;

/// The last audit of one employee workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowAuditRow {
    pub agent_id: String,
    pub binding_name: String,
    /// The hash of the definition the audit left; '' when stale.
    pub definition_hash: String,
    /// What the audit added, fixed and still finds wrong (JSON).
    pub report: String,
    /// Each tool a step called without declaring it (JSON array).
    pub healed: String,
    pub audited_at: i64,
}

fn row_to_audit(row: &rusqlite::Row) -> rusqlite::Result<WorkflowAuditRow> {
    Ok(WorkflowAuditRow {
        agent_id: row.get(0)?,
        binding_name: row.get(1)?,
        definition_hash: row.get(2)?,
        report: row.get(3)?,
        healed: row.get(4)?,
        audited_at: row.get(5)?,
    })
}

const AUDIT_COLUMNS: &str = "agent_id, binding_name, definition_hash, report, healed, audited_at";

impl Store {
    /// The last audit of an employee's workflow.
    pub fn workflow_audit(&self, agent_id: &str, binding: &str) -> Result<Option<WorkflowAuditRow>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            &format!("SELECT {AUDIT_COLUMNS} FROM workflow_audits WHERE agent_id = ?1 AND binding_name = ?2"),
            params![agent_id, binding],
            row_to_audit,
        )
        .optional()
        .db_err("workflow_audit")
    }

    /// The last audit of each of an employee's workflows.
    pub fn workflow_audits(&self, agent_id: &str) -> Result<Vec<WorkflowAuditRow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!("SELECT {AUDIT_COLUMNS} FROM workflow_audits WHERE agent_id = ?1 ORDER BY binding_name"))
            .db_err("workflow_audits")?;
        let rows = stmt.query_map(params![agent_id], row_to_audit).db_err("workflow_audits")?;
        rows.collect::<Result<Vec<_>, _>>().db_err("workflow_audits")
    }

    /// Keep an audit of an employee's workflow. The steps' undeclared tools
    /// it already holds are kept.
    pub fn save_workflow_audit(
        &self,
        agent_id: &str,
        binding: &str,
        definition_hash: &str,
        report: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO workflow_audits (agent_id, binding_name, definition_hash, report, audited_at)
             VALUES (?1, ?2, ?3, ?4, unixepoch())
             ON CONFLICT(agent_id, binding_name) DO UPDATE SET
                definition_hash = excluded.definition_hash,
                report = excluded.report,
                audited_at = excluded.audited_at",
            params![agent_id, binding, definition_hash, report],
        )
        .db_err("save_workflow_audit")?;
        Ok(())
    }

    /// A step of run `run_id` called `tool` without declaring it, and the
    /// call `outcome` ("ran", or "asked" under the Ask mode). It is kept
    /// once per step, tool and outcome, and the workflow's audit is marked
    /// stale, so the next run's check audits it again and the run history
    /// adds the tool to the step. Returns whether the run is an employee
    /// workflow's.
    pub fn record_step_heal(
        &self,
        run_id: &str,
        activity: &str,
        tool: &str,
        read_only: bool,
        outcome: &str,
    ) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let run: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT workflow_id, trigger_detail FROM workflow_runs WHERE id = ?1",
                params![run_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .db_err("record_step_heal run")?;
        let Some((workflow_id, detail)) = run else { return Ok(false) };
        let Some(agent_id) = types::keyparser::agent_id_from_workflow_id(&workflow_id) else { return Ok(false) };
        let binding = detail.as_deref().map(|d| d.split(':').next().unwrap_or(d)).unwrap_or("");
        if binding.is_empty() {
            return Ok(false);
        }
        let entry = serde_json::json!({
            "step": activity,
            "tool": tool,
            "readOnly": read_only,
            "outcome": outcome,
            "at": chrono::Utc::now().timestamp(),
        });
        conn.execute(
            "INSERT INTO workflow_audits (agent_id, binding_name, healed) VALUES (?1, ?2, json_array(json(?3)))
             ON CONFLICT(agent_id, binding_name) DO UPDATE SET
                definition_hash = '',
                healed = CASE
                    WHEN EXISTS (SELECT 1 FROM json_each(workflow_audits.healed) h
                                 WHERE json_extract(h.value, '$.step') = ?4
                                   AND json_extract(h.value, '$.tool') = ?5
                                   AND json_extract(h.value, '$.outcome') = ?6)
                    THEN workflow_audits.healed
                    ELSE json_insert(workflow_audits.healed, '$[#]', json(?3))
                END",
            params![agent_id, binding, entry.to_string(), activity, tool, outcome],
        )
        .db_err("record_step_heal")?;
        Ok(true)
    }

    /// The tools picked for an open-ended step under `step_key`, when it was
    /// picked before.
    pub fn step_pick(&self, step_key: &str) -> Result<Option<Vec<String>>, NeboError> {
        let conn = self.conn()?;
        let tools: Option<String> = conn
            .query_row("SELECT tools FROM workflow_step_picks WHERE step_key = ?1", params![step_key], |r| r.get(0))
            .optional()
            .db_err("step_pick")?;
        Ok(tools.and_then(|t| serde_json::from_str(&t).ok()))
    }

    /// Keep the tools picked for an open-ended step.
    pub fn save_step_pick(&self, step_key: &str, tools: &[String]) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO workflow_step_picks (step_key, tools, created_at) VALUES (?1, ?2, unixepoch())
             ON CONFLICT(step_key) DO UPDATE SET tools = excluded.tools, created_at = excluded.created_at",
            params![step_key, serde_json::to_string(tools).unwrap_or_else(|_| "[]".into())],
        )
        .db_err("save_step_pick")?;
        Ok(())
    }

    /// Every tool any run on this Nebo has called: the read-only audit's
    /// view of tools it can't start (an MCP server's).
    pub fn called_tools(&self) -> Result<BTreeSet<String>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT DISTINCT tool FROM permission_activity WHERE decision = 'allow'")
            .db_err("called_tools")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).db_err("called_tools")?;
        rows.collect::<Result<BTreeSet<_>, _>>().db_err("called_tools")
    }

    /// What each step of an employee's workflow called in its past runs:
    /// activity id → the tools the permission check let run. A step that
    /// ran and called nothing has no entry.
    pub fn workflow_step_history(&self, agent_id: &str, binding: &str) -> Result<HashMap<String, BTreeSet<String>>, NeboError> {
        let conn = self.conn()?;
        let mut runs = conn
            .prepare("SELECT id, trigger_detail FROM workflow_runs WHERE workflow_id = ?1")
            .db_err("workflow_step_history runs")?;
        let of_binding: BTreeSet<String> = runs
            .query_map(params![types::keyparser::agent_workflow_id(agent_id)], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .db_err("workflow_step_history runs")?
            .filter_map(Result::ok)
            .filter(|(_, detail)| detail.as_deref().map(|d| d.split(':').next().unwrap_or(d)) == Some(binding))
            .map(|(id, _)| id)
            .collect();
        let mut calls = conn
            .prepare(
                "SELECT session_key, tool FROM permission_activity
                 WHERE agent_id = ?1 AND decision = 'allow' AND session_key LIKE '%:workflow:%'",
            )
            .db_err("workflow_step_history calls")?;
        let mut out: HashMap<String, BTreeSet<String>> = HashMap::new();
        for (key, tool) in calls
            .query_map(params![agent_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .db_err("workflow_step_history calls")?
            .filter_map(Result::ok)
        {
            if let Some((run, activity)) = types::keyparser::workflow_step(&key)
                && of_binding.contains(run)
            {
                out.entry(activity.to_string()).or_default().insert(tool);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap();
        (dir, s)
    }

    fn run(s: &Store, id: &str, agent: &str, binding: &str) {
        s.create_workflow_run(id, &format!("agent:{agent}"), "schedule", Some(binding), None, None, Some("{}")).unwrap();
    }

    fn call(s: &Store, agent: &str, run: &str, step: &str, tool: &str, decision: &str) {
        s.record_permission_activity(&crate::PermissionActivityRow {
            agent_id: agent.into(),
            session_key: format!("agent:{agent}:workflow:{run}:{step}::0"),
            door: "workflow".into(),
            tool: tool.into(),
            rule_key: tool.into(),
            activity: String::new(),
            decision: decision.into(),
            why: "{}".into(),
            ask_id: None,
            unreviewed: false,
            judgement: None,
            created_at: 1,
        })
        .unwrap();
    }

    /// A step's history is what its runs of this workflow let run: another
    /// workflow's runs and refused calls are not its own.
    #[test]
    fn a_steps_history_is_what_its_runs_called() {
        let (_d, s) = store();
        run(&s, "r1", "emp", "unbilled-sweep");
        run(&s, "r2", "emp", "unbilled-sweep");
        run(&s, "r3", "emp", "issue-invoice");
        call(&s, "emp", "r1", "find-stuck", "ledger_invoice_search", "allow");
        call(&s, "emp", "r2", "find-stuck", "ledger_aging_receivable", "allow");
        call(&s, "emp", "r2", "find-stuck", "create_schedule", "deny");
        call(&s, "emp", "r3", "find-stuck", "ledger_invoice_create", "allow");
        let h = s.workflow_step_history("emp", "unbilled-sweep").unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h["find-stuck"].iter().map(String::as_str).collect::<Vec<_>>(), ["ledger_aging_receivable", "ledger_invoice_search"]);
    }

    /// A step's undeclared tool is kept once, and the audit goes stale; a
    /// new audit keeps it.
    #[test]
    fn an_undeclared_tool_is_kept_once_and_stales_the_audit() {
        let (_d, s) = store();
        run(&s, "r1", "emp", "unbilled-sweep");
        s.save_workflow_audit("emp", "unbilled-sweep", "h1", "{}").unwrap();
        assert!(s.record_step_heal("r1", "find-unbilled", "ledger_invoice_search", true, "ran").unwrap());
        assert!(s.record_step_heal("r1", "find-unbilled", "ledger_invoice_search", true, "ran").unwrap());
        let row = s.workflow_audit("emp", "unbilled-sweep").unwrap().unwrap();
        assert_eq!(row.definition_hash, "", "stale");
        let healed: Vec<serde_json::Value> = serde_json::from_str(&row.healed).unwrap();
        assert_eq!(healed.len(), 1);
        assert_eq!(healed[0]["tool"], "ledger_invoice_search");
        s.save_workflow_audit("emp", "unbilled-sweep", "h2", "{}").unwrap();
        let row = s.workflow_audit("emp", "unbilled-sweep").unwrap().unwrap();
        assert_eq!(row.definition_hash, "h2");
        assert_eq!(serde_json::from_str::<Vec<serde_json::Value>>(&row.healed).unwrap().len(), 1, "kept across audits");
        assert!(!s.record_step_heal("no-run", "a", "b", true, "ran").unwrap());
    }

    #[test]
    fn a_pick_is_kept_by_its_key() {
        let (_d, s) = store();
        assert_eq!(s.step_pick("k").unwrap(), None);
        s.save_step_pick("k", &["search_web".to_string()]).unwrap();
        assert_eq!(s.step_pick("k").unwrap(), Some(vec!["search_web".to_string()]));
    }
}
