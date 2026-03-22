use rusqlite::params;

use crate::models::{RoleWorkflowStats, Workflow, WorkflowActivityResult, WorkflowRun, WorkflowRunError, WorkflowToolBinding};
use crate::OptionalExt;
use crate::Store;
use types::NeboError;

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
        conn.execute("DELETE FROM workflow_runs WHERE workflow_id = ?1", params![workflow_id])
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

    pub fn create_workflow_run(
        &self,
        id: &str,
        workflow_id: &str,
        trigger_type: &str,
        trigger_detail: Option<&str>,
        inputs: Option<&str>,
        session_key: Option<&str>,
    ) -> Result<WorkflowRun, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO workflow_runs (id, workflow_id, trigger_type, trigger_detail, inputs, session_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             RETURNING id, workflow_id, trigger_type, trigger_detail, status, inputs,
                       current_activity, total_tokens_used, error, error_activity,
                       session_key, output, started_at, completed_at",
            params![id, workflow_id, trigger_type, trigger_detail, inputs, session_key],
            row_to_workflow_run,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
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
        let conn = self.conn()?;
        // Build dynamic SET clause
        let mut sets = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1;

        if let Some(s) = status {
            sets.push(format!("status = ?{}", idx));
            values.push(Box::new(s.to_string()));
            idx += 1;
        }
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
        if let Some(e) = error {
            sets.push(format!("error = ?{}", idx));
            values.push(Box::new(e.to_string()));
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
        let conn = self.conn()?;
        conn.execute(
            "UPDATE workflow_runs
             SET status = ?1, total_tokens_used = ?2, error = ?3,
                 error_activity = ?4, output = ?5, completed_at = unixepoch()
             WHERE id = ?6",
            params![status, total_tokens_used, error, error_activity, output, id],
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
            .prepare(
                "SELECT id, workflow_id, trigger_type, trigger_detail, status, inputs,
                        current_activity, total_tokens_used, error, error_activity,
                        session_key, output, started_at, completed_at
                 FROM workflow_runs WHERE workflow_id = ?1
                 ORDER BY started_at DESC LIMIT ?2 OFFSET ?3",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![workflow_id, limit, offset], row_to_workflow_run)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn count_workflow_runs(&self, workflow_id: &str) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COUNT(*) FROM workflow_runs WHERE workflow_id = ?1",
            params![workflow_id],
            |row| row.get(0),
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_workflow_run(&self, id: &str) -> Result<Option<WorkflowRun>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, workflow_id, trigger_type, trigger_detail, status, inputs,
                    current_activity, total_tokens_used, error, error_activity,
                    session_key, output, started_at, completed_at
             FROM workflow_runs WHERE id = ?1",
            params![id],
            row_to_workflow_run,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Mark any "running" workflow runs as cancelled (orphaned from previous shutdown).
    pub fn cleanup_orphaned_runs(&self) -> Result<u64, NeboError> {
        let conn = self.conn()?;
        let count = conn.execute(
            "UPDATE workflow_runs SET status = 'cancelled', error = 'server restart', completed_at = unixepoch()
             WHERE status = 'running'",
            [],
        ).map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(count as u64)
    }

    // ── Activity Results ──

    pub fn create_activity_result(
        &self,
        run_id: &str,
        activity_id: &str,
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
             (run_id, activity_id, status, tokens_used, attempts, error, started_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![run_id, activity_id, status, tokens_used, attempts, error, started_at, completed_at],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Aggregate stats for all workflow runs belonging to a role.
    pub fn role_workflow_stats(&self, role_id: &str) -> Result<RoleWorkflowStats, NeboError> {
        let conn = self.conn()?;
        let wf_id = format!("role:{}", role_id);
        conn.query_row(
            "SELECT
                COUNT(*) AS total_runs,
                COALESCE(SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END), 0) AS completed,
                COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) AS failed,
                COALESCE(SUM(CASE WHEN status = 'cancelled' THEN 1 ELSE 0 END), 0) AS cancelled,
                COALESCE(SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END), 0) AS running,
                COALESCE(SUM(total_tokens_used), 0) AS total_tokens,
                AVG(CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL
                    THEN completed_at - started_at ELSE NULL END) AS avg_duration,
                MAX(started_at) AS last_run_at,
                MAX(CASE WHEN status = 'completed' THEN started_at ELSE NULL END) AS last_success_at,
                (SELECT error FROM workflow_runs
                 WHERE workflow_id = ?1 AND status = 'failed'
                 ORDER BY started_at DESC LIMIT 1) AS last_error
             FROM workflow_runs WHERE workflow_id = ?1",
            params![wf_id],
            |row| {
                Ok(RoleWorkflowStats {
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

    /// Recent failures for a role's workflows (last N errors).
    pub fn role_recent_errors(
        &self,
        role_id: &str,
        limit: i64,
    ) -> Result<Vec<WorkflowRunError>, NeboError> {
        let conn = self.conn()?;
        let wf_id = format!("role:{}", role_id);
        let mut stmt = conn
            .prepare(
                "SELECT id, error, error_activity, started_at
                 FROM workflow_runs
                 WHERE workflow_id = ?1 AND status = 'failed' AND error IS NOT NULL
                 ORDER BY started_at DESC LIMIT ?2",
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

