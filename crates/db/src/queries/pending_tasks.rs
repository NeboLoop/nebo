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

    pub fn update_task_completed(&self, id: &str, output: Option<&str>) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE pending_tasks SET status = 'completed', completed_at = unixepoch(), output = ?2 WHERE id = ?1",
            params![id, output],
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

    /// Returns all pending/running tasks plus recently completed tasks (within the last hour).
    pub fn get_active_and_recent_tasks(&self) -> Result<Vec<PendingTask>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM pending_tasks
                 WHERE status IN ('pending', 'running')
                    OR (status = 'completed' AND completed_at > unixepoch() - 3600)
                 ORDER BY
                    CASE status WHEN 'running' THEN 0 WHEN 'pending' THEN 1 ELSE 2 END,
                    created_at DESC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_pending_task)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    // --- Tracking methods (task_type = 'tracking') ---

    /// Seed an entire task list from a slice of step instructions (workflow mode).
    pub fn seed_task_list(&self, list_id: &str, steps: &[&str]) -> Result<Vec<PendingTask>, NeboError> {
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

    /// Get a single tracking task by ID.
    pub fn get_task_item(&self, id: &str) -> Result<Option<PendingTask>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM pending_tasks WHERE id = ?1",
            params![id],
            row_to_pending_task,
        )
        .optional()
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
