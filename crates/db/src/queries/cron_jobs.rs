//! Scheduled jobs. `cron_jobs` holds the DEFINITION of a schedule — name,
//! cron, what to run. Everything durable about it lives in the engine: each
//! fire is an engine run of kind `task` with `external_ref = cron:<id>`, and
//! the next occurrence is one pending timer aimed at binding `cron:<id>`.
//! `last_run`, `run_count` and `last_error` are read from those runs.

use rusqlite::params;

use crate::Store;
use crate::models::{CronHistory, CronJob};
use crate::queries::engine::NewRun;
use types::NeboError;

/// The job row plus its derived columns. Every read goes through this.
const JOB_SELECT: &str = "SELECT j.id, j.name, j.schedule, j.command, j.task_type, j.message, j.deliver, j.instructions,
        j.enabled, j.created_at, j.agent_id, j.channel_ctx_json,
        (SELECT datetime(MAX(r.created_at), 'unixepoch') FROM engine_runs r WHERE r.external_ref = 'cron:' || j.id) AS last_run,
        (SELECT COUNT(*) FROM engine_runs r WHERE r.external_ref = 'cron:' || j.id) AS run_count,
        (SELECT r.error FROM engine_runs r WHERE r.external_ref = 'cron:' || j.id ORDER BY r.created_at DESC, r.rowid DESC LIMIT 1) AS last_error
 FROM cron_jobs j";

/// The engine ref every fire of a job carries.
pub fn cron_ref(job_id: i64) -> String {
    format!("cron:{job_id}")
}

