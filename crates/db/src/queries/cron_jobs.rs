//! Scheduled jobs. `cron_jobs` holds the DEFINITION of a schedule — name,
//! cron, what to run. Everything durable about it lives in the engine: each
//! fire is an engine run of kind `task` with `external_ref = cron:<id>`, and
//! the next occurrence is one pending timer aimed at binding `cron:<id>`.
//! `last_run`, `run_count` and `last_error` are read from those runs.

use rusqlite::params;

use crate::Store;
use crate::models::{CronHistory, CronJob, OverlapPolicy, ScheduleProvenance};
use crate::queries::engine::NewRun;
use types::NeboError;

/// The job row plus its derived columns. Every read goes through this.
const JOB_SELECT: &str = "SELECT j.id, j.name, j.schedule, j.command, j.task_type, j.message, j.deliver, j.instructions,
        j.enabled, j.created_at, j.agent_id, j.channel_ctx_json, j.overlap_policy,
        j.created_by, j.created_by_run, j.created_in, j.reason,
        (SELECT datetime(MAX(r.created_at), 'unixepoch') FROM engine_runs r WHERE r.external_ref = 'cron:' || j.id) AS last_run,
        (SELECT COUNT(*) FROM engine_runs r WHERE r.external_ref = 'cron:' || j.id) AS run_count,
        (SELECT r.error FROM engine_runs r WHERE r.external_ref = 'cron:' || j.id ORDER BY r.created_at DESC, r.rowid DESC LIMIT 1) AS last_error
 FROM cron_jobs j";

/// Most enabled schedules one employee may hold (owner-wide schedules, no
/// employee, count as one holder). The live fleet's busiest employee held 6
/// (2026-10-08, 15 cloud bots); 50 is room for any real job and a stop for
/// a runaway.
pub const MAX_ACTIVE_SCHEDULES_PER_EMPLOYEE: i64 = 50;

/// Most one-shot schedules ("in 5 minutes") one employee may create in an
/// hour. The fleet's most in any hour was 3, outside the Vivid incident
/// (82 in one hour, 2026-10-08).
pub const MAX_ONE_SHOTS_PER_HOUR: i64 = 10;

