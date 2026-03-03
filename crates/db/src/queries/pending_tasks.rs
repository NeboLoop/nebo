use rusqlite::params;

use crate::models::PendingTask;
use crate::Store;
use types::NeboError;

impl Store {
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
    ) -> Result<PendingTask, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO pending_tasks (id, task_type, status, session_key, user_id, prompt, system_prompt, description, lane, priority, created_at)
             VALUES (?1, ?2, 'pending', ?3, ?4, ?5, ?6, ?7, ?8, ?9, unixepoch()) RETURNING *",
            params![id, task_type, session_key, user_id, prompt, system_prompt, description, lane, priority],
            row_to_pending_task,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_pending_task(&self, id: &str) -> Result<Option<PendingTask>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM pending_tasks WHERE id = ?1",
            params![id],
            row_to_pending_task,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_pending_tasks_by_status(
        &self,
        status: &str,
    ) -> Result<Vec<PendingTask>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM pending_tasks WHERE status = ?1
                 ORDER BY priority DESC, created_at ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![status], row_to_pending_task)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_recoverable_tasks(&self) -> Result<Vec<PendingTask>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM pending_tasks WHERE status IN ('pending', 'running')
                 ORDER BY priority DESC, created_at ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_pending_task)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_tasks_by_lane_and_status(
        &self,
        lane: &str,
        status: &str,
    ) -> Result<Vec<PendingTask>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM pending_tasks WHERE lane = ?1 AND status = ?2
                 ORDER BY priority DESC, created_at ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![lane, status], row_to_pending_task)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_tasks_by_user(&self, user_id: &str) -> Result<Vec<PendingTask>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM pending_tasks WHERE user_id = ?1 AND status IN ('pending', 'running')
                 ORDER BY created_at DESC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id], row_to_pending_task)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_child_tasks(&self, parent_task_id: &str) -> Result<Vec<PendingTask>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM pending_tasks WHERE parent_task_id = ?1 ORDER BY created_at ASC")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![parent_task_id], row_to_pending_task)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_task_status(&self, id: &str, status: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE pending_tasks SET status = ?2,
                started_at = CASE WHEN ?2 = 'running' THEN unixepoch() ELSE started_at END,
                completed_at = CASE WHEN ?2 IN ('completed', 'failed', 'cancelled') THEN unixepoch() ELSE completed_at END
             WHERE id = ?1",
            params![id, status],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_task_running(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE pending_tasks SET status = 'running', started_at = unixepoch(), attempts = attempts + 1 WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_task_completed(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE pending_tasks SET status = 'completed', completed_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_task_failed(&self, id: &str, last_error: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE pending_tasks SET
                status = CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'pending' END,
                last_error = ?2,
                completed_at = CASE WHEN attempts >= max_attempts THEN unixepoch() ELSE NULL END
             WHERE id = ?1",
            params![id, last_error],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn cancel_task(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE pending_tasks SET status = 'cancelled', completed_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn cancel_child_tasks(&self, parent_task_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE pending_tasks SET status = 'cancelled', completed_at = unixepoch()
             WHERE parent_task_id = ?1 AND status IN ('pending', 'running')",
            params![parent_task_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_completed_tasks(&self) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM pending_tasks WHERE status IN ('completed', 'failed', 'cancelled')
             AND completed_at < unixepoch() - (7 * 24 * 60 * 60)",
            [],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
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
