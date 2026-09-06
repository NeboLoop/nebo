//! The durable-work engine's rows: events, runs, waits, keys, effects.
//! Design of record: "One Engine for Durable Work" (2026-09-06). This module
//! is the data layer only — what is durable and what the invariants are.
//! The loop that turns rows into work lives in the server crate.
//!
//! Invariants enforced here:
//! - I-1 delivery is at-least-once under a lease; a lease that expires
//!   releases the event for redelivery, bounded by the poison cap.
//! - I-2 every event carries an idempotency key; a second insert with the
//!   same key is a duplicate, recorded and never delivered.
//! - I-3 a run resumes at most once after a process death.
//! - I-8 a key names at most one open run, enforced by a partial unique index.
//! - I-9 only the current wait generation can wake a run; an event aimed at
//!   a superseded wait is dropped.

use rusqlite::{params, OptionalExtension};

use crate::DbErrExt;
use crate::Store;
use types::NeboError;

/// An event is poisoned (stamped delivered with a note, never retried) after
/// this many claims. Same margin the wake rail settled on.
pub const EVENT_MAX_ATTEMPTS: i64 = 5;

/// How long a claim holds an event before an expired lease lets the next
/// tick redeliver it. Long enough for a real turn to start; short enough that
/// a dead process does not hold work for long.
pub const EVENT_LEASE_SECS: i64 = 120;

#[derive(Debug, Clone)]
pub struct EngineEvent {
    pub id: i64,
    pub kind: String,
    pub target_type: String,
    pub target_id: String,
    pub payload: String,
    pub channel: String,
    pub r#ref: String,
    pub idem_key: String,
    pub provenance: String,
    pub handoff_depth: i64,
    pub retention: String,
    pub due_at: Option<i64>,
    pub schedule: Option<String>,
    pub attempts: i64,
}

/// What to write for a new event. Timers set `due_at`; everything else is
/// deliverable immediately.
#[derive(Debug, Clone, Default)]
pub struct NewEvent<'a> {
    pub kind: &'a str,
    pub target_type: &'a str,
    pub target_id: &'a str,
    pub payload: &'a str,
    pub channel: &'a str,
    pub r#ref: &'a str,
    pub idem_key: &'a str,
    pub provenance: &'a str,
    pub handoff_depth: i64,
    pub durable: bool,
    pub due_at: Option<i64>,
    pub schedule: Option<&'a str>,
}

/// I-2: the outcome of an enqueue is either a new row or the news that the
/// key was already there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enqueued {
    Inserted(i64),
    Duplicate,
}

#[derive(Debug, Clone)]
pub struct EngineRun {
    pub id: String,
    pub kind: String,
    pub state: String,
    pub session_key: String,
    pub agent_id: String,
    pub lane: String,
    pub parent_run_id: Option<String>,
    pub definition: Option<String>,
    pub inputs: Option<String>,
    /// The row elsewhere this run executes as (a case turn's workflow run).
    pub external_ref: Option<String>,
    pub current_wait_id: Option<i64>,
    pub attempts: i64,
    pub resume_attempted: i64,
    pub result: Option<String>,
    pub error: Option<String>,
    pub summary: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct NewRun<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub session_key: &'a str,
    pub agent_id: &'a str,
    pub lane: &'a str,
    pub parent_run_id: Option<&'a str>,
    pub definition: Option<&'a str>,
    pub inputs: Option<&'a str>,
    pub external_ref: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct EngineWait {
    pub id: i64,
    pub run_id: String,
    pub action: String,
    pub on_kind: String,
    pub key: String,
    pub deadline: Option<i64>,
    pub parked: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct NewWait<'a> {
    /// `resume` or `trigger_child`.
    pub action: &'a str,
    /// Event kind to match, or `any`.
    pub on_kind: &'a str,
    pub key: &'a str,
    pub deadline: Option<i64>,
    pub parked: Option<&'a str>,
    pub reason: &'a str,
}

#[derive(Debug, Clone)]
pub struct EngineEffect {
    pub id: i64,
    pub run_id: String,
    pub class: String,
    pub idem_key: String,
    pub provider: String,
    pub provider_key: String,
    pub state: String,
    pub attempts: i64,
    pub provider_ref: Option<String>,
}

const RUN_COLUMNS: &str = "id, kind, state, session_key, agent_id, lane, parent_run_id, definition, inputs, external_ref, current_wait_id, attempts, resume_attempted, result, error, summary, created_at, started_at, ended_at";

fn row_to_run(r: &rusqlite::Row<'_>) -> rusqlite::Result<EngineRun> {
    Ok(EngineRun {
        id: r.get(0)?,
        kind: r.get(1)?,
        state: r.get(2)?,
        session_key: r.get(3)?,
        agent_id: r.get(4)?,
        lane: r.get(5)?,
        parent_run_id: r.get(6)?,
        definition: r.get(7)?,
        inputs: r.get(8)?,
        external_ref: r.get(9)?,
        current_wait_id: r.get(10)?,
        attempts: r.get(11)?,
        resume_attempted: r.get(12)?,
        result: r.get(13)?,
        error: r.get(14)?,
        summary: r.get(15)?,
        created_at: r.get(16)?,
        started_at: r.get(17)?,
        ended_at: r.get(18)?,
    })
}

const EVENT_COLUMNS: &str = "id, kind, target_type, target_id, payload, channel, ref, idem_key, provenance, handoff_depth, retention, due_at, schedule, attempts";

fn row_to_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<EngineEvent> {
    Ok(EngineEvent {
        id: r.get(0)?,
        kind: r.get(1)?,
        target_type: r.get(2)?,
        target_id: r.get(3)?,
        payload: r.get(4)?,
        channel: r.get(5)?,
        r#ref: r.get(6)?,
        idem_key: r.get(7)?,
        provenance: r.get(8)?,
        handoff_depth: r.get(9)?,
        retention: r.get(10)?,
        due_at: r.get(11)?,
        schedule: r.get(12)?,
        attempts: r.get(13)?,
    })
}

const WAIT_COLUMNS: &str = "id, run_id, action, on_kind, key, deadline, parked, reason";

fn row_to_wait(r: &rusqlite::Row<'_>) -> rusqlite::Result<EngineWait> {
    Ok(EngineWait {
        id: r.get(0)?,
        run_id: r.get(1)?,
        action: r.get(2)?,
        on_kind: r.get(3)?,
        key: r.get(4)?,
        deadline: r.get(5)?,
        parked: r.get(6)?,
        reason: r.get(7)?,
    })
}

impl Store {
    // ── events ─────────────────────────────────────────────────────────

