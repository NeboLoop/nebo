use rusqlite::params;

use crate::{DbErrExt, OptionalExt};
use crate::Store;
use crate::models::{
    AgentWorkflowStats, InterruptedRun, Workflow, WorkflowActivityResult, WorkflowRun,
    WorkflowRunError, WorkflowToolBinding,
};
use crate::queries::engine::{NewRun, NewWait};
use types::NeboError;

/// The engine's state read back as the status vocabulary every caller
/// speaks, as a SQL expression over an aliased engine_runs row `r`.
const STATUS_EXPR: &str = "CASE r.state
          WHEN 'done' THEN CASE WHEN r.summary = 'exited' THEN 'exited' ELSE 'completed' END
          WHEN 'cancelled' THEN CASE WHEN r.summary = 'denied' THEN 'denied' ELSE 'cancelled' END
          WHEN 'waiting' THEN 'awaiting_approval'
          WHEN 'queued' THEN 'interrupted'
          ELSE r.state END";

/// A workflow run is an engine run (its durable half: state, definition,
/// inputs, session, result, error) joined to its workflow detail.
const RUN_SELECT: &str = "SELECT w.id, w.workflow_id, w.trigger_type, w.trigger_detail, STATUS_EXPR AS status,
        r.inputs, w.current_activity, w.total_tokens_used, r.error, w.error_activity,
        NULLIF(r.session_key, '') AS session_key, r.result AS output, w.started_at, w.completed_at
 FROM workflow_runs w JOIN engine_runs r ON r.id = w.id";

/// `RUN_SELECT` with the status expression spliced in.
fn run_select() -> String {
    RUN_SELECT.replace("STATUS_EXPR", STATUS_EXPR)
}

