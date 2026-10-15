//! Tasks. Two things wear the `PendingTask` shape:
//!
//! - Helpers: the harness's delegated work. These are ENGINE RUNS (kind
//!   `helper`); the methods below read and write `engine_runs` and map to
//!   `PendingTask`.
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

/// Engine run kinds that are tasks.
const TASK_KINDS: &str = "('helper')";

/// The checklist a workflow run keeps for one activity's steps. The run
/// owns every list under `run:<run id>:`, and its end closes them
/// ([`Store::close_run_task_items`]).
pub fn run_task_list(run_id: &str, activity_id: &str) -> String {
    format!("run:{run_id}:{activity_id}")
}

/// The checklist a conversation keeps with the task tools. A helper's is
/// its run's, so it closes when the helper ends however it ends, the way a
/// workflow run's does; any other conversation's is its own.
pub fn conversation_task_list(session_key: &str, session_id: &str) -> String {
    match types::keyparser::split_helper_key(session_key) {
        Some((_, run_id)) => run_task_list(run_id, "tasks"),
        None => format!("session:{session_id}"),
    }
}

/// How a checklist item is closed when the run that owns it ends before
/// reaching it, or while it was on it: `exited`, the word a step that
/// stopped early already ends with. Status only; the row stays.
const CLOSE_OPEN_ITEMS: &str = "UPDATE pending_tasks SET status = 'exited', completed_at = unixepoch()
     WHERE task_type = 'tracking' AND status IN ('pending', 'in_progress')";

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

    /// What the orchestrator looks at after a restart: tasks queued, still
    /// marked running by the dead process, or interrupted by the engine's
    /// boot sweep.
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
        let ended: bool = conn
            .query_row(
                "UPDATE engine_runs SET
                    state = CASE WHEN attempts >= COALESCE(json_extract(inputs, '$.max_attempts'), 3) THEN 'failed' ELSE 'queued' END,
                    error = ?2,
                    ended_at = CASE WHEN attempts >= COALESCE(json_extract(inputs, '$.max_attempts'), 3) THEN ?3 ELSE NULL END
                 WHERE id = ?1 RETURNING state = 'failed'",
                params![id, last_error, now()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| NeboError::Database(e.to_string()))?
            .unwrap_or(false);
        drop(conn);
        if ended {
            self.close_run_task_items(id)?;
        }
        Ok(())
    }

    pub fn cancel_task(&self, id: &str) -> Result<(), NeboError> {
        self.engine_set_run_state(id, "cancelled", now(), None)?;
        Ok(())
    }

    /// Cancelling a parent takes every live descendant with it — children,
    /// grandchildren, however deep the fan-out went. Finished ones are left
    /// as they ended.
    pub fn cancel_child_tasks(&self, parent_task_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "WITH RECURSIVE below(id) AS (
                     SELECT id FROM engine_runs WHERE parent_run_id = ?1
                     UNION
                     SELECT r.id FROM engine_runs r JOIN below b ON r.parent_run_id = b.id
                 )
                 UPDATE engine_runs SET state = 'cancelled', ended_at = ?2, current_wait_id = NULL
                 WHERE id IN (SELECT id FROM below) AND state IN ('queued', 'running', 'waiting', 'interrupted')
                 RETURNING id",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let cancelled = stmt
            .query_map(params![parent_task_id, now()], |row| row.get::<_, String>(0))
            .map_err(|e| NeboError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))?;
        drop(stmt);
        drop(conn);
        for id in cancelled {
            self.close_run_task_items(&id)?;
        }
        Ok(())
    }

    // ── checklist items (task_type = 'tracking'): pending_tasks ──────────

    /// The items for one pass over an activity's steps (workflow mode), one
    /// per step in order. A resumed run carries on the pass it parked in:
    /// that pass is the list's latest item for each step. Any other start —
    /// the first, a retry, a loop's next item, a relaunch after a crash —
    /// is a new pass, and what an earlier pass left open is closed first,
    /// so only the pass at work has open items.
    pub fn seed_task_list(
        &self,
        list_id: &str,
        steps: &[&str],
        resume: bool,
    ) -> Result<Vec<PendingTask>, NeboError> {
        let conn = self.conn()?;
        if resume {
            let mut stmt = conn
                .prepare(
                    "SELECT * FROM pending_tasks WHERE rowid IN (
                         SELECT MAX(rowid) FROM pending_tasks
                         WHERE list_id = ?1 AND task_type = 'tracking' GROUP BY seq
                     ) ORDER BY seq ASC",
                )
                .map_err(|e| NeboError::Database(e.to_string()))?;
            let parked = stmt
                .query_map(params![list_id], row_to_pending_task)
                .map_err(|e| NeboError::Database(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| NeboError::Database(e.to_string()))?;
            if parked.len() == steps.len() {
                return Ok(parked);
            }
        }
        conn.execute(&format!("{CLOSE_OPEN_ITEMS} AND list_id = ?1"), params![list_id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
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

    /// Mark a tracking task as in_progress. Only a pending one: an item its
    /// run already closed stays closed.
    pub fn start_task_item(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE pending_tasks SET status = 'in_progress', started_at = unixepoch() WHERE id = ?1 AND status = 'pending'",
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
                completed_at = CASE WHEN ?2 IN ('completed', 'failed', 'skipped', 'exited') THEN unixepoch() ELSE completed_at END
             WHERE id = ?1",
            params![id, status, output, error, tokens_in, tokens_out],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// A list's items, ordered by seq. `current` is the list as whoever
    /// works it sees it now: once every item on it is finished the list is
    /// done and reads empty, and the next item starts it afresh — what came
    /// before stays in the table, unread. `false` is its whole history, for
    /// a run's record.
    pub fn list_task_items(&self, list_id: &str, current: bool) -> Result<Vec<PendingTask>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM pending_tasks WHERE list_id = ?1 AND task_type = 'tracking' ORDER BY seq ASC, rowid ASC")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let items = stmt
            .query_map(params![list_id], row_to_pending_task)
            .map_err(|e| NeboError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))?;
        if !current {
            return Ok(items);
        }
        // An item starts a fresh list when everything before it had
        // finished before it was made (whole seconds: the same second keeps
        // them on one list).
        let mut start = 0;
        let mut finished_by: Option<i64> = Some(0);
        for (i, item) in items.iter().enumerate() {
            if i > 0 && finished_by.is_some_and(|t| t < item.created_at) {
                start = i;
            }
            finished_by = match (finished_by, item.completed_at) {
                (Some(t), Some(done)) if !is_open(&item.status) => Some(t.max(done)),
                _ => None,
            };
        }
        let list = items.into_iter().skip(start).collect::<Vec<_>>();
        Ok(if list.iter().any(|t| is_open(&t.status)) { list } else { Vec::new() })
    }

    /// A run ended: every item still open on its lists is closed. The one
    /// close every end goes through: the engine's terminal state write
    /// ([`Store::engine_set_run_state`]), its poison fail, a helper's last
    /// failed attempt and a cancelled parent's descendants — so a workflow
    /// run or a helper that is done, exited, failed, cancelled, stopped or
    /// interrupted twice leaves no item open.
    pub fn close_run_task_items(&self, run_id: &str) -> Result<usize, NeboError> {
        let lo = run_task_list(run_id, "");
        // `;` follows `:`: the range is exactly the lists under `run:<id>:`.
        let hi = format!("run:{run_id};");
        let conn = self.conn()?;
        conn.execute(&format!("{CLOSE_OPEN_ITEMS} AND list_id >= ?1 AND list_id < ?2"), params![lo, hi])
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Boot: close the open items of every run that is no longer live —
    /// ended, or gone — left by a build that did not close lists at a run's
    /// end, or by a process that died before it could. A live run's lists
    /// (queued, running, waiting, interrupted and about to resume) are left
    /// to it. Status only; nothing is deleted.
    pub fn close_orphaned_task_items(&self) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            &format!(
                "{CLOSE_OPEN_ITEMS} AND substr(list_id, 1, 4) = 'run:'
                 AND NOT EXISTS (
                     SELECT 1 FROM engine_runs r
                     WHERE r.id = substr(list_id, 5, instr(substr(list_id, 5), ':') - 1)
                       AND r.state IN ('queued', 'running', 'waiting', 'interrupted')
                 )"
            ),
            [],
        )
        .map_err(|e| NeboError::Database(e.to_string()))
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

/// A checklist item still to do or being done.
fn is_open(status: &str) -> bool {
    matches!(status, "pending" | "in_progress")
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
    use super::{conversation_task_list, run_task_list};
    use crate::Store;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-tasks-test-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    /// A helper's whole lifecycle on engine runs: create, run,
    /// fail-and-requeue until attempts are spent, live listing, cancel children.
    #[test]
    fn helper_tasks_are_engine_runs_with_retry_and_recovery() {
        let s = store();
        let parent = s
            .create_pending_task("dag-1", "helper", "agent:a:web", Some("u1"), "plan", None, Some("DAG"), Some("subagent"), 0, None)
            .unwrap();
        assert_eq!((parent.status.as_str(), parent.task_type.as_str(), parent.max_attempts), ("pending", "helper", Some(3)));
        let child = s
            .create_pending_task("dag-1-a", "helper", "subagent:agent:a:web:dag-1-a", Some("u1"), "do a", Some("sys"), Some("A"), Some("subagent"), 5, Some("dag-1"))
            .unwrap();
        assert_eq!(child.parent_task_id.as_deref(), Some("dag-1"));
        assert_eq!(child.priority, Some(5));
        assert_eq!(s.engine_live_runs_of_kind("helper").unwrap().len(), 2);

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

        // A completed task carries its output; a cancelled parent takes its live children.
        s.update_task_running("dag-1").unwrap();
        s.update_task_completed("dag-1", Some("all done")).unwrap();
        let done = s.get_pending_task("dag-1").unwrap().unwrap();
        assert_eq!((done.status.as_str(), done.output.as_deref()), ("completed", Some("all done")));
        let _ = s.create_pending_task("dag-1-b", "helper", "k", None, "do b", None, None, None, 0, Some("dag-1")).unwrap();
        s.cancel_child_tasks("dag-1").unwrap();
        assert_eq!(s.get_pending_task("dag-1-b").unwrap().unwrap().status, "cancelled");
        assert!(s.engine_live_runs_of_kind("helper").unwrap().is_empty());

        // Checklist items are still found by id through the same door.
        let item = s.create_task_item("list-1", "Draft", None).unwrap();
        assert_eq!(s.get_pending_task(&item.id).unwrap().unwrap().task_type, "tracking");
    }

    /// The statuses on one of a run's lists, sorted (a re-seeded list
    /// repeats its step numbers).
    fn statuses(s: &Store, run_id: &str, activity: &str) -> Vec<String> {
        let mut all: Vec<String> = s.list_task_items(&run_task_list(run_id, activity), false).unwrap().into_iter().map(|t| t.status).collect();
        all.sort();
        all
    }

    /// A run on step two of a three-step checklist (step one done, after
    /// an earlier attempt stopped on its first step), with a second activity
    /// whose list is seeded and not reached.
    fn run_with_open_lists(s: &Store, id: &str) {
        s.create_workflow_run(id, "agent:a1", "schedule", Some("scan"), None, None, Some("{}")).unwrap();
        let earlier = s.seed_task_list(&run_task_list(id, "scan"), &["one", "two", "three"], false).unwrap();
        s.start_task_item(&earlier[0].id).unwrap();
        let items = s.seed_task_list(&run_task_list(id, "scan"), &["one", "two", "three"], false).unwrap();
        s.start_task_item(&items[0].id).unwrap();
        s.update_task_item(&items[0].id, "completed", Some("ok"), None, 1, 1).unwrap();
        s.start_task_item(&items[1].id).unwrap();
        s.seed_task_list(&run_task_list(id, "report"), &["write"], false).unwrap();
    }

    /// Every way a workflow run ends closes every item still open on its
    /// lists — the step it was on, the steps it never reached, an earlier
    /// attempt's leftovers — and leaves what had already ended as it ended.
    /// Seen live: 1,034 items left `pending` and 3 `in_progress` by runs
    /// that exited, failed or were cancelled early.
    #[test]
    fn every_way_a_run_ends_closes_its_checklist() {
        let s = store();
        let ends: [(&str, &dyn Fn(&Store, &str)); 9] = [
            ("completed", &|s, id| s.complete_workflow_run(id, "completed", 0, None, None, None).unwrap()),
            ("exited", &|s, id| s.complete_workflow_run(id, "exited", 0, Some("nothing to do"), None, None).unwrap()),
            ("failed", &|s, id| s.complete_workflow_run(id, "failed", 0, Some("boom"), None, None).unwrap()),
            ("stopped", &|s, id| s.complete_workflow_run(id, "stopped", 0, Some("limit"), None, None).unwrap()),
            ("cancelled parked", &|s, id| s.complete_workflow_run(id, "cancelled", 0, Some("cancelled by user"), None, None).unwrap()),
            ("denied", &|s, id| s.complete_workflow_run(id, "denied", 0, Some("no"), None, None).unwrap()),
            ("cancelled running", &|s, id| s.update_workflow_run(id, Some("cancelled"), None, None, Some("cancelled by user"), None).unwrap()),
            ("failed by the manager", &|s, id| s.update_workflow_run(id, Some("failed"), None, None, Some("timed out"), None).unwrap()),
            ("interrupted twice", &|s, id| {
                s.engine_mark_interrupted().unwrap();
                assert!(s.engine_resume_once(id, 1).unwrap());
                s.update_workflow_run(id, Some("running"), None, None, None, None).unwrap();
                s.engine_mark_interrupted().unwrap();
                assert!(!s.engine_resume_once(id, 2).unwrap(), "poison");
            }),
        ];
        for (i, (how, end)) in ends.iter().enumerate() {
            let id = format!("r{i}");
            run_with_open_lists(&s, &id);
            // A run whose id extends this one's is another run's lists.
            let other = format!("r{i}0");
            run_with_open_lists(&s, &other);
            end(&s, &id);
            assert_eq!(
                statuses(&s, &id, "scan"),
                ["completed", "exited", "exited", "exited", "exited", "exited"],
                "{how}: step one stays done; the step it was on, the rest, and the earlier attempt's close"
            );
            assert_eq!(statuses(&s, &id, "report"), ["exited"], "{how}: an activity never reached");
            assert!(
                s.list_task_items(&run_task_list(&id, "scan"), false).unwrap().iter().all(|t| t.completed_at.is_some()),
                "{how}: every closed item says when"
            );
            assert_eq!(statuses(&s, &other, "scan"), ["completed", "exited", "exited", "exited", "in_progress", "pending"], "{how}: only its pass at work is open");
            s.update_workflow_run(&other, Some("completed"), None, None, None, None).unwrap();
        }

        // A parked run is not over: its list stays open for the resume.
        run_with_open_lists(&s, "parked");
        s.update_workflow_run("parked", Some("awaiting_approval"), None, None, None, None).unwrap();
        assert_eq!(statuses(&s, "parked", "report"), ["pending"]);
        // And a closed item is not started again by a step still in flight.
        let closed = s.list_task_items(&run_task_list("r0", "report"), false).unwrap();
        s.start_task_item(&closed[0].id).unwrap();
        assert_eq!(statuses(&s, "r0", "report"), ["exited"]);
    }

    /// Boot closes the lists runs left open before their end closed them —
    /// ended runs and runs that are gone — and leaves live runs' lists and
    /// a chat's own list alone. Status only: every row is still there.
    #[test]
    fn the_boot_sweep_closes_orphans_and_deletes_nothing() {
        let s = store();
        for id in ["done", "failed", "running", "waiting", "interrupted"] {
            s.create_workflow_run(id, "agent:a1", "schedule", Some("scan"), None, None, Some("{}")).unwrap();
        }
        let leave_open = |id: &str| {
            let items = s.seed_task_list(&run_task_list(id, "scan"), &["one", "two"], false).unwrap();
            s.start_task_item(&items[0].id).unwrap();
        };
        for id in ["done", "failed", "running", "waiting", "interrupted", "gone"] {
            leave_open(id);
        }
        s.create_task_item("session:chat-1", "Draft the reply", None).unwrap();
        // Ended the way an older build ended them: the state, nothing else.
        s.conn_exec_for_test("UPDATE engine_runs SET state = 'done' WHERE id = 'done'");
        s.conn_exec_for_test("UPDATE engine_runs SET state = 'failed' WHERE id = 'failed'");
        s.update_workflow_run("waiting", Some("awaiting_approval"), None, None, None, None).unwrap();
        s.conn_exec_for_test("UPDATE engine_runs SET state = 'interrupted' WHERE id = 'interrupted'");
        let rows = |s: &Store| s.conn().unwrap().query_row("SELECT COUNT(*) FROM pending_tasks", [], |r| r.get::<_, i64>(0)).unwrap();
        let before = rows(&s);

        assert_eq!(s.close_orphaned_task_items().unwrap(), 6, "done, failed and gone: two items each");
        for id in ["done", "failed", "gone"] {
            assert_eq!(statuses(&s, id, "scan"), ["exited", "exited"], "{id}");
        }
        for id in ["running", "waiting", "interrupted"] {
            assert_eq!(statuses(&s, id, "scan"), ["in_progress", "pending"], "{id} is live");
        }
        let chat: Vec<String> = s.list_task_items("session:chat-1", false).unwrap().into_iter().map(|t| t.status).collect();
        assert_eq!(chat, ["pending"], "a chat's list is the chat's");
        assert_eq!(rows(&s), before, "nothing deleted");
        assert_eq!(s.close_orphaned_task_items().unwrap(), 0, "a second boot finds nothing");
    }

    /// A retry, a loop's next item or a relaunch is a new pass: what the
    /// earlier pass left open closes as it starts. A resume at a parked step
    /// carries on the parked pass — no second set of items.
    #[test]
    fn a_new_pass_closes_the_last_and_a_resume_carries_it_on() {
        let s = store();
        s.create_workflow_run("r", "agent:a1", "schedule", Some("scan"), None, None, Some("{}")).unwrap();
        let list = run_task_list("r", "scan");
        let first = s.seed_task_list(&list, &["one", "two", "three"], false).unwrap();
        s.start_task_item(&first[0].id).unwrap();
        s.update_task_item(&first[0].id, "failed", None, Some("boom"), 0, 0).unwrap();
        let retry = s.seed_task_list(&list, &["one", "two", "three"], false).unwrap();
        assert_eq!(statuses(&s, "r", "scan"), ["exited", "exited", "failed", "pending", "pending", "pending"]);

        // Parks at step two: step one done, step two back to pending.
        s.update_task_item(&retry[0].id, "completed", Some("ok"), None, 1, 1).unwrap();
        s.update_task_item(&retry[1].id, "pending", None, Some("awaiting approval"), 0, 0).unwrap();
        let resumed = s.seed_task_list(&list, &["one", "two", "three"], true).unwrap();
        assert_eq!(resumed.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), retry.iter().map(|t| t.id.as_str()).collect::<Vec<_>>());
        assert_eq!(s.list_task_items(&list, false).unwrap().len(), 6, "no third set");
    }

    /// A conversation's list reads empty once everything on it is finished,
    /// and the next item starts it afresh; the finished items stay in the
    /// table. A helper's list is its run's and closes when the helper ends.
    #[test]
    fn a_finished_conversation_list_reads_empty_and_a_helpers_closes_with_it() {
        let s = store();
        let chat = conversation_task_list("agent:a1:web", "chat-1");
        assert_eq!(chat, "session:chat-1");
        let a = s.create_task_item(&chat, "Draft", None).unwrap();
        let b = s.create_task_item(&chat, "Send", None).unwrap();
        s.update_task_item(&a.id, "completed", Some("ok"), None, 0, 0).unwrap();
        assert_eq!(s.list_task_items(&chat, true).unwrap().len(), 2, "one still open: the whole list");
        s.update_task_item(&b.id, "completed", Some("sent"), None, 0, 0).unwrap();
        assert!(s.list_task_items(&chat, true).unwrap().is_empty(), "all finished: done");
        // Made after the old list was done (a second later, as unixepoch has it).
        s.conn_exec_for_test(&format!("UPDATE pending_tasks SET completed_at = completed_at - 5, created_at = created_at - 5 WHERE list_id = '{chat}'"));
        let c = s.create_task_item(&chat, "Follow up", None).unwrap();
        let now: Vec<String> = s.list_task_items(&chat, true).unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(now, [c.id.clone()], "a fresh list");
        assert_eq!(s.list_task_items(&chat, false).unwrap().len(), 3, "nothing deleted");

        // A helper's list.
        s.create_pending_task("h1", "helper", "subagent:agent:a1:web:h1", None, "look", None, None, Some("subagent"), 0, None).unwrap();
        let helper = conversation_task_list("subagent:agent:a1:web:h1", "helper-session");
        assert_eq!(helper, run_task_list("h1", "tasks"));
        s.create_task_item(&helper, "Read the files", None).unwrap();
        s.update_task_running("h1").unwrap();
        s.update_task_completed("h1", Some("done")).unwrap();
        assert!(s.list_task_items(&helper, false).unwrap().iter().all(|t| t.status == "exited"), "closed with the helper");
        // A helper that fails for good, and one cancelled with its parent.
        s.create_pending_task("h-parent", "helper", "agent:a1:web", None, "fan out", None, None, Some("subagent"), 0, None).unwrap();
        for (id, end) in [("h2", "failed"), ("h3", "cancelled")] {
            s.create_pending_task(id, "helper", &format!("subagent:agent:a1:web:{id}"), None, "look", None, None, Some("subagent"), 0, Some("h-parent")).unwrap();
            let list = run_task_list(id, "tasks");
            s.create_task_item(&list, "Read", None).unwrap();
            if end == "failed" {
                for _ in 0..3 {
                    s.update_task_running(id).unwrap();
                    s.update_task_failed(id, "boom").unwrap();
                }
            } else {
                s.cancel_child_tasks("h-parent").unwrap();
            }
            assert_eq!(s.get_pending_task(id).unwrap().unwrap().status, end);
            assert!(s.list_task_items(&list, false).unwrap().iter().all(|t| t.status == "exited"), "{end}");
        }
    }
}