/// A one-shot schedule: seven fields with a fixed year (`sec min hour dom
/// mon * 2026`, what `at` resolves to). Repeating schedules have five or six
/// fields, or `*` for the year.
pub fn is_one_shot(schedule: &str) -> bool {
    let fields: Vec<&str> = schedule.split_whitespace().collect();
    fields.len() == 7 && fields[6].len() == 4 && fields[6].chars().all(|c| c.is_ascii_digit())
}

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
        // `None` = the default, skip.
        overlap: Option<OverlapPolicy>,
        provenance: &ScheduleProvenance,
    ) -> Result<CronJob, NeboError> {
        let conn = self.conn()?;
        if enabled {
            let active: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM cron_jobs WHERE enabled = 1 AND agent_id IS ?1",
                    params![agent_id],
                    |r| r.get(0),
                )
                .map_err(|e| NeboError::Database(e.to_string()))?;
            if active >= MAX_ACTIVE_SCHEDULES_PER_EMPLOYEE {
                return Err(NeboError::Validation(format!(
                    "This employee already has {active} active schedules, the most one may have \
                     ({MAX_ACTIVE_SCHEDULES_PER_EMPLOYEE}). Nothing was scheduled. Delete or pause ones it no \
                     longer needs, or tell the owner."
                )));
            }
        }
        if is_one_shot(schedule) {
            let recent: Vec<String> = conn
                .prepare(
                    "SELECT schedule FROM cron_jobs WHERE agent_id IS ?1 AND created_at >= datetime('now', '-1 hour')",
                )
                .and_then(|mut st| st.query_map(params![agent_id], |r| r.get(0))?.collect())
                .map_err(|e| NeboError::Database(e.to_string()))?;
            let one_shots = recent.iter().filter(|s| is_one_shot(s)).count() as i64;
            if one_shots >= MAX_ONE_SHOTS_PER_HOUR {
                return Err(NeboError::Validation(format!(
                    "This employee made {one_shots} one-time schedules in the last hour, the most it may \
                     ({MAX_ONE_SHOTS_PER_HOUR}). Nothing was scheduled. Don't schedule retries or checks: \
                     tell the owner what is waiting."
                )));
            }
        }
        conn.execute(
            "INSERT INTO cron_jobs (name, schedule, command, task_type, message, deliver, instructions, enabled, agent_id, channel_ctx_json, overlap_policy,
                                    created_by, created_by_run, created_in, reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, COALESCE(?11, 'skip'), ?12, ?13, ?14, ?15)",
            params![
                name, schedule, command, task_type, message, deliver, instructions, enabled as i64, agent_id, channel_ctx_json,
                overlap.map(OverlapPolicy::as_str),
                provenance.created_by.as_str(), provenance.run_id, provenance.session_key, provenance.reason
            ],
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
        // `None` keeps the job's policy (skip for a new one): a trigger
        // re-registered at every load never undoes the owner's choice.
        overlap: Option<OverlapPolicy>,
        // Who made it and why. A schedule that already exists keeps the
        // provenance it was made with: an edit or a re-registration is not
        // a new maker.
        provenance: &ScheduleProvenance,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO cron_jobs (name, schedule, command, task_type, message, deliver, instructions, enabled, agent_id, channel_ctx_json, overlap_policy,
                                    created_by, created_by_run, created_in, reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, COALESCE(?11, 'skip'), ?12, ?13, ?14, ?15)
             ON CONFLICT(name) DO UPDATE SET
                schedule = excluded.schedule, command = excluded.command,
                task_type = excluded.task_type, message = excluded.message,
                deliver = excluded.deliver, instructions = excluded.instructions,
                enabled = excluded.enabled,
                agent_id = excluded.agent_id, channel_ctx_json = excluded.channel_ctx_json,
                overlap_policy = COALESCE(?11, overlap_policy)",
            params![
                name, schedule, command, task_type, message, deliver, instructions, enabled as i64, agent_id, channel_ctx_json,
                overlap.map(OverlapPolicy::as_str),
                provenance.created_by.as_str(), provenance.run_id, provenance.session_key, provenance.reason
            ],
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

    /// Set a job's schedule only while it still reads `old`, so a repair
    /// never overwrites a schedule changed since it was read. Whether it
    /// was set.
    pub fn replace_cron_job_schedule(&self, id: i64, old: &str, new: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE cron_jobs SET schedule = ?3 WHERE id = ?1 AND schedule = ?2",
                params![id, old, new],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(changed == 1)
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

    /// Switch off the enabled schedules the run `run_id` made, and return
    /// them as they were. A turn the owner stopped takes the schedules it
    /// set up with it. Switched off, never deleted, as every retired
    /// schedule is: what it was stays visible.
    pub fn retire_schedules_made_by(&self, run_id: &str) -> Result<Vec<CronJob>, NeboError> {
        if run_id.is_empty() {
            return Ok(Vec::new());
        }
        let jobs: Vec<CronJob> = {
            let conn = self.conn()?;
            let mut stmt = conn
                .prepare(&format!("{JOB_SELECT} WHERE j.created_by_run = ?1 AND j.enabled = 1"))
                .map_err(|e| NeboError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![run_id], row_to_cron_job)
                .map_err(|e| NeboError::Database(e.to_string()))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| NeboError::Database(e.to_string()))?
        };
        for job in &jobs {
            self.set_cron_job_enabled(job.id, false)?;
        }
        Ok(jobs)
    }

    /// When each schedule fires next: its one pending timer's due moment
    /// (unix seconds), keyed by job id. A schedule with no timer armed yet
    /// (just made, or switched off) is absent.
    pub fn cron_next_fires(&self) -> Result<std::collections::HashMap<i64, i64>, NeboError> {
        Ok(self
            .engine_pending_timers("binding")?
            .into_iter()
            .filter_map(|t| Some((t.target_id.strip_prefix("cron:")?.parse::<i64>().ok()?, t.due_at?)))
            .collect())
    }

    // ── fires ─────────────────────────────────────────────────────────────

    /// Queue one fire of a job as an engine run. The engine loop executes it
    /// and records the outcome on the same row; `manual` marks a run-now so
    /// its completion is announced to the UI rather than the desktop.
    /// `buffered` holds it `waiting` until the fire before it ends (overlap
    /// policy buffer_one); the engine releases it then.
    pub fn queue_cron_run(&self, job: &CronJob, manual: bool, buffered: bool) -> Result<String, NeboError> {
        let id = uuid::Uuid::new_v4().to_string();
        let inputs = serde_json::json!({ "job_id": job.id, "name": job.name, "manual": manual }).to_string();
        self.engine_create_run(&NewRun {
            state: buffered.then_some("waiting"),
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
        overlap_policy: row.get("overlap_policy")?,
        created_by: row.get("created_by")?,
        created_by_run: row.get("created_by_run")?,
        created_in: row.get("created_in")?,
        reason: row.get("reason")?,
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
    use super::{MAX_ACTIVE_SCHEDULES_PER_EMPLOYEE, MAX_ONE_SHOTS_PER_HOUR, is_one_shot};
    use crate::Store;
    use types::NeboError;

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
    fn one_shots_are_seven_fields_with_a_year() {
        assert!(is_one_shot("16 12 16 8 10 * 2026"));
        assert!(!is_one_shot("0 0 9 * * * *"));
        assert!(!is_one_shot("0 9 * * 1-5"));
        assert!(!is_one_shot("0 0 9 * * *"));
    }

    /// The caps: active schedules per employee, one-shots per hour. Another
    /// employee is counted on its own.
    #[test]
    fn caps_hold_per_employee() {
        let store = temp_store();
        for i in 0..MAX_ONE_SHOTS_PER_HOUR {
            store
                .create_cron_job(&format!("retry-{i}"), "16 12 16 8 10 * 2099", "", "agent", Some("x"), None, None, true, Some("emp"), None, None, &crate::models::ScheduleProvenance::new(crate::models::ScheduleCreator::Owner, ""))
                .unwrap();
        }
        let refused = store
            .create_cron_job("retry-more", "16 12 16 8 10 * 2099", "", "agent", Some("x"), None, None, true, Some("emp"), None, None, &crate::models::ScheduleProvenance::new(crate::models::ScheduleCreator::Owner, ""))
            .unwrap_err();
        assert!(matches!(refused, NeboError::Validation(ref m) if m.contains("one-time schedules")), "{refused}");
        // A repeating schedule isn't a one-shot; another employee has its own count.
        store.create_cron_job("daily", "0 9 * * *", "", "agent", Some("x"), None, None, true, Some("emp"), None, None, &crate::models::ScheduleProvenance::new(crate::models::ScheduleCreator::Owner, "")).unwrap();
        store
            .create_cron_job("other-once", "16 12 16 8 10 * 2099", "", "agent", Some("x"), None, None, true, Some("other"), None, None, &crate::models::ScheduleProvenance::new(crate::models::ScheduleCreator::Owner, ""))
            .unwrap();

        for i in 0..(MAX_ACTIVE_SCHEDULES_PER_EMPLOYEE - MAX_ONE_SHOTS_PER_HOUR - 1) {
            store.create_cron_job(&format!("d-{i}"), "0 9 * * *", "", "agent", Some("x"), None, None, true, Some("emp"), None, None, &crate::models::ScheduleProvenance::new(crate::models::ScheduleCreator::Owner, "")).unwrap();
        }
        let full = store.create_cron_job("one-too-many", "0 9 * * *", "", "agent", Some("x"), None, None, true, Some("emp"), None, None, &crate::models::ScheduleProvenance::new(crate::models::ScheduleCreator::Owner, "")).unwrap_err();
        assert!(matches!(full, NeboError::Validation(ref m) if m.contains("active schedules")), "{full}");
        // A paused one doesn't count against it.
        store.create_cron_job("paused", "0 9 * * *", "", "agent", Some("x"), None, None, false, Some("emp"), None, None, &crate::models::ScheduleProvenance::new(crate::models::ScheduleCreator::Owner, "")).unwrap();
    }

    /// A schedule keeps who made it, from which run and conversation, and
    /// why; re-registering it keeps that. The run's own schedules are the
    /// ones retired with it: switched off, never deleted, and only once.
    #[test]
    fn provenance_is_kept_and_a_runs_schedules_retire_with_it() {
        use crate::models::{ScheduleCreator, ScheduleProvenance};
        let store = temp_store();
        let by_run = ScheduleProvenance::new(ScheduleCreator::Chat, "Kristi asked").in_run(Some("run-1"), "agent:ava:web");
        let mine = store.create_cron_job("mine", "0 9 * * *", "", "agent", Some("x"), None, None, true, Some("ava"), None, None, &by_run).unwrap();
        assert_eq!(
            (mine.created_by.as_str(), mine.created_by_run.as_deref(), mine.created_in.as_deref(), mine.reason.as_str()),
            ("chat", Some("run-1"), Some("agent:ava:web"), "Kristi asked")
        );
        let other = ScheduleProvenance::new(ScheduleCreator::Chat, "y").in_run(Some("run-2"), "agent:ava:web");
        let theirs = store.create_cron_job("theirs", "0 9 * * *", "", "agent", Some("x"), None, None, true, Some("ava"), None, None, &other).unwrap();
        // An edit through upsert is not a new maker.
        store
            .upsert_cron_job("mine", "0 10 * * *", "", "agent", Some("x"), None, None, true, Some("ava"), None, None, &ScheduleProvenance::new(ScheduleCreator::Owner, "edited"))
            .unwrap();
        let edited = store.get_cron_job(mine.id).unwrap().unwrap();
        assert_eq!((edited.schedule.as_str(), edited.created_by.as_str(), edited.reason.as_str()), ("0 10 * * *", "chat", "Kristi asked"));

        let retired = store.retire_schedules_made_by("run-1").unwrap();
        assert_eq!(retired.iter().map(|j| j.id).collect::<Vec<_>>(), vec![mine.id]);
        assert_eq!(store.get_cron_job(mine.id).unwrap().unwrap().enabled, Some(0), "switched off, kept");
        assert_eq!(store.get_cron_job(theirs.id).unwrap().unwrap().enabled, Some(1));
        assert!(store.retire_schedules_made_by("run-1").unwrap().is_empty(), "once");
        assert!(store.retire_schedules_made_by("").unwrap().is_empty(), "no run, nothing");
    }

    /// The engine lists the enabled schedules every tick (5 s); each job's
    /// derived columns must seek engine_runs, never scan it. Scanning it once
    /// per subquery per job outlasted the tick on a long-used desktop and
    /// held SQLite's page cache until the server stopped answering
    /// (2026-10-09).
    #[test]
    fn listing_enabled_jobs_seeks_engine_runs() {
        let store = temp_store();
        let conn = store.conn().unwrap();
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {} WHERE j.enabled = 1 ORDER BY j.name", super::JOB_SELECT))
            .unwrap();
        let plan: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let on_runs: Vec<&String> = plan.iter().filter(|step| step.contains(" r ") || step.ends_with(" r")).collect();
        assert_eq!(on_runs.len(), 3, "one step per subquery: {plan:#?}");
        for step in on_runs {
            assert!(step.starts_with("SEARCH r USING") && step.contains("idx_engine_runs_external_ref"), "{step} in {plan:#?}");
        }
    }

    /// A job's last run, run count, last error and history are its engine
    /// runs — nothing is stamped on the job row.
    #[test]
    fn derived_columns_and_history_read_from_engine_runs() {
        let store = temp_store();
        let job = store
            .create_cron_job("j", "0 0 9 * * *", "echo hi", "shell", None, None, None, true, None, None, None, &crate::models::ScheduleProvenance::new(crate::models::ScheduleCreator::Owner, ""))
            .unwrap();
        assert_eq!(job.run_count, Some(0));
        assert!(job.last_run.is_none());
        assert!(store.list_cron_history(job.id, 10, 0).unwrap().is_empty());

        let first = store.queue_cron_run(&job, false, false).unwrap();
        store.engine_set_run_state(&first, "running", 1_000, None).unwrap();
        store.engine_set_run_result(&first, "hi", None).unwrap();
        store.engine_set_run_state(&first, "done", 1_001, None).unwrap();
        let second = store.queue_cron_run(&job, true, false).unwrap();
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
