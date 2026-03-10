use rusqlite::params;

use crate::models::{CronHistory, CronJob};
use crate::Store;
use types::NeboError;

impl Store {
    pub fn list_cron_jobs(&self, limit: i64, offset: i64) -> Result<Vec<CronJob>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, schedule, command, task_type, message, deliver, instructions,
                        enabled, last_run, run_count, last_error, created_at
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
                    enabled, last_run, run_count, last_error, created_at
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
                    enabled, last_run, run_count, last_error, created_at
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
    ) -> Result<CronJob, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO cron_jobs (name, schedule, command, task_type, message, deliver, instructions, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             RETURNING id, name, schedule, command, task_type, message, deliver, instructions,
                       enabled, last_run, run_count, last_error, created_at",
            params![name, schedule, command, task_type, message, deliver, instructions, enabled as i64],
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
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO cron_jobs (name, schedule, command, task_type, message, deliver, instructions, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(name) DO UPDATE SET
                schedule = excluded.schedule, command = excluded.command,
                task_type = excluded.task_type, message = excluded.message,
                deliver = excluded.deliver, instructions = excluded.instructions,
                enabled = excluded.enabled",
            params![name, schedule, command, task_type, message, deliver, instructions, enabled as i64],
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
                        enabled, last_run, run_count, last_error, created_at
                 FROM cron_jobs WHERE enabled = 1 ORDER BY name",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_cron_job)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
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