impl Store {
    pub fn list_cron_jobs(&self, limit: i64, offset: i64) -> Result<Vec<CronJob>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!("{JOB_SELECT} ORDER BY j.created_at DESC LIMIT ?1 OFFSET ?2"))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit, offset], row_to_cron_job)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_cron_job(&self, id: i64) -> Result<Option<CronJob>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(&format!("{JOB_SELECT} WHERE j.id = ?1"), params![id], row_to_cron_job)
            .optional()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_cron_job_by_name(&self, name: &str) -> Result<Option<CronJob>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(&format!("{JOB_SELECT} WHERE j.name = ?1"), params![name], row_to_cron_job)
            .optional()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn create_cron_job(
        &self,
        name: &str,
        schedule: &str,
        command: &str,
        task_type: &str,
        message: Option<&str>,
        deliver: Option<&str>,
        instructions: Option<&str>,
        enabled: bool,
        agent_id: Option<&str>,
        channel_ctx_json: Option<&str>,
    ) -> Result<CronJob, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO cron_jobs (name, schedule, command, task_type, message, deliver, instructions, enabled, agent_id, channel_ctx_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![name, schedule, command, task_type, message, deliver, instructions, enabled as i64, agent_id, channel_ctx_json],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        let id = conn.last_insert_rowid();
        drop(conn);
        self.get_cron_job(id)?
            .ok_or_else(|| NeboError::Database("cron job vanished after insert".into()))
    }

    pub fn upsert_cron_job(
        &self,
        name: &str,
        schedule: &str,
        command: &str,
        task_type: &str,
        message: Option<&str>,
        deliver: Option<&str>,
        instructions: Option<&str>,
        enabled: bool,
        agent_id: Option<&str>,
        channel_ctx_json: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO cron_jobs (name, schedule, command, task_type, message, deliver, instructions, enabled, agent_id, channel_ctx_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(name) DO UPDATE SET
                schedule = excluded.schedule, command = excluded.command,
                task_type = excluded.task_type, message = excluded.message,
                deliver = excluded.deliver, instructions = excluded.instructions,
                enabled = excluded.enabled,
                agent_id = excluded.agent_id, channel_ctx_json = excluded.channel_ctx_json",
            params![name, schedule, command, task_type, message, deliver, instructions, enabled as i64, agent_id, channel_ctx_json],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_cron_job(&self, id: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM cron_jobs WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_cron_job_by_name(&self, name: &str) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM cron_jobs WHERE name = ?1", params![name])
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn toggle_cron_job(&self, id: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE cron_jobs SET enabled = NOT enabled WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_cron_job_enabled(&self, id: i64, enabled: bool) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE cron_jobs SET enabled = ?2 WHERE id = ?1",
            params![id, enabled as i64],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn enable_cron_job_by_name(&self, name: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE cron_jobs SET enabled = 1 WHERE name = ?1",
            params![name],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn disable_cron_job_by_name(&self, name: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE cron_jobs SET enabled = 0 WHERE name = ?1",
            params![name],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn count_cron_jobs(&self) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT COUNT(*) FROM cron_jobs", [], |row| row.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_enabled_cron_jobs(&self) -> Result<Vec<CronJob>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!("{JOB_SELECT} WHERE j.enabled = 1 ORDER BY j.name"))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_cron_job)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    // ── fires ─────────────────────────────────────────────────────────────

    /// Queue one fire of a job as an engine run. The engine loop executes it
    /// and records the outcome on the same row; `manual` marks a run-now so
    /// its completion is announced to the UI rather than the desktop.
    pub fn queue_cron_run(&self, job: &CronJob, manual: bool) -> Result<String, NeboError> {
        let id = uuid::Uuid::new_v4().to_string();
        let inputs = serde_json::json!({ "job_id": job.id, "name": job.name, "manual": manual }).to_string();
        self.engine_create_run(&NewRun {
            id: &id,
            kind: "task",
            session_key: &format!("cron-{}", job.name),
            agent_id: job.agent_id.as_deref().unwrap_or(""),
            lane: "main",
            inputs: Some(&inputs),
            external_ref: Some(&cron_ref(job.id)),
            ..Default::default()
        })?;
        Ok(id)
    }

    pub fn list_cron_history(
        &self,
        job_id: i64,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CronHistory>, NeboError> {
        let runs = self.engine_runs_for_ref(&cron_ref(job_id), limit, offset)?;
        Ok(runs
            .into_iter()
            .map(|r| CronHistory {
                id: r.id,
                job_id,
                started_at: Some(db_datetime(r.started_at.unwrap_or(r.created_at))),
                finished_at: r.ended_at.map(db_datetime),
                success: Some((r.state == "done") as i64),
                output: r.result,
                error: r.error,
            })
            .collect())
    }

    pub fn get_recent_cron_history(&self, job_id: i64) -> Result<Vec<CronHistory>, NeboError> {
        self.list_cron_history(job_id, 10, 0)
    }

    pub fn count_cron_history(&self, job_id: i64) -> Result<i64, NeboError> {
        self.engine_count_runs_for_ref(&cron_ref(job_id))
    }
}

/// The `datetime('now')` shape the old columns had, so every reader of
/// `last_run` / history timestamps sees what it always saw.
fn db_datetime(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

fn row_to_cron_job(row: &rusqlite::Row) -> rusqlite::Result<CronJob> {
    Ok(CronJob {
        id: row.get("id")?,
        name: row.get("name")?,
        schedule: row.get("schedule")?,
        command: row.get("command")?,
        task_type: row.get("task_type")?,
        message: row.get("message")?,
        deliver: row.get("deliver")?,
        instructions: row.get("instructions")?,
        enabled: row.get("enabled")?,
        last_run: row.get("last_run")?,
        run_count: row.get("run_count")?,
        last_error: row.get("last_error")?,
        created_at: row.get("created_at")?,
        agent_id: row.get("agent_id")?,
        channel_ctx_json: row.get("channel_ctx_json")?,
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

    fn temp_store() -> Store {
        let path = std::env::temp_dir().join(format!(
            "nebo-cron-test-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Store::new(path.to_str().unwrap()).unwrap()
    }

    /// A job's last run, run count, last error and history are its engine
    /// runs — nothing is stamped on the job row.
    #[test]
    fn derived_columns_and_history_read_from_engine_runs() {
        let store = temp_store();
        let job = store
            .create_cron_job("j", "0 0 9 * * *", "echo hi", "shell", None, None, None, true, None, None)
            .unwrap();
        assert_eq!(job.run_count, Some(0));
        assert!(job.last_run.is_none());
        assert!(store.list_cron_history(job.id, 10, 0).unwrap().is_empty());

        let first = store.queue_cron_run(&job, false).unwrap();
        store.engine_set_run_state(&first, "running", 1_000, None).unwrap();
        store.engine_set_run_result(&first, "hi", None).unwrap();
        store.engine_set_run_state(&first, "done", 1_001, None).unwrap();
        let second = store.queue_cron_run(&job, true).unwrap();
        store.engine_set_run_state(&second, "running", 2_000, None).unwrap();
        store.engine_set_run_state(&second, "failed", 2_001, Some("exit code: 1")).unwrap();

        let job = store.get_cron_job(job.id).unwrap().unwrap();
        assert_eq!(job.run_count, Some(2));
        assert!(job.last_run.is_some());
        assert_eq!(job.last_error.as_deref(), Some("exit code: 1"));
        assert_eq!(store.count_cron_history(job.id).unwrap(), 2);

        let history = store.list_cron_history(job.id, 10, 0).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].id, second, "newest first");
        assert_eq!(history[0].success, Some(0));
        assert_eq!(history[0].error.as_deref(), Some("exit code: 1"));
        assert_eq!(history[1].success, Some(1));
        assert_eq!(history[1].output.as_deref(), Some("hi"));
        assert_eq!(history[1].started_at.as_deref(), Some("1970-01-01 00:16:40"));
        assert_eq!(history[1].finished_at.as_deref(), Some("1970-01-01 00:16:41"));
        assert!(store.engine_has_live_run_for_ref("cron:1").unwrap() == false);
    }
}