/// A workflow status → the engine state it is, plus the summary tag that
/// keeps the finer word (`exited`, `denied`) readable.
fn engine_state_of(status: &str) -> (&str, Option<&str>) {
    match status {
        "completed" => ("done", None),
        "exited" => ("done", Some("exited")),
        "denied" => ("cancelled", Some("denied")),
        "awaiting_approval" | "suspended" => ("waiting", None),
        other => (other, None),
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

impl Store {
    pub fn list_workflows(&self, limit: i64, offset: i64) -> Result<Vec<Workflow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, code, name, version, definition, skill_md, manifest,
                        is_enabled, installed_at, updated_at, napp_path
                 FROM workflows ORDER BY installed_at DESC LIMIT ?1 OFFSET ?2",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit, offset], row_to_workflow)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn count_workflows(&self) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT COUNT(*) FROM workflows", [], |row| row.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_workflow(&self, id: &str) -> Result<Option<Workflow>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, code, name, version, definition, skill_md, manifest,
                    is_enabled, installed_at, updated_at, napp_path
             FROM workflows WHERE id = ?1",
            params![id],
            row_to_workflow,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_workflow_by_code(&self, code: &str) -> Result<Option<Workflow>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, code, name, version, definition, skill_md, manifest,
                    is_enabled, installed_at, updated_at, napp_path
             FROM workflows WHERE code = ?1",
            params![code],
            row_to_workflow,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn create_workflow(
        &self,
        id: &str,
        code: Option<&str>,
        name: &str,
        version: &str,
        definition: &str,
        skill_md: Option<&str>,
        manifest: Option<&str>,
    ) -> Result<Workflow, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO workflows (id, code, name, version, definition, skill_md, manifest)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             RETURNING id, code, name, version, definition, skill_md, manifest,
                       is_enabled, installed_at, updated_at, napp_path",
            params![id, code, name, version, definition, skill_md, manifest],
            row_to_workflow,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_workflow(
        &self,
        id: &str,
        name: &str,
        version: &str,
        definition: &str,
        skill_md: Option<&str>,
        manifest: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE workflows SET name = ?1, version = ?2, definition = ?3,
                    skill_md = ?4, manifest = ?5, updated_at = unixepoch()
             WHERE id = ?6",
            params![name, version, definition, skill_md, manifest, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_workflow_runs(&self, workflow_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM workflow_runs WHERE workflow_id = ?1",
            params![workflow_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_workflow(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM workflows WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_workflow_napp_path(&self, id: &str, napp_path: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE workflows SET napp_path = ?1, updated_at = unixepoch() WHERE id = ?2",
            params![napp_path, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn toggle_workflow(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE workflows SET is_enabled = NOT is_enabled, updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    // ── Workflow Tool Bindings ──

    pub fn list_workflow_bindings(
        &self,
        workflow_id: &str,
    ) -> Result<Vec<WorkflowToolBinding>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, workflow_id, interface_name, tool_code
                 FROM workflow_tool_bindings WHERE workflow_id = ?1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![workflow_id], |row| {
                Ok(WorkflowToolBinding {
                    id: row.get(0)?,
                    workflow_id: row.get(1)?,
                    interface_name: row.get(2)?,
                    tool_code: row.get(3)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn upsert_workflow_binding(
        &self,
        workflow_id: &str,
        interface_name: &str,
        tool_code: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO workflow_tool_bindings (workflow_id, interface_name, tool_code)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(workflow_id, interface_name) DO UPDATE SET tool_code = excluded.tool_code",
            params![workflow_id, interface_name, tool_code],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_workflow_bindings(&self, workflow_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM workflow_tool_bindings WHERE workflow_id = ?1",
            params![workflow_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    // ── Workflow Runs ──

    /// `definition` snapshots the workflow JSON at launch (WS4): a crash
    /// resume executes the definition the run started with, immune to owner
    /// edits and package sync landing mid-flight. `None` for paths whose
    /// definition is durable elsewhere (legacy standalone workflows).
    pub fn create_workflow_run(
        &self,
        id: &str,
        workflow_id: &str,
        trigger_type: &str,
        trigger_detail: Option<&str>,
        inputs: Option<&str>,
        session_key: Option<&str>,
        definition: Option<&str>,
    ) -> Result<WorkflowRun, NeboError> {
        let agent_id = types::keyparser::agent_id_from_workflow_id(workflow_id).unwrap_or("");
        self.engine_create_run(&NewRun {
            id,
            kind: "workflow",
            session_key: session_key.unwrap_or(""),
            agent_id,
            lane: "main",
            definition,
            inputs,
            ..Default::default()
        })?;
        self.engine_set_run_state(id, "running", now(), None)?;
        self.insert_workflow_run_detail(id, workflow_id, trigger_type, trigger_detail)?;
        self.get_workflow_run(id)?
            .ok_or_else(|| NeboError::Database("workflow run vanished after insert".into()))
    }

    /// The workflow half of a run whose engine row already exists (a case
    /// turn queued by the engine before the manager starts it).
    pub fn insert_workflow_run_detail(
        &self,
        id: &str,
        workflow_id: &str,
        trigger_type: &str,
        trigger_detail: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO workflow_runs (id, workflow_id, trigger_type, trigger_detail) VALUES (?1, ?2, ?3, ?4)",
            params![id, workflow_id, trigger_type, trigger_detail],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Boot recovery worklist (WS4): every workflow run the engine's sweep
    /// stamped `interrupted` gets its ONE resume here (I-3) and comes back
    /// with the definition snapshotted at launch; a run interrupted a second
    /// time is failed by the engine as poison and never returned. Runs the
    /// engine sweep already resumed (queued, resume spent) are included, so
    /// the order of the two boot sweeps does not matter.
    pub fn claim_interrupted_workflow_runs(&self, now: i64) -> Result<Vec<InterruptedRun>, NeboError> {
        let candidates: Vec<(String, String)> = {
            let conn = self.conn()?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, state FROM engine_runs
                     WHERE kind = 'workflow'
                       AND (state = 'interrupted' OR (state = 'queued' AND resume_attempted = 1))
                     ORDER BY created_at, rowid",
                )
                .map_err(|e| NeboError::Database(e.to_string()))?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| NeboError::Database(e.to_string()))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| NeboError::Database(e.to_string()))?
        };
        let mut out = Vec::new();
        for (id, state) in candidates {
            if state == "interrupted" && !self.engine_resume_once(&id, now)? {
                continue;
            }
            let conn = self.conn()?;
            let row = conn
                .query_row(
                    "SELECT w.id, w.workflow_id, w.trigger_detail, r.inputs, r.definition
                     FROM workflow_runs w JOIN engine_runs r ON r.id = w.id WHERE w.id = ?1",
                    params![id],
                    |row| {
                        Ok(InterruptedRun {
                            id: row.get(0)?,
                            workflow_id: row.get(1)?,
                            trigger_detail: row.get(2)?,
                            inputs: row.get(3)?,
                            definition: row.get(4)?,
                        })
                    },
                )
                .optional()
                .map_err(|e| NeboError::Database(e.to_string()))?;
            if let Some(r) = row {
                out.push(r);
            }
        }
        Ok(out)
    }

    pub fn update_workflow_run(
        &self,
        id: &str,
        status: Option<&str>,
        current_activity: Option<&str>,
        total_tokens_used: Option<i64>,
        error: Option<&str>,
        error_activity: Option<&str>,
    ) -> Result<(), NeboError> {
        // The durable half goes to the engine run; the rest is workflow detail.
        if let Some(s) = status {
            let (state, tag) = engine_state_of(s);
            if let Some(tag) = tag {
                self.engine_set_run_result_tag(id, tag)?;
            }
            self.engine_set_run_state(id, state, now(), error)?;
        } else if let Some(e) = error {
            self.engine_set_run_state_error(id, e)?;
        }

        let conn = self.conn()?;
        // Build dynamic SET clause
        let mut sets = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1;

        if let Some(ca) = current_activity {
            sets.push(format!("current_activity = ?{}", idx));
            values.push(Box::new(ca.to_string()));
            idx += 1;
        }
        if let Some(t) = total_tokens_used {
            sets.push(format!("total_tokens_used = ?{}", idx));
            values.push(Box::new(t));
            idx += 1;
        }
        if let Some(ea) = error_activity {
            sets.push(format!("error_activity = ?{}", idx));
            values.push(Box::new(ea.to_string()));
            idx += 1;
        }

        if sets.is_empty() {
            return Ok(());
        }

        let sql = format!(
            "UPDATE workflow_runs SET {} WHERE id = ?{}",
            sets.join(", "),
            idx
        );
        values.push(Box::new(id.to_string()));

        let params: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        conn.execute(&sql, params.as_slice())
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn complete_workflow_run(
        &self,
        id: &str,
        status: &str,
        total_tokens_used: i64,
        error: Option<&str>,
        error_activity: Option<&str>,
        output: Option<&str>,
    ) -> Result<(), NeboError> {
        let (state, tag) = engine_state_of(status);
        if let Some(output) = output {
            self.engine_set_run_result(id, output, tag)?;
        } else if let Some(tag) = tag {
            self.engine_set_run_result_tag(id, tag)?;
        }
        self.engine_set_run_state(id, state, now(), error)?;
        let conn = self.conn()?;
        conn.execute(
            "UPDATE workflow_runs
             SET total_tokens_used = ?1, error_activity = ?2, completed_at = unixepoch()
             WHERE id = ?3",
            params![total_tokens_used, error_activity, id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn list_workflow_runs(
        &self,
        workflow_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<WorkflowRun>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "{} WHERE w.workflow_id = ?1 ORDER BY w.started_at DESC LIMIT ?2 OFFSET ?3", run_select()
            ))
            .db_err("list_workflow_runs prepare")?;
        let rows = stmt
            .query_map(params![workflow_id, limit, offset], row_to_workflow_run)
            .db_err("list_workflow_runs query")?;
        rows.collect::<Result<Vec<_>, _>>()
            .db_err("list_workflow_runs collect")
    }

    /// Check if there is already a running workflow run for the given workflow_id
    /// whose trigger_detail starts with the given binding name.
    pub fn has_running_run(
        &self,
        workflow_id: &str,
        binding_prefix: &str,
    ) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let pattern = format!("{}:%", binding_prefix);
        conn.query_row(
            "SELECT COUNT(*) > 0 FROM workflow_runs w JOIN engine_runs r ON r.id = w.id
             WHERE w.workflow_id = ?1 AND r.state = 'running' AND w.trigger_detail LIKE ?2",
            params![workflow_id, pattern],
            |row| row.get(0),
        )
        .db_err("has_running_run")
    }

    /// Runs across every workflow that started at or after `since` (unix
    /// seconds), newest first. The dashboard's recent-runs table.
    pub fn list_workflow_runs_since(
        &self,
        since: i64,
        limit: i64,
    ) -> Result<Vec<WorkflowRun>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "{} WHERE w.started_at >= ?1 ORDER BY w.started_at DESC LIMIT ?2", run_select()
            ))
            .db_err("list_workflow_runs_since prepare")?;
        let rows = stmt
            .query_map(params![since, limit], row_to_workflow_run)
            .db_err("list_workflow_runs_since query")?;
        rows.collect::<Result<Vec<_>, _>>()
            .db_err("list_workflow_runs_since collect")
    }

    /// (local day, workflow_id, status, count) for runs started at or after
    /// `since`. Days are the machine's local calendar, the same one the
    /// scheduler fires on.
    pub fn count_workflow_runs_by_day(
        &self,
        since: i64,
    ) -> Result<Vec<(String, String, String, i64)>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT date(w.started_at, 'unixepoch', 'localtime') AS day, w.workflow_id, {STATUS_EXPR} AS status, COUNT(*)
                 FROM workflow_runs w JOIN engine_runs r ON r.id = w.id WHERE w.started_at >= ?1
                 GROUP BY day, w.workflow_id, status"
            ))
            .db_err("count_workflow_runs_by_day prepare")?;
        let rows = stmt
            .query_map(params![since], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .db_err("count_workflow_runs_by_day query")?;
        rows.collect::<Result<Vec<_>, _>>()
            .db_err("count_workflow_runs_by_day collect")
    }

    pub fn count_workflow_runs(&self, workflow_id: &str) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COUNT(*) FROM workflow_runs WHERE workflow_id = ?1",
            params![workflow_id],
            |row| row.get(0),
        )
        .db_err("count_workflow_runs")
    }

    pub fn get_workflow_run(&self, id: &str) -> Result<Option<WorkflowRun>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            &format!("{} WHERE w.id = ?1", run_select()),
            params![id],
            row_to_workflow_run,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    // ── Activity Results ──

    /// Append one execution to a run's activity log. `iteration` is the loop
    /// scope path ("" outside any loop) — a loop body appends one row per item,
    /// and the path is what tells them apart on resume.
    #[allow(clippy::too_many_arguments)]
    pub fn create_activity_result(
        &self,
        run_id: &str,
        activity_id: &str,
        iteration: &str,
        status: &str,
        tokens_used: i64,
        attempts: i64,
        error: Option<&str>,
        started_at: i64,
        completed_at: Option<i64>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO workflow_activity_results
             (run_id, activity_id, iteration, status, tokens_used, attempts, error, started_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                run_id,
                activity_id,
                iteration,
                status,
                tokens_used,
                attempts,
                error,
                started_at,
                completed_at
            ],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Aggregate stats for all workflow runs belonging to an agent.
    pub fn agent_workflow_stats(&self, agent_id: &str) -> Result<AgentWorkflowStats, NeboError> {
        let conn = self.conn()?;
        let wf_id = types::keyparser::agent_workflow_id(agent_id);
        conn.query_row(
            "SELECT
                COUNT(*) AS total_runs,
                COALESCE(SUM(CASE WHEN r.state = 'done' THEN 1 ELSE 0 END), 0) AS completed,
                COALESCE(SUM(CASE WHEN r.state = 'failed' THEN 1 ELSE 0 END), 0) AS failed,
                COALESCE(SUM(CASE WHEN r.state = 'cancelled' AND r.summary != 'denied' THEN 1 ELSE 0 END), 0) AS cancelled,
                COALESCE(SUM(CASE WHEN r.state = 'running' THEN 1 ELSE 0 END), 0) AS running,
                COALESCE(SUM(w.total_tokens_used), 0) AS total_tokens,
                CAST(AVG(CASE WHEN w.completed_at IS NOT NULL AND w.started_at IS NOT NULL
                    THEN w.completed_at - w.started_at ELSE NULL END) AS INTEGER) AS avg_duration,
                MAX(w.started_at) AS last_run_at,
                MAX(CASE WHEN r.state = 'done' AND r.summary != 'exited' THEN w.started_at ELSE NULL END) AS last_success_at,
                (SELECT r2.error FROM workflow_runs w2 JOIN engine_runs r2 ON r2.id = w2.id
                 WHERE w2.workflow_id = ?1 AND r2.state = 'failed'
                 ORDER BY w2.started_at DESC LIMIT 1) AS last_error
             FROM workflow_runs w JOIN engine_runs r ON r.id = w.id WHERE w.workflow_id = ?1",
            params![wf_id],
            |row| {
                Ok(AgentWorkflowStats {
                    total_runs: row.get(0)?,
                    completed: row.get(1)?,
                    failed: row.get(2)?,
                    cancelled: row.get(3)?,
                    running: row.get(4)?,
                    total_tokens: row.get(5)?,
                    avg_duration_secs: row.get(6)?,
                    last_run_at: row.get(7)?,
                    last_success_at: row.get(8)?,
                    last_error: row.get(9)?,
                })
            },
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Recent failures for an agent's workflows (last N errors).
    pub fn agent_recent_errors(
        &self,
        agent_id: &str,
        limit: i64,
    ) -> Result<Vec<WorkflowRunError>, NeboError> {
        let conn = self.conn()?;
        let wf_id = types::keyparser::agent_workflow_id(agent_id);
        let mut stmt = conn
            .prepare(
                "SELECT w.id, r.error, w.error_activity, w.started_at
                 FROM workflow_runs w JOIN engine_runs r ON r.id = w.id
                 WHERE w.workflow_id = ?1 AND r.state = 'failed' AND r.error IS NOT NULL
                 ORDER BY w.started_at DESC LIMIT ?2",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![wf_id, limit], |row| {
                Ok(WorkflowRunError {
                    run_id: row.get(0)?,
                    error: row.get(1)?,
                    activity_id: row.get(2)?,
                    started_at: row.get(3)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_activity_results(
        &self,
        run_id: &str,
    ) -> Result<Vec<WorkflowActivityResult>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, run_id, activity_id, status, tokens_used, attempts,
                        error, started_at, completed_at
                 FROM workflow_activity_results WHERE run_id = ?1
                 ORDER BY started_at ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![run_id], |row| {
                Ok(WorkflowActivityResult {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    activity_id: row.get(2)?,
                    status: row.get(3)?,
                    tokens_used: row.get(4)?,
                    attempts: row.get(5)?,
                    error: row.get(6)?,
                    started_at: row.get(7)?,
                    completed_at: row.get(8)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    // ── Approval-checkpoint suspensions (headless pause-and-resume) ──

    /// Record a completed activity's output so a suspended run can rebuild the
    /// downstream context on resume. Additive alongside create_activity_result
    /// (whose signature many call sites share).
    pub fn set_activity_result_content(
        &self,
        run_id: &str,
        activity_id: &str,
        iteration: &str,
        content: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        // Scoped to the iteration that produced it. Without the iteration
        // clause this updated EVERY row for the activity, so each loop pass
        // overwrote the output of every pass before it.
        conn.execute(
            "UPDATE workflow_activity_results SET result_content = ?4
             WHERE run_id = ?1 AND activity_id = ?2 AND iteration = ?3",
            params![run_id, activity_id, iteration, content],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Outputs of every completed activity in a run, keyed by
    /// (activity_id, iteration) — used on resume to skip finished work and
    /// rebuild prior context.
    ///
    /// Keying on activity_id alone collapsed a loop body's per-iteration rows
    /// into one entry, so the fast-forward treated item 2 as already done.
    pub fn completed_activity_contents(
        &self,
        run_id: &str,
    ) -> Result<std::collections::HashMap<(String, String), String>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT activity_id, iteration, COALESCE(result_content, '')
                 FROM workflow_activity_results
                 WHERE run_id = ?1 AND status = 'completed'",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![run_id], |row| {
                Ok((
                    (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<std::collections::HashMap<_, _>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Persist a run's approval suspension (one per run; replace on conflict).
    #[allow(clippy::too_many_arguments)]
    pub fn create_workflow_suspension(
        &self,
        run_id: &str,
        agent_id: &str,
        binding_name: &str,
        activity_id: &str,
        iteration: &str,
        step_index: Option<i64>,
        messages: &str,
        pending_tool: &str,
        operation: &str,
        display: &str,
    ) -> Result<(), NeboError> {
        // The run's live wait: resumed by the owner's approval, carrying the
        // parked conversation and the call that parked it. Declaring it
        // moves the run to waiting and supersedes any earlier wait.
        let parked = serde_json::json!({
            "agent_id": agent_id,
            "binding_name": binding_name,
            "activity_id": activity_id,
            "iteration": iteration,
            "step_index": step_index,
            "messages": messages,
            "pending_tool": pending_tool,
            "operation": operation,
            "display": display,
        })
        .to_string();
        self.engine_declare_wait(
            run_id,
            &NewWait {
                action: "resume",
                on_kind: "approval",
                key: &format!("approval:{run_id}"),
                deadline: None,
                parked: Some(&parked),
                reason: display,
            },
            now(),
        )?;
        Ok(())
    }

    /// Load a run's suspension: (agent_id, binding_name, activity_id,
    /// iteration, step_index, messages, pending_tool, operation, display).
    #[allow(clippy::type_complexity)]
    pub fn get_workflow_suspension(
        &self,
        run_id: &str,
    ) -> Result<
        Option<(
            String,
            String,
            String,
            String,
            Option<i64>,
            String,
            String,
            String,
            String,
        )>,
        NeboError,
    > {
        // The run's latest approval wait, live or just resolved: the parked
        // state is read AFTER the approval event released the wait, when
        // the manager rehydrates the conversation. Whether the run is still
        // parked is the run's status, not this row's existence.
        let conn = self.conn()?;
        let parked: Option<String> = conn
            .query_row(
                "SELECT parked FROM engine_waits WHERE run_id = ?1 AND on_kind = 'approval' ORDER BY id DESC LIMIT 1",
                params![run_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| NeboError::Database(e.to_string()))?
            .flatten();
        let Some(parked) = parked else {
            return Ok(None);
        };
        let p: serde_json::Value = serde_json::from_str(&parked)
            .map_err(|_| NeboError::Database(format!("run {run_id}: parked approval is unreadable")))?;
        let s = |k: &str| p[k].as_str().unwrap_or("").to_string();
        Ok(Some((
            s("agent_id"),
            s("binding_name"),
            s("activity_id"),
            s("iteration"),
            p["step_index"].as_i64(),
            s("messages"),
            s("pending_tool"),
            s("operation"),
            s("display"),
        )))
    }

    /// All parked approvals: `(run_id, agent_id, binding_name, display)` per
    /// suspension row. Powers reconcile-on-connect backfill of the owner's web
    /// inbox (pushes are best-effort) and pending-approval listings without
    /// the notification-prefix N+1.
    pub fn list_workflow_suspensions(
        &self,
    ) -> Result<Vec<(String, String, String, String, i64)>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT w.run_id, json_extract(w.parked, '$.agent_id'), json_extract(w.parked, '$.binding_name'),
                        json_extract(w.parked, '$.display'), w.created_at
                 FROM engine_waits w JOIN engine_runs r ON r.current_wait_id = w.id
                 WHERE w.on_kind = 'approval' AND w.superseded_at IS NULL
                 ORDER BY w.id DESC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// The approval was resolved (resume or deny): the wait is released.
    /// The caller sets the run's next state.
    pub fn delete_workflow_suspension(&self, run_id: &str) -> Result<(), NeboError> {
        self.engine_release_wait(run_id, now())
    }
}

fn row_to_workflow(row: &rusqlite::Row) -> rusqlite::Result<Workflow> {
    Ok(Workflow {
        id: row.get(0)?,
        code: row.get(1)?,
        name: row.get(2)?,
        version: row.get(3)?,
        definition: row.get(4)?,
        skill_md: row.get(5)?,
        manifest: row.get(6)?,
        is_enabled: row.get(7)?,
        installed_at: row.get(8)?,
        updated_at: row.get(9)?,
        napp_path: row.get(10)?,
    })
}

fn row_to_workflow_run(row: &rusqlite::Row) -> rusqlite::Result<WorkflowRun> {
    Ok(WorkflowRun {
        id: row.get(0)?,
        workflow_id: row.get(1)?,
        trigger_type: row.get(2)?,
        trigger_detail: row.get(3)?,
        status: row.get(4)?,
        inputs: row.get(5)?,
        current_activity: row.get(6)?,
        total_tokens_used: row.get(7)?,
        error: row.get(8)?,
        error_activity: row.get(9)?,
        session_key: row.get(10)?,
        output: row.get(11)?,
        started_at: row.get(12)?,
        completed_at: row.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use crate::Store;

    /// A loop body appends one row per item under the SAME run and activity id.
    /// Both the read and the content write must key on the iteration, or the
    /// resume fast-forward treats item 2 as already done (it ran the body
    /// exactly once however many items there were) and each pass overwrites
    /// the recorded output of every pass before it.
    #[test]
    fn test_activity_results_are_per_iteration() {
        let path = std::env::temp_dir()
            .join(format!("nebo-wf-iter-test-{}.db", uuid::Uuid::new_v4()));
        let store = Store::new(&path.to_string_lossy()).unwrap();
        store
            .create_workflow_run("run1", "wf1", "manual", None, None, None, None)
            .unwrap();

        for (iteration, content) in [("0", "first item"), ("1", "second item")] {
            store
                .create_activity_result("run1", "body", iteration, "completed", 0, 1, None, 0, Some(1))
                .unwrap();
            store
                .set_activity_result_content("run1", "body", iteration, content)
                .unwrap();
        }

        let done = store.completed_activity_contents("run1").unwrap();

        // Distinct entries — not one collapsed key.
        assert_eq!(done.len(), 2);
        assert_eq!(
            done.get(&("body".into(), "0".into())).map(String::as_str),
            Some("first item")
        );
        // Writing iteration 1 must not have clobbered iteration 0.
        assert_eq!(
            done.get(&("body".into(), "1".into())).map(String::as_str),
            Some("second item")
        );
        // An iteration that never ran is absent, so it is not fast-forwarded.
        assert!(!done.contains_key(&("body".into(), "2".into())));

        let _ = std::fs::remove_file(&path);
    }
}

impl Store {
    /// Terminal runs the reporter has not yet pushed, oldest first. The limit
    /// bounds a single report; the cursor is the row's own reported_at.
    pub fn list_unreported_runs(&self, limit: i64) -> Result<Vec<WorkflowRun>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "{} WHERE r.state IN ('done', 'failed') AND w.reported_at IS NULL
                 ORDER BY w.completed_at ASC LIMIT ?1",
                run_select()
            ))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![limit], row_to_workflow_run)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Marks runs as pushed. Only called after the platform accepted the
    /// batch — an unacked run stays in the outbox and is re-sent, which is
    /// safe because ingest dedups on (bot_id, run_id).
    pub fn mark_runs_reported(&self, ids: &[String]) -> Result<(), NeboError> {
        if ids.is_empty() {
            return Ok(());
        }
        let conn = self.conn()?;
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "UPDATE workflow_runs SET reported_at = unixepoch() WHERE id IN ({placeholders})"
        );
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        conn.execute(&sql, params.as_slice())
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod durability_tests {
    use crate::Store;

    fn store() -> Store {
        let path = std::env::temp_dir()
            .join(format!("nebo-wfdur-test-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    /// WS4-R1/R6 on the engine: the boot sweep stamps every stranded run
    /// `interrupted`; recovery claims it with the definition snapshotted at
    /// launch; finished runs are never touched. The run reads as interrupted
    /// until the relaunch flips it back to running.
    #[test]
    fn sweep_stamps_stranded_runs_and_recovery_claims_the_snapshot() {
        let s = store();
        let created = s
            .create_workflow_run("r-run", "agent:a1", "watch", Some("order-intake"),
                Some(r#"{"_watch_payload":{"id":"m1"}}"#), Some("agent:a1:workflow:r-run"), Some(r#"{"name":"order-intake"}"#))
            .unwrap();
        assert_eq!(created.status, "running");
        assert_eq!(created.session_key.as_deref(), Some("agent:a1:workflow:r-run"));
        assert_eq!(created.inputs.as_deref(), Some(r#"{"_watch_payload":{"id":"m1"}}"#));
        assert_eq!(s.engine_get_run("r-run").unwrap().unwrap().agent_id, "a1");
        s.create_workflow_run("r-done", "agent:a1", "manual", None, None, None, Some("{}"))
            .unwrap();
        s.complete_workflow_run("r-done", "completed", 12, None, None, Some("report")).unwrap();

        assert_eq!(s.engine_mark_interrupted().unwrap().len(), 1, "only the stranded run is swept");
        let rows = s.claim_interrupted_workflow_runs(100).unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.id, "r-run");
        assert_eq!(r.definition.as_deref(), Some(r#"{"name":"order-intake"}"#));
        assert_eq!(r.trigger_detail.as_deref(), Some("order-intake"));
        assert_eq!(s.get_workflow_run("r-run").unwrap().unwrap().status, "interrupted", "never a phantom running row");
        let done = s.get_workflow_run("r-done").unwrap().unwrap();
        assert_eq!(done.status, "completed", "finished runs untouched");
        assert_eq!(done.output.as_deref(), Some("report"));
        assert_eq!(done.total_tokens_used, Some(12));
        assert!(done.completed_at.is_some());

        // Relaunch flips it back; a second claim finds nothing to do.
        s.update_workflow_run("r-run", Some("running"), None, None, None, None).unwrap();
        assert_eq!(s.get_workflow_run("r-run").unwrap().unwrap().status, "running");
        assert!(s.claim_interrupted_workflow_runs(200).unwrap().is_empty());
    }

    /// WS4-R5: a run interrupted a second time is poison — failed by the
    /// engine's resume-once rule, never returned to recovery again.
    #[test]
    fn a_run_interrupted_twice_is_failed_not_boot_looped() {
        let s = store();
        s.create_workflow_run("r1", "agent:a1", "manual", None, None, None, Some("{}")).unwrap();
        s.engine_mark_interrupted().unwrap();
        assert_eq!(s.claim_interrupted_workflow_runs(100).unwrap().len(), 1);
        s.update_workflow_run("r1", Some("running"), None, None, None, None).unwrap();
        // Process died again during the resume.
        s.engine_mark_interrupted().unwrap();
        assert!(s.claim_interrupted_workflow_runs(200).unwrap().is_empty(), "poison: not retried");
        let run = s.get_workflow_run("r1").unwrap().unwrap();
        assert_eq!(run.status, "failed");
        assert!(run.error.as_deref().unwrap_or("").contains("poison"));
    }

    /// A parked approval is the run's live wait: the run reads as awaiting
    /// approval, the suspension reads back whole, the listing shows it, and
    /// resolving it releases the wait and leaves the next state to the caller.
    #[test]
    fn a_parked_approval_is_the_runs_wait() {
        let s = store();
        s.create_workflow_run("r1", "agent:a1", "watch", Some("order-intake:x"), None, None, Some("{}")).unwrap();
        s.create_workflow_suspension("r1", "a1", "order-intake", "act-2", "", Some(3), "[msgs]", r#"{"name":"crm_write"}"#, "crm.write", "Create invoice 1042")
            .unwrap();
        s.update_workflow_run("r1", Some("awaiting_approval"), Some("act-2"), None, None, None).unwrap();
        let run = s.get_workflow_run("r1").unwrap().unwrap();
        assert_eq!(run.status, "awaiting_approval");
        assert_eq!(run.current_activity.as_deref(), Some("act-2"));
        let (agent, binding, activity, iteration, step, messages, pending, op, display) =
            s.get_workflow_suspension("r1").unwrap().unwrap();
        assert_eq!((agent.as_str(), binding.as_str(), activity.as_str(), iteration.as_str(), step), ("a1", "order-intake", "act-2", "", Some(3)));
        assert_eq!((messages.as_str(), pending.as_str(), op.as_str(), display.as_str()), ("[msgs]", r#"{"name":"crm_write"}"#, "crm.write", "Create invoice 1042"));
        let listed = s.list_workflow_suspensions().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].0.as_str(), listed[0].1.as_str(), listed[0].3.as_str()), ("r1", "a1", "Create invoice 1042"));

        // Denied: wait released, run reads as denied; the parked state is
        // still readable (the resume path reads it after release), but it
        // is no longer listed as pending.
        s.delete_workflow_suspension("r1").unwrap();
        s.update_workflow_run("r1", Some("denied"), None, None, Some("Owner denied: Create invoice 1042"), None).unwrap();
        assert!(s.get_workflow_suspension("r1").unwrap().is_some());
        assert!(s.list_workflow_suspensions().unwrap().is_empty());
        let run = s.get_workflow_run("r1").unwrap().unwrap();
        assert_eq!(run.status, "denied");
        assert_eq!(run.error.as_deref(), Some("Owner denied: Create invoice 1042"));
        let stats = s.agent_workflow_stats("a1").unwrap();
        assert_eq!((stats.total_runs, stats.cancelled), (1, 0), "a denial is not a cancellation");
    }
}
