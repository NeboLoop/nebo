//! Tasks. Two things wear the `PendingTask` shape:
//!
//! - Sub-agent and DAG tasks — durable work the orchestrator runs, retries
//!   and recovers. These are ENGINE RUNS (kind `subagent` / `dag`); the
//!   methods below read and write `engine_runs` and map to `PendingTask`.
//! - Checklist items (`task_type = 'tracking'`) — the runner's work-panel
//!   list for a run's steps. Not durable execution; they live in
//!   `pending_tasks`.

use rusqlite::params;

use crate::Store;
use crate::models::PendingTask;
use crate::queries::engine::NewRun;
use types::NeboError;

/// The engine run row read as a task. `status` is the task vocabulary the
/// orchestrator and tools speak; `state` is the engine's.
const TASK_SELECT: &str = "SELECT id, kind, state, session_key, lane, parent_run_id, attempts, result, error, created_at, started_at, ended_at,
        json_extract(inputs, '$.prompt') AS prompt,
        json_extract(inputs, '$.system_prompt') AS system_prompt,
        json_extract(inputs, '$.description') AS description,
        json_extract(inputs, '$.user_id') AS user_id,
        json_extract(inputs, '$.priority') AS priority,
        json_extract(inputs, '$.max_attempts') AS max_attempts
 FROM engine_runs";

/// Engine run kinds that are orchestrator tasks.
const TASK_KINDS: &str = "('subagent', 'dag')";

fn engine_state(status: &str) -> &str {
    match status {
        "pending" => "queued",
        "completed" => "done",
        other => other,
    }
}

fn task_status(state: &str) -> &str {
    match state {
        "queued" | "interrupted" => "pending",
        "waiting" => "running",
        "done" => "completed",
        other => other,
    }
}

