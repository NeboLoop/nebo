use rusqlite::params;

use crate::Store;
use crate::models::{CronHistory, CronJob};
use types::NeboError;

impl Store {
    pub fn list_cron_jobs(&self, limit: i64, offset: i64) -> Result<Vec<CronJob>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, schedule, command, task_type, message, deliver, instructions,
                        enabled, last_run, run_count, last_error, created_at,
                        agent_id, channel_ctx_json
                 FROM cron_jobs ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit, offset], row_to_cron_job)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_cron_job(&self, id: i64) -> Result<Option<CronJob>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, name, schedule, command, task_type, message, deliver, instructions,
                    enabled, last_run, run_count, last_error, created_at,
                    agent_id, channel_ctx_json
             FROM cron_jobs WHERE id = ?1",
            params![id],
            row_to_cron_job,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_cron_job_by_name(&self, name: &str) -> Result<Option<CronJob>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, name, schedule, command, task_type, message, deliver, instructions,
                    enabled, last_run, run_count, last_error, created_at,
                    agent_id, channel_ctx_json
             FROM cron_jobs WHERE name = ?1",
            params![name],
            row_to_cron_job,
        )
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
        conn.query_row(
            "INSERT INTO cron_jobs (name, schedule, command, task_type, message, deliver, instructions, enabled, agent_id, channel_ctx_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             RETURNING id, name, schedule, command, task_type, message, deliver, instructions,
                       enabled, last_run, run_count, last_error, created_at,
                       agent_id, channel_ctx_json",
            params![name, schedule, command, task_type, message, deliver, instructions, enabled as i64, agent_id, channel_ctx_json],
            row_to_cron_job,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
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

    pub fn update_cron_job_last_run(
        &self,
        id: i64,
        last_error: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE cron_jobs SET last_run = datetime('now'), run_count = run_count + 1, last_error = ?2 WHERE id = ?1",
            params![id, last_error],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Record the outcome of a run without touching last_run/run_count —
    /// those are written once at dispatch time by `update_cron_job_last_run`.
    pub fn update_cron_job_last_error(
        &self,
        id: i64,
        last_error: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE cron_jobs SET last_error = ?2 WHERE id = ?1",
            params![id, last_error],
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
            .prepare(
                "SELECT id, name, schedule, command, task_type, message, deliver, instructions,
                        enabled, last_run, run_count, last_error, created_at,
                        agent_id, channel_ctx_json
                 FROM cron_jobs WHERE enabled = 1 ORDER BY name",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_cron_job)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Enabled jobs whose dispatch never recorded an outcome — the process died
    /// mid-run. last_run is consumed at dispatch time, so without a recovery
    /// sweep these occurrences would be silently lost. Bounded to the last 24h
    /// so ancient dangling rows don't resurrect stale work.
    pub fn list_interrupted_cron_jobs(&self) -> Result<Vec<CronJob>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT j.id, j.name, j.schedule, j.command, j.task_type, j.message,
                        j.deliver, j.instructions, j.enabled, j.last_run, j.run_count,
                        j.last_error, j.created_at, j.agent_id, j.channel_ctx_json
                 FROM cron_jobs j
                 JOIN cron_history h ON h.job_id = j.id
                 WHERE h.finished_at IS NULL
                   AND j.enabled = 1
                   AND h.started_at >= datetime('now', '-1 day')
                 ORDER BY j.name",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_cron_job)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Mark every unfinished history row as failed. Called once at startup,
    /// before the first tick, so interrupted runs read as failed instead of
    /// forever pending. Returns the number of rows closed.
    pub fn close_interrupted_cron_history(&self) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE cron_history
             SET finished_at = CURRENT_TIMESTAMP, success = 0,
                 error = 'interrupted by restart'
             WHERE finished_at IS NULL",
            [],
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn create_cron_history(&self, job_id: i64) -> Result<CronHistory, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO cron_history (job_id, started_at)
             VALUES (?1, CURRENT_TIMESTAMP)
             RETURNING id, job_id, started_at, finished_at, success, output, error",
            params![job_id],
            row_to_cron_history,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_cron_history(
        &self,
        id: i64,
        success: bool,
        output: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE cron_history SET finished_at = CURRENT_TIMESTAMP, success = ?2, output = ?3, error = ?4 WHERE id = ?1",
            params![id, success as i64, output, error],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn list_cron_history(
        &self,
        job_id: i64,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CronHistory>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, job_id, started_at, finished_at, success, output, error
                 FROM cron_history WHERE job_id = ?1 ORDER BY started_at DESC LIMIT ?2 OFFSET ?3",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![job_id, limit, offset], |row| {
                row_to_cron_history(row)
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_recent_cron_history(&self, job_id: i64) -> Result<Vec<CronHistory>, NeboError> {
        self.list_cron_history(job_id, 10, 0)
    }

    pub fn count_cron_history(&self, job_id: i64) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COUNT(*) FROM cron_history WHERE job_id = ?1",
            params![job_id],
            |row| row.get(0),
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }
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

fn row_to_cron_history(row: &rusqlite::Row) -> rusqlite::Result<CronHistory> {
    Ok(CronHistory {
        id: row.get("id")?,
        job_id: row.get("job_id")?,
        started_at: row.get("started_at")?,
        finished_at: row.get("finished_at")?,
        success: row.get("success")?,
        output: row.get("output")?,
        error: row.get("error")?,
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

    #[test]
    fn test_interrupted_cron_recovery() {
        let store = temp_store();
        let job = store
            .create_cron_job(
                "j-enabled", "0 0 9 * * *", "echo hi", "shell",
                None, None, None, true, None, None,
            )
            .unwrap();
        let disabled = store
            .create_cron_job(
                "j-disabled", "0 0 9 * * *", "echo hi", "shell",
                None, None, None, false, None, None,
            )
            .unwrap();

        // No dangling history yet
        assert!(store.list_interrupted_cron_jobs().unwrap().is_empty());

        // Dangling rows for both jobs — only the enabled one is recoverable
        let h1 = store.create_cron_history(job.id).unwrap();
        let _h2 = store.create_cron_history(disabled.id).unwrap();

        let interrupted = store.list_interrupted_cron_jobs().unwrap();
        assert_eq!(interrupted.len(), 1);
        assert_eq!(interrupted[0].id, job.id);

        // A finished row is no longer interrupted
        store.update_cron_history(h1.id, true, None, None).unwrap();
        assert!(store.list_interrupted_cron_jobs().unwrap().is_empty());

        // close_interrupted marks every remaining dangling row failed,
        // regardless of the job's enabled state
        let _h3 = store.create_cron_history(job.id).unwrap();
        let closed = store.close_interrupted_cron_history().unwrap();
        assert_eq!(closed, 2); // disabled job's row + the new dangling row
        assert!(store.list_interrupted_cron_jobs().unwrap().is_empty());
    }
}