    /// Write-ahead. I-2: the idempotency key is unique across all time; a
    /// repeat is reported, not inserted, and the caller never delivers it.
    pub fn engine_enqueue_event(&self, e: &NewEvent<'_>) -> Result<Enqueued, NeboError> {
        let conn = self.conn()?;
        let inserted = conn
            .execute(
                "INSERT INTO engine_events
                    (kind, target_type, target_id, payload, channel, ref, idem_key, provenance, handoff_depth, retention, due_at, schedule)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(idem_key) DO NOTHING",
                params![
                    e.kind,
                    e.target_type,
                    e.target_id,
                    e.payload,
                    e.channel,
                    e.r#ref,
                    e.idem_key,
                    e.provenance,
                    e.handoff_depth,
                    if e.durable { "durable" } else { "transient" },
                    e.due_at,
                    e.schedule,
                ],
            )
            .db_err("engine_enqueue_event")?;
        Ok(if inserted == 1 { Enqueued::Inserted(conn.last_insert_rowid()) } else { Enqueued::Duplicate })
    }

    /// I-1: claim what is deliverable now, under a lease. Deliverable means
    /// not delivered, due (or not a timer), and not held by a live lease.
    /// Claiming is the attempt: a process that dies after claiming leaves
    /// the lease to expire and the next tick claims again. Rows over the
    /// poison cap are stamped delivered with a note and never returned; the
    /// second tuple element counts them so a failure is loud, never silent.
    pub fn engine_claim_events(&self, now: i64, limit: i64) -> Result<(Vec<EngineEvent>, usize), NeboError> {
        let conn = self.conn()?;
        let poisoned = conn
            .execute(
                "UPDATE engine_events
                 SET delivered_at = ?1, note = 'poisoned: exceeded delivery attempts'
                 WHERE delivered_at IS NULL AND attempts >= ?2
                   AND (lease_until IS NULL OR lease_until < ?1)",
                params![now, EVENT_MAX_ATTEMPTS],
            )
            .db_err("engine_claim_events poison")?;
        let mut stmt = conn
            .prepare(&format!(
                "UPDATE engine_events
                 SET claimed_at = ?1, lease_until = ?1 + ?2, attempts = attempts + 1
                 WHERE id IN (
                     SELECT id FROM engine_events
                     WHERE delivered_at IS NULL
                       AND target_type != 'session'
                       AND (due_at IS NULL OR due_at <= ?1)
                       AND (lease_until IS NULL OR lease_until < ?1)
                       AND attempts < ?3
                     ORDER BY id
                     LIMIT ?4
                 )
                 RETURNING {EVENT_COLUMNS}"
            ))
            .db_err("engine_claim_events")?;
        let rows = stmt
            .query_map(params![now, EVENT_LEASE_SECS, EVENT_MAX_ATTEMPTS, limit], row_to_event)
            .db_err("engine_claim_events")?
            .collect::<Result<Vec<_>, _>>()
            .db_err("engine_claim_events")?;
        Ok((rows, poisoned))
    }

    // ── session wakes: events aimed at a session ─────────────────────────
    //
    // The wake rail (server `wake.rs`) owns these: it delivers on enqueue,
    // on run completion, and on boot, with the same write-ahead / at-most-
    // once / poison discipline it always had. The engine's global claim
    // skips them, so the two never race for one row.

    /// Write-ahead: persist a wake BEFORE any delivery attempt. Wakes have
    /// no natural idempotency key (two identical coworker replies are two
    /// replies), so each gets its own.
    pub fn engine_enqueue_wake(
        &self,
        session_key: &str,
        kind: &str,
        payload: &str,
        provenance: &str,
        handoff_depth: u8,
    ) -> Result<i64, NeboError> {
        let idem = format!("wake:{}", uuid::Uuid::new_v4());
        match self.engine_enqueue_event(&NewEvent {
            kind,
            target_type: "session",
            target_id: session_key,
            payload,
            idem_key: &idem,
            provenance,
            handoff_depth: handoff_depth as i64,
            ..Default::default()
        })? {
            Enqueued::Inserted(id) => Ok(id),
            Enqueued::Duplicate => Err(NeboError::Internal("fresh wake key collided".into())),
        }
    }

    /// Undelivered wakes for one session, FIFO, attempts bumped in the same
    /// statement — claiming IS the attempt. Rows over the poison cap are
    /// stamped delivered with a note and never returned; the second tuple
    /// element counts them so a wake may fail loudly, never loop silently.
    pub fn engine_claim_session_events(&self, session_key: &str, now: i64) -> Result<(Vec<EngineEvent>, usize), NeboError> {
        let conn = self.conn()?;
        let poisoned = conn
            .execute(
                "UPDATE engine_events
                 SET delivered_at = ?2, note = 'poisoned: exceeded delivery attempts'
                 WHERE target_type = 'session' AND target_id = ?1 AND delivered_at IS NULL AND attempts >= ?3",
                params![session_key, now, EVENT_MAX_ATTEMPTS],
            )
            .db_err("engine_claim_session_events poison")?;
        let mut stmt = conn
            .prepare(&format!(
                "UPDATE engine_events SET attempts = attempts + 1, claimed_at = ?2
                 WHERE target_type = 'session' AND target_id = ?1 AND delivered_at IS NULL
                 RETURNING {EVENT_COLUMNS}"
            ))
            .db_err("engine_claim_session_events")?;
        let mut rows = stmt
            .query_map(params![session_key, now], row_to_event)
            .db_err("engine_claim_session_events")?
            .collect::<Result<Vec<_>, _>>()
            .db_err("engine_claim_session_events")?;
        rows.sort_by_key(|e| e.id);
        Ok((rows, poisoned))
    }