impl Store {
    // ── sub-agent / DAG tasks: engine runs ───────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn create_pending_task(
        &self,
        id: &str,
        task_type: &str,
        session_key: &str,
        user_id: Option<&str>,
        prompt: &str,
        system_prompt: Option<&str>,
        description: Option<&str>,
        lane: Option<&str>,
        priority: i64,
        parent_task_id: Option<&str>,
    ) -> Result<PendingTask, NeboError> {
        let inputs = serde_json::json!({
            "prompt": prompt,
            "system_prompt": system_prompt,
            "description": description,
            "user_id": user_id,
            "priority": priority,
            "max_attempts": 3,
        })
        .to_string();
        self.engine_create_run(&NewRun {
            id,
            kind: task_type,
            session_key,
            agent_id: "",
            lane: lane.unwrap_or("subagent"),
            parent_run_id: parent_task_id,
            inputs: Some(&inputs),
            ..Default::default()
        })?;
        self.get_pending_task(id)?
            .ok_or_else(|| NeboError::Database("task vanished after insert".into()))
    }

    /// A task by id: an engine run, or a checklist item.
    pub fn get_pending_task(&self, id: &str) -> Result<Option<PendingTask>, NeboError> {
        let conn = self.conn()?;
        let run = conn
            .query_row(&format!("{TASK_SELECT} WHERE id = ?1"), params![id], run_to_task)
            .optional()
            .map_err(|e| NeboError::Database(e.to_string()))?;
        if run.is_some() {
            return Ok(run);
        }
        conn.query_row(
            "SELECT * FROM pending_tasks WHERE id = ?1",
            params![id],
            row_to_pending_task,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Tasks in one status: engine runs whose state means it, plus
    /// checklist items carrying it verbatim.
    pub fn get_pending_tasks_by_status(&self, status: &str) -> Result<Vec<PendingTask>, NeboError> {
        let conn = self.conn()?;
        let mut out = self.query_tasks(
            &format!("{TASK_SELECT} WHERE kind IN {TASK_KINDS} AND state = ?1 ORDER BY priority DESC, created_at ASC"),
            params![engine_state(status)],
        )?;
        let mut stmt = conn
            .prepare("SELECT * FROM pending_tasks WHERE status = ?1 ORDER BY priority DESC, created_at ASC")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let items = stmt
            .query_map(params![status], row_to_pending_task)
            .map_err(|e| NeboError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))?;
        out.extend(items);
        Ok(out)
    }

    /// What the orchestrator looks at after a restart: tasks queued, still
    /// marked running by the dead process, or interrupted by the engine's
    /// boot sweep.
    pub fn get_recoverable_tasks(&self) -> Result<Vec<PendingTask>, NeboError> {
        self.query_tasks(
            &format!("{TASK_SELECT} WHERE kind IN {TASK_KINDS} AND state IN ('queued', 'running', 'interrupted') ORDER BY priority DESC, created_at ASC"),
            [],
        )
    }

    pub fn get_tasks_by_lane_and_status(
        &self,
        lane: &str,
        status: &str,
    ) -> Result<Vec<PendingTask>, NeboError> {
        self.query_tasks(
            &format!("{TASK_SELECT} WHERE kind IN {TASK_KINDS} AND lane = ?1 AND state = ?2 ORDER BY priority DESC, created_at ASC"),
            params![lane, engine_state(status)],
        )
    }

    pub fn get_tasks_by_user(&self, user_id: &str) -> Result<Vec<PendingTask>, NeboError> {
        self.query_tasks(
            &format!("{TASK_SELECT} WHERE kind IN {TASK_KINDS} AND json_extract(inputs, '$.user_id') = ?1 AND state IN ('queued', 'running', 'interrupted') ORDER BY created_at DESC"),
            params![user_id],
        )
    }

    pub fn get_child_tasks(&self, parent_task_id: &str) -> Result<Vec<PendingTask>, NeboError> {
        self.query_tasks(
            &format!("{TASK_SELECT} WHERE parent_run_id = ?1 ORDER BY created_at ASC"),
            params![parent_task_id],
        )
    }

    pub fn update_task_status(&self, id: &str, status: &str) -> Result<(), NeboError> {
        self.engine_set_run_state(id, engine_state(status), now(), None)?;
        Ok(())
    }

    pub fn update_task_running(&self, id: &str) -> Result<(), NeboError> {
        self.engine_set_run_state(id, "running", now(), None)?;
        Ok(())
    }

    pub fn update_task_completed(&self, id: &str, output: Option<&str>) -> Result<(), NeboError> {
        if let Some(output) = output {
            self.engine_set_run_result(id, output, None)?;
        }
        self.engine_set_run_state(id, "done", now(), None)?;
        Ok(())
    }

    /// A failed attempt: the task goes back to the queue for the
    /// orchestrator's recovery until its attempts are spent, then fails.
    pub fn update_task_failed(&self, id: &str, last_error: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE engine_runs SET
                state = CASE WHEN attempts >= COALESCE(json_extract(inputs, '$.max_attempts'), 3) THEN 'failed' ELSE 'queued' END,
                error = ?2,
                ended_at = CASE WHEN attempts >= COALESCE(json_extract(inputs, '$.max_attempts'), 3) THEN ?3 ELSE NULL END
             WHERE id = ?1",
            params![id, last_error, now()],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn cancel_task(&self, id: &str) -> Result<(), NeboError> {
        self.engine_set_run_state(id, "cancelled", now(), None)?;
        Ok(())
    }

    pub fn cancel_child_tasks(&self, parent_task_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE engine_runs SET state = 'cancelled', ended_at = ?2
             WHERE parent_run_id = ?1 AND state IN ('queued', 'running', 'interrupted')",
            params![parent_task_id, now()],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Returns all pending/running tasks plus recently completed tasks (within the last hour).
    pub fn get_active_and_recent_tasks(&self) -> Result<Vec<PendingTask>, NeboError> {
        self.query_tasks(
            &format!(
                "{TASK_SELECT} WHERE kind IN {TASK_KINDS} AND (state IN ('queued', 'running', 'interrupted')
                    OR (state = 'done' AND ended_at > ?1 - 3600))
                 ORDER BY CASE state WHEN 'running' THEN 0 WHEN 'queued' THEN 1 WHEN 'interrupted' THEN 1 ELSE 2 END, created_at DESC"
            ),
            params![now()],
        )
    }

    fn query_tasks<P: rusqlite::Params>(&self, sql: &str, p: P) -> Result<Vec<PendingTask>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(sql).map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(p, run_to_task)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    // ── checklist items (task_type = 'tracking'): pending_tasks ──────────

    /// Seed an entire task list from a slice of step instructions (workflow mode).
    pub fn seed_task_list(
        &self,
        list_id: &str,
        steps: &[&str],
    ) -> Result<Vec<PendingTask>, NeboError> {
        let conn = self.conn()?;
        let mut items = Vec::with_capacity(steps.len());
        for (i, step) in steps.iter().enumerate() {
            let seq = (i + 1) as i64;
            let id = uuid::Uuid::new_v4().to_string();
            let item = conn
                .query_row(
                    "INSERT INTO pending_tasks (id, task_type, status, session_key, prompt, description, list_id, seq, created_at)
                     VALUES (?1, 'tracking', 'pending', ?2, ?3, ?3, ?2, ?4, unixepoch())
                     RETURNING *",
                    params![id, list_id, step, seq],
                    row_to_pending_task,
                )
                .map_err(|e| NeboError::Database(e.to_string()))?;
            items.push(item);
        }
        Ok(items)
    }

    /// Create a single tracking task (general mode — LLM creates dynamically).
    pub fn create_task_item(
        &self,
        list_id: &str,
        subject: &str,
        description: Option<&str>,
    ) -> Result<PendingTask, NeboError> {
        let conn = self.conn()?;
        let next_seq: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM pending_tasks WHERE list_id = ?1",
                params![list_id],
                |row| row.get(0),
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let id = uuid::Uuid::new_v4().to_string();
        conn.query_row(
            "INSERT INTO pending_tasks (id, task_type, status, session_key, prompt, description, list_id, seq, created_at)
             VALUES (?1, 'tracking', 'pending', ?2, ?3, ?4, ?2, ?5, unixepoch())
             RETURNING *",
            params![id, list_id, subject, description, next_seq],
            row_to_pending_task,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Mark a tracking task as in_progress.
    pub fn start_task_item(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE pending_tasks SET status = 'in_progress', started_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Update a tracking task's status, output, error, and token counts.
    pub fn update_task_item(
        &self,
        id: &str,
        status: &str,
        output: Option<&str>,
        error: Option<&str>,
        tokens_in: i64,
        tokens_out: i64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE pending_tasks SET
                status = ?2,
                output = ?3,
                last_error = ?4,
                tokens_input = ?5,
                tokens_output = ?6,
                completed_at = CASE WHEN ?2 IN ('completed', 'failed', 'skipped') THEN unixepoch() ELSE completed_at END
             WHERE id = ?1",
            params![id, status, output, error, tokens_in, tokens_out],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// List all tracking tasks in a given list, ordered by seq.
    pub fn list_task_items(&self, list_id: &str) -> Result<Vec<PendingTask>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM pending_tasks WHERE list_id = ?1 AND task_type = 'tracking' ORDER BY seq ASC")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![list_id], row_to_pending_task)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Delete tracking task lists completed more than N days ago.
    pub fn cleanup_old_task_lists(&self, days: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM pending_tasks WHERE task_type = 'tracking' AND list_id IN (
                SELECT DISTINCT list_id FROM pending_tasks WHERE task_type = 'tracking'
                GROUP BY list_id
                HAVING MAX(COALESCE(completed_at, created_at)) < unixepoch() - (?1 * 86400)
            )",
            params![days],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// The seven-day TTL on finished tasks — sub-agent runs and checklist
    /// items alike.
    pub fn delete_completed_tasks(&self) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            &format!(
                "DELETE FROM engine_runs WHERE kind IN {TASK_KINDS} AND state IN ('done', 'failed', 'cancelled')
                 AND ended_at < unixepoch() - (7 * 24 * 60 * 60)"
            ),
            [],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        conn.execute(
            "DELETE FROM pending_tasks WHERE status IN ('completed', 'failed', 'cancelled')
             AND completed_at < unixepoch() - (7 * 24 * 60 * 60)",
            [],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn run_to_task(row: &rusqlite::Row) -> rusqlite::Result<PendingTask> {
    let state: String = row.get("state")?;
    let prompt: Option<String> = row.get("prompt")?;
    Ok(PendingTask {
        id: row.get("id")?,
        task_type: row.get("kind")?,
        status: task_status(&state).to_string(),
        session_key: row.get("session_key")?,
        user_id: row.get("user_id")?,
        prompt: prompt.unwrap_or_default(),
        system_prompt: row.get("system_prompt")?,
        description: row.get("description")?,
        lane: row.get("lane")?,
        priority: row.get("priority")?,
        attempts: row.get("attempts")?,
        max_attempts: row.get("max_attempts")?,
        last_error: row.get("error")?,
        created_at: row.get("created_at")?,
        started_at: row.get("started_at")?,
        completed_at: row.get("ended_at")?,
        parent_task_id: row.get("parent_run_id")?,
        output: row.get("result")?,
        list_id: None,
        seq: None,
        tokens_input: None,
        tokens_output: None,
        metadata: None,
    })
}

fn row_to_pending_task(row: &rusqlite::Row) -> rusqlite::Result<PendingTask> {
    Ok(PendingTask {
        id: row.get("id")?,
        task_type: row.get("task_type")?,
        status: row.get("status")?,
        session_key: row.get("session_key")?,
        user_id: row.get("user_id")?,
        prompt: row.get("prompt")?,
        system_prompt: row.get("system_prompt")?,
        description: row.get("description")?,
        lane: row.get("lane")?,
        priority: row.get("priority")?,
        attempts: row.get("attempts")?,
        max_attempts: row.get("max_attempts")?,
        last_error: row.get("last_error")?,
        created_at: row.get("created_at")?,
        started_at: row.get("started_at")?,
        completed_at: row.get("completed_at")?,
        parent_task_id: row.get("parent_task_id")?,
        output: row.get("output")?,
        list_id: row.get("list_id")?,
        seq: row.get("seq")?,
        tokens_input: row.get("tokens_input")?,
        tokens_output: row.get("tokens_output")?,
        metadata: row.get("metadata")?,
    })
}

trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-tasks-test-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    /// The orchestrator's whole lifecycle on engine runs: create, run,
    /// fail-and-requeue until attempts are spent, recover, cancel children.
    #[test]
    fn subagent_tasks_are_engine_runs_with_retry_and_recovery() {
        let s = store();
        let parent = s
            .create_pending_task("dag-1", "dag", "agent:a:web", Some("u1"), "plan", None, Some("DAG"), Some("subagent"), 0, None)
            .unwrap();
        assert_eq!((parent.status.as_str(), parent.task_type.as_str(), parent.max_attempts), ("pending", "dag", Some(3)));
        let child = s
            .create_pending_task("dag-1-a", "subagent", "subagent:agent:a:web:dag-1-a", Some("u1"), "do a", Some("sys"), Some("A"), Some("subagent"), 5, Some("dag-1"))
            .unwrap();
        assert_eq!(child.parent_task_id.as_deref(), Some("dag-1"));
        assert_eq!(child.priority, Some(5));
        assert_eq!(s.get_child_tasks("dag-1").unwrap().len(), 1);
        assert_eq!(s.get_recoverable_tasks().unwrap().len(), 2);

        // Two failures re-queue, the third fails for good.
        for _ in 0..2 {
            s.update_task_running("dag-1-a").unwrap();
            s.update_task_failed("dag-1-a", "boom").unwrap();
            let t = s.get_pending_task("dag-1-a").unwrap().unwrap();
            assert_eq!(t.status, "pending");
            assert_eq!(t.last_error.as_deref(), Some("boom"));
        }
        s.update_task_running("dag-1-a").unwrap();
        s.update_task_failed("dag-1-a", "boom").unwrap();
        let t = s.get_pending_task("dag-1-a").unwrap().unwrap();
        assert_eq!((t.status.as_str(), t.attempts), ("failed", Some(3)));
        assert!(t.completed_at.is_some());
        assert_eq!(s.get_pending_tasks_by_status("failed").unwrap().len(), 1);

        // A completed task carries its output; a cancelled parent takes its live children.
        s.update_task_running("dag-1").unwrap();
        s.update_task_completed("dag-1", Some("all done")).unwrap();
        assert_eq!(s.get_pending_task("dag-1").unwrap().unwrap().output.as_deref(), Some("all done"));
        assert_eq!(s.get_pending_tasks_by_status("completed").unwrap().len(), 1);
        let _ = s.create_pending_task("dag-1-b", "subagent", "k", None, "do b", None, None, None, 0, Some("dag-1")).unwrap();
        s.cancel_child_tasks("dag-1").unwrap();
        assert_eq!(s.get_pending_task("dag-1-b").unwrap().unwrap().status, "cancelled");
        assert!(s.get_recoverable_tasks().unwrap().is_empty());

        // Checklist items are still found by id through the same door.
        let item = s.create_task_item("list-1", "Draft", None).unwrap();
        assert_eq!(s.get_pending_task(&item.id).unwrap().unwrap().task_type, "tracking");
        assert_eq!(s.get_pending_tasks_by_status("pending").unwrap().len(), 1, "the checklist item; no engine task is pending");
    }
}