    /// Stamp a batch delivered — the woken run carried these payloads.
    pub fn engine_complete_events(&self, ids: &[i64], now: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        for id in ids {
            conn.execute(
                "UPDATE engine_events SET delivered_at = ?2 WHERE id = ?1 AND delivered_at IS NULL",
                params![id, now],
            )
            .db_err("engine_complete_events")?;
        }
        Ok(())
    }

    /// Sessions with undelivered, un-poisoned wakes — the boot sweep's worklist.
    pub fn engine_sessions_with_pending(&self) -> Result<Vec<String>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT target_id FROM engine_events
                 WHERE target_type = 'session' AND delivered_at IS NULL AND attempts < ?1
                 ORDER BY target_id",
            )
            .db_err("engine_sessions_with_pending")?;
        let rows = stmt
            .query_map(params![EVENT_MAX_ATTEMPTS], |r| r.get(0))
            .db_err("engine_sessions_with_pending")?
            .collect::<Result<Vec<String>, _>>()
            .db_err("engine_sessions_with_pending")?;
        Ok(rows)
    }

    /// The claim's work is done and its observable effects are recorded.
    pub fn engine_complete_event(&self, id: i64, now: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE engine_events SET delivered_at = ?2 WHERE id = ?1",
            params![id, now],
        )
        .db_err("engine_complete_event")?;
        Ok(())
    }

    /// Dropped on purpose, and the row says why: I-9's older wait
    /// generation, a schedule that changed under its timer, a fire skipped
    /// because the last one is still running or the window was missed.
    pub fn engine_supersede_event(&self, id: i64, now: i64, note: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE engine_events SET delivered_at = ?2, note = ?3 WHERE id = ?1",
            params![id, now, note],
        )
        .db_err("engine_supersede_event")?;
        Ok(())
    }

    /// A "seen" mark: the record that something external (a hub wire
    /// message) was processed. Written already delivered, so it is never
    /// claimed; its whole job is the unique idempotency key. Returns true
    /// the first time, false on a replay.
    pub fn engine_mark_seen(&self, target_id: &str, idem_key: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let inserted = conn
            .execute(
                "INSERT INTO engine_events (kind, target_type, target_id, idem_key, delivered_at, attempts)
                 VALUES ('seen', 'entity', ?1, ?2, unixepoch(), 1)
                 ON CONFLICT(idem_key) DO NOTHING",
                params![target_id, idem_key],
            )
            .db_err("engine_mark_seen")?;
        Ok(inserted == 1)
    }

    // ── scheduled bindings: one pending timer per schedule ───────────────

    /// Pending timers aimed at one kind of target — the arming worklist.
    pub fn engine_pending_timers(&self, target_type: &str) -> Result<Vec<EngineEvent>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {EVENT_COLUMNS} FROM engine_events
                 WHERE kind = 'timer' AND target_type = ?1 AND delivered_at IS NULL
                 ORDER BY id"
            ))
            .db_err("engine_pending_timers")?;
        let rows = stmt
            .query_map(params![target_type], row_to_event)
            .db_err("engine_pending_timers")?
            .collect::<Result<Vec<_>, _>>()
            .db_err("engine_pending_timers")?;
        Ok(rows)
    }

    /// The floor the next occurrence is computed from: the last timer this
    /// target consumed (its due moment) or dropped (the moment it was
    /// dropped, so a superseded future timer never pushes the floor ahead).
    pub fn engine_last_timer_floor(&self, target_type: &str, target_id: &str) -> Result<Option<i64>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT MAX(MIN(due_at, delivered_at)) FROM engine_events
             WHERE kind = 'timer' AND target_type = ?1 AND target_id = ?2 AND delivered_at IS NOT NULL",
            params![target_type, target_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .db_err("engine_last_timer_floor")
    }

    /// Durable history of one target, oldest first, bounded.
    pub fn engine_events_for(&self, target_type: &str, target_id: &str, limit: i64) -> Result<Vec<EngineEvent>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {EVENT_COLUMNS} FROM engine_events
                 WHERE target_type = ?1 AND target_id = ?2
                 ORDER BY id DESC LIMIT ?3"
            ))
            .db_err("engine_events_for")?;
        let mut rows = stmt
            .query_map(params![target_type, target_id, limit], row_to_event)
            .db_err("engine_events_for")?
            .collect::<Result<Vec<_>, _>>()
            .db_err("engine_events_for")?;
        rows.reverse();
        Ok(rows)
    }

    /// Transient rows older than the TTL are gone; durable rows are history.
    pub fn engine_expire_transient_events(&self, older_than: i64) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM engine_events
             WHERE retention = 'transient' AND delivered_at IS NOT NULL AND delivered_at < ?1",
            params![older_than],
        )
        .db_err("engine_expire_transient_events")
    }

    // ── runs ───────────────────────────────────────────────────────────

    pub fn engine_create_run(&self, r: &NewRun<'_>) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO engine_runs (id, kind, state, session_key, agent_id, lane, parent_run_id, definition, inputs, external_ref)
             VALUES (?1, ?2, 'queued', ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![r.id, r.kind, r.session_key, r.agent_id, r.lane, r.parent_run_id, r.definition, r.inputs, r.external_ref],
        )
        .db_err("engine_create_run")?;
        Ok(())
    }

    pub fn engine_get_run(&self, id: &str) -> Result<Option<EngineRun>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            &format!("SELECT {RUN_COLUMNS} FROM engine_runs WHERE id = ?1"),
            params![id],
            row_to_run,
        )
        .optional()
        .db_err("engine_get_run")
    }

    /// Move a run between states. `running` stamps started_at; the terminal
    /// states stamp ended_at. Returns false when the row is not there.
    pub fn engine_set_run_state(&self, id: &str, state: &str, now: i64, error: Option<&str>) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let n = conn
            .execute(
                "UPDATE engine_runs
                 SET state = ?2,
                     started_at = CASE WHEN ?2 = 'running' AND started_at IS NULL THEN ?3 ELSE started_at END,
                     ended_at = CASE WHEN ?2 IN ('done','failed','cancelled') THEN ?3 ELSE ended_at END,
                     attempts = CASE WHEN ?2 = 'running' THEN attempts + 1 ELSE attempts END,
                     error = COALESCE(?4, error)
                 WHERE id = ?1",
                params![id, state, now, error],
            )
            .db_err("engine_set_run_state")?;
        Ok(n == 1)
    }

    pub fn engine_set_run_result(&self, id: &str, result: &str, summary: Option<&str>) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE engine_runs SET result = ?2, summary = COALESCE(?3, summary) WHERE id = ?1",
            params![id, result, summary],
        )
        .db_err("engine_set_run_result")?;
        Ok(())
    }

    /// Queued runs, oldest first, for one lane.
    pub fn engine_queued_runs(&self, lane: &str, limit: i64) -> Result<Vec<EngineRun>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {RUN_COLUMNS} FROM engine_runs WHERE state = 'queued' AND lane = ?1 ORDER BY created_at, id LIMIT ?2"
            ))
            .db_err("engine_queued_runs")?;
        let rows = stmt
            .query_map(params![lane, limit], row_to_run)
            .db_err("engine_queued_runs")?
            .collect::<Result<Vec<_>, _>>()
            .db_err("engine_queued_runs")?;
        Ok(rows)
    }

    /// Queued runs of one kind across every lane, oldest first.
    pub fn engine_queued_runs_of_kind(&self, kind: &str, limit: i64) -> Result<Vec<EngineRun>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {RUN_COLUMNS} FROM engine_runs WHERE state = 'queued' AND kind = ?1 ORDER BY created_at, id LIMIT ?2"
            ))
            .db_err("engine_queued_runs_of_kind")?;
        let rows = stmt
            .query_map(params![kind, limit], row_to_run)
            .db_err("engine_queued_runs_of_kind")?
            .collect::<Result<Vec<_>, _>>()
            .db_err("engine_queued_runs_of_kind")?;
        Ok(rows)
    }

    /// Running runs of one kind — the reconciliation worklist.
    pub fn engine_running_runs_of_kind(&self, kind: &str) -> Result<Vec<EngineRun>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!("SELECT {RUN_COLUMNS} FROM engine_runs WHERE state = 'running' AND kind = ?1 ORDER BY id"))
            .db_err("engine_running_runs_of_kind")?;
        let rows = stmt
            .query_map(params![kind], row_to_run)
            .db_err("engine_running_runs_of_kind")?
            .collect::<Result<Vec<_>, _>>()
            .db_err("engine_running_runs_of_kind")?;
        Ok(rows)
    }

    /// Runs executed as one row elsewhere (a cron job's fires), newest first.
    pub fn engine_runs_for_ref(&self, external_ref: &str, limit: i64, offset: i64) -> Result<Vec<EngineRun>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {RUN_COLUMNS} FROM engine_runs WHERE external_ref = ?1
                 ORDER BY created_at DESC, rowid DESC LIMIT ?2 OFFSET ?3"
            ))
            .db_err("engine_runs_for_ref")?;
        let rows = stmt
            .query_map(params![external_ref, limit, offset], row_to_run)
            .db_err("engine_runs_for_ref")?
            .collect::<Result<Vec<_>, _>>()
            .db_err("engine_runs_for_ref")?;
        Ok(rows)
    }

    pub fn engine_count_runs_for_ref(&self, external_ref: &str) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COUNT(*) FROM engine_runs WHERE external_ref = ?1",
            params![external_ref],
            |r| r.get(0),
        )
        .db_err("engine_count_runs_for_ref")
    }

    /// Is a fire of this ref still queued or running? The overlap policy
    /// (skip) asks before starting another.
    pub fn engine_has_live_run_for_ref(&self, external_ref: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM engine_runs WHERE external_ref = ?1 AND state IN ('queued', 'running'))",
            params![external_ref],
            |r| r.get::<_, bool>(0),
        )
        .db_err("engine_has_live_run_for_ref")
    }

    /// The one live child of a parent, if a turn is running or about to.
    /// One live turn per case: a signal that arrives while this exists is
    /// steered into it rather than starting another.
    pub fn engine_live_child(&self, parent_id: &str) -> Result<Option<EngineRun>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            &format!(
                "SELECT {RUN_COLUMNS} FROM engine_runs
                 WHERE parent_run_id = ?1 AND state IN ('queued', 'running')
                 ORDER BY id LIMIT 1"
            ),
            params![parent_id],
            row_to_run,
        )
        .optional()
        .db_err("engine_live_child")
    }

    /// A signal that arrived while the turn was still queued rides in its
    /// inputs, so the turn sees everything that happened before it ran.
    pub fn engine_append_pending_signal(&self, id: &str, payload: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let value = serde_json::from_str::<serde_json::Value>(payload).unwrap_or_else(|_| serde_json::Value::String(payload.to_string()));
        conn.execute(
            "UPDATE engine_runs
             SET inputs = json_set(
                     json_set(COALESCE(inputs, '{}'), '$._case.pending_signals',
                              COALESCE(json_extract(COALESCE(inputs, '{}'), '$._case.pending_signals'), json('[]'))),
                     '$._case.pending_signals[#]', json(?2))
             WHERE id = ?1",
            params![id, value.to_string()],
        )
        .db_err("engine_append_pending_signal")?;
        Ok(())
    }

    pub fn engine_set_external_ref(&self, id: &str, external_ref: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE engine_runs SET external_ref = ?2 WHERE id = ?1",
            params![id, external_ref],
        )
        .db_err("engine_set_external_ref")?;
        Ok(())
    }

    /// Boot sweep, half one: every run the dead process left `running` is
    /// stamped `interrupted` and returned for triage. Never a phantom
    /// `running` after a restart.
    pub fn engine_mark_interrupted(&self) -> Result<Vec<EngineRun>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "UPDATE engine_runs SET state = 'interrupted' WHERE state = 'running' RETURNING {RUN_COLUMNS}"
            ))
            .db_err("engine_mark_interrupted")?;
        let rows = stmt
            .query_map([], row_to_run)
            .db_err("engine_mark_interrupted")?
            .collect::<Result<Vec<_>, _>>()
            .db_err("engine_mark_interrupted")?;
        Ok(rows)
    }

    /// I-3, half two: an interrupted run gets ONE resume. The first call
    /// re-queues it and returns true; the second call for the same run fails
    /// it with the poison reason and returns false.
    pub fn engine_resume_once(&self, id: &str, now: i64) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let resumed = conn
            .execute(
                "UPDATE engine_runs SET state = 'queued', resume_attempted = 1
                 WHERE id = ?1 AND state = 'interrupted' AND resume_attempted = 0",
                params![id],
            )
            .db_err("engine_resume_once")?;
        if resumed == 1 {
            return Ok(true);
        }
        conn.execute(
            "UPDATE engine_runs SET state = 'failed', ended_at = ?2,
                    error = 'interrupted again during resume — not retried (poison-run protection)'
             WHERE id = ?1 AND state = 'interrupted'",
            params![id, now],
        )
        .db_err("engine_resume_once poison")?;
        Ok(false)
    }

    // ── waits ──────────────────────────────────────────────────────────

    /// Declare what will wake this run next. One transaction: the previous
    /// live wait is superseded, the new one becomes current, the run moves
    /// to `waiting`, and a deadline (if any) becomes a timer event aimed at
    /// the new wait's id. I-9 follows: a timer for the old generation finds
    /// its wait superseded and is dropped.
    pub fn engine_declare_wait(&self, run_id: &str, w: &NewWait<'_>, now: i64) -> Result<i64, NeboError> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().db_err("engine_declare_wait tx")?;
        tx.execute(
            "UPDATE engine_waits SET superseded_at = ?2 WHERE run_id = ?1 AND superseded_at IS NULL",
            params![run_id, now],
        )
        .db_err("engine_declare_wait supersede")?;
        tx.execute(
            "INSERT INTO engine_waits (run_id, action, on_kind, key, deadline, parked, reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![run_id, w.action, w.on_kind, w.key, w.deadline, w.parked, w.reason],
        )
        .db_err("engine_declare_wait insert")?;
        let wait_id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE engine_runs SET state = 'waiting', current_wait_id = ?2 WHERE id = ?1",
            params![run_id, wait_id],
        )
        .db_err("engine_declare_wait run")?;
        if let Some(deadline) = w.deadline {
            tx.execute(
                "INSERT INTO engine_events (kind, target_type, target_id, payload, idem_key, retention, due_at)
                 VALUES ('timer', 'wait', ?1, ?2, ?3, 'transient', ?4)",
                params![
                    wait_id.to_string(),
                    w.reason,
                    format!("wait:{wait_id}:deadline"),
                    deadline
                ],
            )
            .db_err("engine_declare_wait timer")?;
        }
        tx.commit().db_err("engine_declare_wait commit")?;
        Ok(wait_id)
    }

    pub fn engine_get_wait(&self, id: i64) -> Result<Option<EngineWait>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            &format!("SELECT {WAIT_COLUMNS} FROM engine_waits WHERE id = ?1"),
            params![id],
            row_to_wait,
        )
        .optional()
        .db_err("engine_get_wait")
    }

    /// The live wait an event should wake, if any. An event aimed at a wait
    /// matches only that wait and only while it is current (I-9). Any other
    /// event matches a live wait by kind and key.
    pub fn engine_match_wait(&self, e: &EngineEvent) -> Result<Option<EngineWait>, NeboError> {
        let conn = self.conn()?;
        if e.target_type == "wait" {
            let id: i64 = e.target_id.parse().unwrap_or(-1);
            return conn
                .query_row(
                    &format!(
                        "SELECT {WAIT_COLUMNS} FROM engine_waits w
                         WHERE w.id = ?1 AND w.superseded_at IS NULL
                           AND EXISTS (SELECT 1 FROM engine_runs r WHERE r.id = w.run_id AND r.current_wait_id = w.id AND r.state = 'waiting')"
                    ),
                    params![id],
                    row_to_wait,
                )
                .optional()
                .db_err("engine_match_wait by id");
        }
        conn.query_row(
            &format!(
                "SELECT {WAIT_COLUMNS} FROM engine_waits w
                 WHERE w.superseded_at IS NULL
                   AND (w.on_kind = ?1 OR w.on_kind = 'any')
                   AND w.key = ?2
                   AND EXISTS (SELECT 1 FROM engine_runs r WHERE r.id = w.run_id AND r.current_wait_id = w.id AND r.state = 'waiting')
                 ORDER BY w.id LIMIT 1"
            ),
            params![e.kind, e.target_id],
            row_to_wait,
        )
        .optional()
        .db_err("engine_match_wait by key")
    }

    /// A matched `resume` wait: the run itself goes back to the queue with
    /// the event's id recorded as what woke it.
    pub fn engine_resume_from_wait(&self, wait_id: i64, event_id: i64, now: i64) -> Result<(), NeboError> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().db_err("engine_resume_from_wait tx")?;
        tx.execute(
            "UPDATE engine_waits SET superseded_at = ?2 WHERE id = ?1",
            params![wait_id, now],
        )
        .db_err("engine_resume_from_wait wait")?;
        tx.execute(
            "UPDATE engine_runs SET state = 'queued', current_wait_id = NULL,
                    inputs = json_set(COALESCE(inputs, '{}'), '$.woken_by', ?2)
             WHERE current_wait_id = ?1",
            params![wait_id, event_id],
        )
        .db_err("engine_resume_from_wait run")?;
        tx.commit().db_err("engine_resume_from_wait commit")
    }

    // ── keys ───────────────────────────────────────────────────────────

    /// I-8: bind a key to a run. The partial unique index refuses a second
    /// OPEN run for the same key; that refusal is returned as `Ok(false)`,
    /// never swallowed.
    pub fn engine_bind_key(&self, run_id: &str, key_type: &str, key_value: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        match conn.execute(
            "INSERT INTO engine_run_keys (run_id, key_type, key_value) VALUES (?1, ?2, ?3)",
            params![run_id, key_type, key_value],
        ) {
            Ok(_) => Ok(true),
            Err(rusqlite::Error::SqliteFailure(err, _)) if err.code == rusqlite::ErrorCode::ConstraintViolation => Ok(false),
            Err(e) => Err(e).db_err("engine_bind_key"),
        }
    }

    /// The open run that owns this key, if any.
    pub fn engine_run_for_key(&self, key_type: &str, key_value: &str) -> Result<Option<EngineRun>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            &format!(
                "SELECT {RUN_COLUMNS} FROM engine_runs r
                 WHERE r.id = (SELECT run_id FROM engine_run_keys WHERE key_type = ?1 AND key_value = ?2 AND released_at IS NULL)"
            ),
            params![key_type, key_value],
            row_to_run,
        )
        .optional()
        .db_err("engine_run_for_key")
    }

    /// Close a run and release every key it holds, in one transaction, so a
    /// new run for the same person can open the instant this one is done.
    pub fn engine_close_run(&self, run_id: &str, state: &str, now: i64) -> Result<(), NeboError> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().db_err("engine_close_run tx")?;
        tx.execute(
            "UPDATE engine_runs SET state = ?2, ended_at = ?3, current_wait_id = NULL WHERE id = ?1",
            params![run_id, state, now],
        )
        .db_err("engine_close_run run")?;
        tx.execute(
            "UPDATE engine_waits SET superseded_at = ?2 WHERE run_id = ?1 AND superseded_at IS NULL",
            params![run_id, now],
        )
        .db_err("engine_close_run waits")?;
        tx.execute(
            "UPDATE engine_run_keys SET released_at = ?2 WHERE run_id = ?1 AND released_at IS NULL",
            params![run_id, now],
        )
        .db_err("engine_close_run keys")?;
        tx.commit().db_err("engine_close_run commit")
    }

    // ── effects ────────────────────────────────────────────────────────

    /// Pending BEFORE it acts. A repeat of the same idem_key returns the
    /// existing row's id so a retried turn finds its own effect.
    pub fn engine_effect_pending(
        &self,
        run_id: &str,
        class: &str,
        idem_key: &str,
        provider: &str,
        provider_key: &str,
    ) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO engine_effects (run_id, class, idem_key, provider, provider_key)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(idem_key) DO NOTHING",
            params![run_id, class, idem_key, provider, provider_key],
        )
        .db_err("engine_effect_pending")?;
        conn.query_row(
            "SELECT id FROM engine_effects WHERE idem_key = ?1",
            params![idem_key],
            |r| r.get(0),
        )
        .db_err("engine_effect_pending id")
    }

    pub fn engine_get_effect(&self, id: i64) -> Result<Option<EngineEffect>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, run_id, class, idem_key, provider, provider_key, state, attempts, provider_ref FROM engine_effects WHERE id = ?1",
            params![id],
            |r| {
                Ok(EngineEffect {
                    id: r.get(0)?,
                    run_id: r.get(1)?,
                    class: r.get(2)?,
                    idem_key: r.get(3)?,
                    provider: r.get(4)?,
                    provider_key: r.get(5)?,
                    state: r.get(6)?,
                    attempts: r.get(7)?,
                    provider_ref: r.get(8)?,
                })
            },
        )
        .optional()
        .db_err("engine_get_effect")
    }

    /// The provider confirmed. A completed effect is never acted on again.
    pub fn engine_effect_completed(&self, id: i64, provider_ref: Option<&str>, result: Option<&str>, now: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE engine_effects SET state = 'completed', provider_ref = ?2, result = ?3, completed_at = ?4
             WHERE id = ?1 AND state = 'pending'",
            params![id, provider_ref, result, now],
        )
        .db_err("engine_effect_completed")?;
        Ok(())
    }

    /// An attempt was made and did not confirm. Counted; the row stays
    /// pending for reconciliation.
    pub fn engine_effect_attempted(&self, id: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE engine_effects SET attempts = attempts + 1 WHERE id = ?1",
            params![id],
        )
        .db_err("engine_effect_attempted")?;
        Ok(())
    }

    pub fn engine_effect_failed(&self, id: i64, result: &str, now: i64) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE engine_effects SET state = 'failed', result = ?2, completed_at = ?3 WHERE id = ?1 AND state = 'pending'",
            params![id, result, now],
        )
        .db_err("engine_effect_failed")?;
        Ok(())
    }

    /// Recovery worklist: every pending effect, oldest first. Reconcile or
    /// retry; never assume done.
    pub fn engine_pending_effects(&self) -> Result<Vec<EngineEffect>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT id, run_id, class, idem_key, provider, provider_key, state, attempts, provider_ref FROM engine_effects WHERE state = 'pending' ORDER BY id")
            .db_err("engine_pending_effects")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(EngineEffect {
                    id: r.get(0)?,
                    run_id: r.get(1)?,
                    class: r.get(2)?,
                    idem_key: r.get(3)?,
                    provider: r.get(4)?,
                    provider_key: r.get(5)?,
                    state: r.get(6)?,
                    attempts: r.get(7)?,
                    provider_ref: r.get(8)?,
                })
            })
            .db_err("engine_pending_effects")?
            .collect::<Result<Vec<_>, _>>()
            .db_err("engine_pending_effects")?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-engine-test-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    fn signal<'a>(key: &'a str, idem: &'a str) -> NewEvent<'a> {
        NewEvent { kind: "signal", target_type: "run", target_id: key, payload: "hi", idem_key: idem, durable: true, ..Default::default() }
    }

    fn case<'a>(id: &'a str) -> NewRun<'a> {
        NewRun { id, kind: "case", session_key: "agent:a:case:x", agent_id: "a", lane: "main", ..Default::default() }
    }

    #[test]
    fn i2_duplicate_idem_key_is_reported_not_inserted() {
        let s = store();
        assert!(matches!(s.engine_enqueue_event(&signal("k", "hub:msg-1")).unwrap(), Enqueued::Inserted(_)));
        assert_eq!(s.engine_enqueue_event(&signal("k", "hub:msg-1")).unwrap(), Enqueued::Duplicate);
        let (claimed, _) = s.engine_claim_events(1_000, 10).unwrap();
        assert_eq!(claimed.len(), 1, "one row, delivered once");
    }

    #[test]
    fn i1_lease_expiry_redelivers_and_poison_cap_ends_it() {
        let s = store();
        s.engine_enqueue_event(&signal("k", "e1")).unwrap();
        let (first, _) = s.engine_claim_events(1_000, 10).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].attempts, 1, "claiming is the attempt");
        // Still leased: nothing to claim.
        assert!(s.engine_claim_events(1_000 + 10, 10).unwrap().0.is_empty());
        // Lease expired without completion: redelivered.
        let (again, _) = s.engine_claim_events(1_000 + EVENT_LEASE_SECS + 1, 10).unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].attempts, 2);
        // Completed: gone for good, even after the lease.
        s.engine_complete_event(again[0].id, 2_000).unwrap();
        assert!(s.engine_claim_events(10_000, 10).unwrap().0.is_empty());

        // Poison: a row that never completes stops after the cap, loudly.
        s.engine_enqueue_event(&signal("k", "cursed")).unwrap();
        let mut t = 20_000;
        for _ in 0..EVENT_MAX_ATTEMPTS {
            assert_eq!(s.engine_claim_events(t, 10).unwrap().0.len(), 1);
            t += EVENT_LEASE_SECS + 1;
        }
        let (none, poisoned) = s.engine_claim_events(t, 10).unwrap();
        assert!(none.is_empty());
        assert_eq!(poisoned, 1);
    }

    #[test]
    fn timers_are_deliverable_only_when_due() {
        let s = store();
        s.engine_enqueue_event(&NewEvent { kind: "timer", target_type: "wait", target_id: "7", idem_key: "t1", due_at: Some(5_000), ..Default::default() }).unwrap();
        assert!(s.engine_claim_events(4_999, 10).unwrap().0.is_empty());
        assert_eq!(s.engine_claim_events(5_000, 10).unwrap().0.len(), 1);
    }

    #[test]
    fn i3_resume_once_then_poison() {
        let s = store();
        s.engine_create_run(&case("r1")).unwrap();
        s.engine_set_run_state("r1", "running", 100, None).unwrap();
        let interrupted = s.engine_mark_interrupted().unwrap();
        assert_eq!(interrupted.len(), 1);
        assert!(s.engine_resume_once("r1", 200).unwrap(), "first resume is granted");
        assert_eq!(s.engine_get_run("r1").unwrap().unwrap().state, "queued");
        // Dies again.
        s.engine_set_run_state("r1", "running", 300, None).unwrap();
        s.engine_mark_interrupted().unwrap();
        assert!(!s.engine_resume_once("r1", 400).unwrap(), "second resume is refused");
        let r = s.engine_get_run("r1").unwrap().unwrap();
        assert_eq!(r.state, "failed");
        assert!(r.error.unwrap().contains("poison"));
    }

    #[test]
    fn i8_one_open_run_per_key_until_released() {
        let s = store();
        s.engine_create_run(&case("c1")).unwrap();
        s.engine_create_run(&case("c2")).unwrap();
        assert!(s.engine_bind_key("c1", "email", "alma@x.com").unwrap());
        assert!(!s.engine_bind_key("c2", "email", "alma@x.com").unwrap(), "the index refuses a second open case");
        assert_eq!(s.engine_run_for_key("email", "alma@x.com").unwrap().unwrap().id, "c1");
        s.engine_close_run("c1", "done", 500).unwrap();
        assert!(s.engine_run_for_key("email", "alma@x.com").unwrap().is_none(), "closing released the key");
        assert!(s.engine_bind_key("c2", "email", "alma@x.com").unwrap(), "a new case may take it now");
    }

    #[test]
    fn i9_only_the_current_wait_generation_wakes_the_run() {
        let s = store();
        s.engine_create_run(&case("c1")).unwrap();
        // Wednesday.
        let w1 = s
            .engine_declare_wait("c1", &NewWait { action: "trigger_child", on_kind: "signal", key: "email:alma@x.com", deadline: Some(3_000), reason: "until Wed", ..Default::default() }, 1_000)
            .unwrap();
        // Customer replies Tuesday; the turn declares Friday.
        let w2 = s
            .engine_declare_wait("c1", &NewWait { action: "trigger_child", on_kind: "signal", key: "email:alma@x.com", deadline: Some(5_000), reason: "until Fri", ..Default::default() }, 2_000)
            .unwrap();
        assert_ne!(w1, w2);
        assert_eq!(s.engine_get_run("c1").unwrap().unwrap().current_wait_id, Some(w2));
        // Wednesday's timer comes due: it names w1, which is superseded.
        let (due, _) = s.engine_claim_events(3_000, 10).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].target_id, w1.to_string());
        assert!(s.engine_match_wait(&due[0]).unwrap().is_none(), "old generation matches nothing");
        s.engine_supersede_event(due[0].id, 3_000, "superseded: wait generation replaced").unwrap();
        // Friday's timer names w2 and matches.
        let (fri, _) = s.engine_claim_events(5_000, 10).unwrap();
        assert_eq!(fri.len(), 1);
        let m = s.engine_match_wait(&fri[0]).unwrap().expect("current generation matches");
        assert_eq!(m.id, w2);
        assert_eq!(m.action, "trigger_child");
    }

    #[test]
    fn a_signal_matches_the_live_wait_by_kind_and_key_and_resume_requeues_the_run() {
        let s = store();
        s.engine_create_run(&case("c1")).unwrap();
        let w = s
            .engine_declare_wait("c1", &NewWait { action: "resume", on_kind: "approval", key: "approval:c1", parked: Some("[messages]"), reason: "waiting for you", ..Default::default() }, 100)
            .unwrap();
        s.engine_enqueue_event(&NewEvent { kind: "approval", target_type: "run", target_id: "approval:c1", idem_key: "approve-1", ..Default::default() }).unwrap();
        let (ev, _) = s.engine_claim_events(200, 10).unwrap();
        let m = s.engine_match_wait(&ev[0]).unwrap().expect("matches by kind+key");
        assert_eq!(m.id, w);
        assert_eq!(m.parked.as_deref(), Some("[messages]"));
        s.engine_resume_from_wait(w, ev[0].id, 200).unwrap();
        let r = s.engine_get_run("c1").unwrap().unwrap();
        assert_eq!(r.state, "queued");
        assert_eq!(r.current_wait_id, None);
        assert!(r.inputs.unwrap().contains("woken_by"));
        // The same event again would match nothing: the wait is spent.
        assert!(s.engine_match_wait(&ev[0]).unwrap().is_none());
    }

    #[test]
    fn i6_effects_are_pending_before_acting_and_a_retry_finds_its_own_row() {
        let s = store();
        s.engine_create_run(&case("c1")).unwrap();
        let id = s.engine_effect_pending("c1", "messaging", "c1:email:alma:day1", "smtp", "").unwrap();
        let again = s.engine_effect_pending("c1", "messaging", "c1:email:alma:day1", "smtp", "").unwrap();
        assert_eq!(id, again, "a retried turn finds the same pending effect");
        assert_eq!(s.engine_pending_effects().unwrap().len(), 1, "recovery sees it as unfinished");
        s.engine_effect_attempted(id).unwrap();
        s.engine_effect_completed(id, Some("msg-9"), Some("250 OK"), 300).unwrap();
        assert!(s.engine_pending_effects().unwrap().is_empty());
        let e = s.engine_get_effect(id).unwrap().unwrap();
        assert_eq!(e.state, "completed");
        assert_eq!(e.attempts, 1);
        assert_eq!(e.provider_ref.as_deref(), Some("msg-9"));
        // Completing twice is a no-op, never a second action.
        s.engine_effect_completed(id, Some("msg-10"), None, 400).unwrap();
        assert_eq!(s.engine_get_effect(id).unwrap().unwrap().provider_ref.as_deref(), Some("msg-9"));
    }

    #[test]
    fn wakes_claim_fifo_per_session_and_the_global_claim_never_touches_them() {
        let s = store();
        let a = s.engine_enqueue_wake("agent:x:web", "coworker_reply", "first", "[]", 1).unwrap();
        let b = s.engine_enqueue_wake("agent:x:web", "coworker_reply", "second", "[\"coworker\"]", 1).unwrap();
        s.engine_enqueue_wake("agent:y:web", "task_done", "other", "[]", 0).unwrap();

        assert!(s.engine_claim_events(1_000, 10).unwrap().0.is_empty(), "session wakes belong to the rail, not the loop");

        let (claimed, poisoned) = s.engine_claim_session_events("agent:x:web", 1_000).unwrap();
        assert_eq!(poisoned, 0);
        assert_eq!(claimed.iter().map(|w| w.id).collect::<Vec<_>>(), vec![a, b], "FIFO per session");
        assert_eq!(claimed[0].payload, "first");
        assert_eq!(claimed[1].provenance, "[\"coworker\"]");
        assert_eq!(claimed[0].attempts, 1, "claiming counts as the attempt");

        s.engine_complete_events(&[a, b], 1_001).unwrap();
        assert!(s.engine_claim_session_events("agent:x:web", 1_002).unwrap().0.is_empty(), "delivered = gone");
        assert_eq!(s.engine_sessions_with_pending().unwrap(), vec!["agent:y:web"]);
    }

    #[test]
    fn wakes_poison_after_max_attempts() {
        let s = store();
        s.engine_enqueue_wake("agent:z:web", "coworker_reply", "cursed", "[]", 0).unwrap();
        for round in 1..=EVENT_MAX_ATTEMPTS {
            let (claimed, _) = s.engine_claim_session_events("agent:z:web", 100 + round).unwrap();
            assert_eq!(claimed.len(), 1, "round {round} still claimable");
        }
        let (claimed, poisoned) = s.engine_claim_session_events("agent:z:web", 500).unwrap();
        assert!(claimed.is_empty());
        assert_eq!(poisoned, 1, "the failure is counted, never silent");
        assert!(s.engine_sessions_with_pending().unwrap().is_empty(), "poisoned wakes leave the worklist");
    }

    #[test]
    fn transient_events_expire_and_durable_history_stays() {
        let s = store();
        s.engine_enqueue_event(&NewEvent { kind: "timer", target_type: "wait", target_id: "1", idem_key: "t", ..Default::default() }).unwrap();
        s.engine_enqueue_event(&signal("run-1", "durable-1")).unwrap();
        let (ev, _) = s.engine_claim_events(100, 10).unwrap();
        for e in &ev {
            s.engine_complete_event(e.id, 100).unwrap();
        }
        assert_eq!(s.engine_expire_transient_events(200).unwrap(), 1, "only the timer went");
        assert_eq!(s.engine_events_for("run", "run-1", 50).unwrap().len(), 1, "the signal is history");
    }
}
