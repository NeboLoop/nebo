//! Verifications (migration 0248): each piece of verified code work and the
//! steps of its history (review rounds, the catch-up, the landing, the live
//! checks), written by the verify loop and read by run history, the chat
//! badge and the mobile app.

use rusqlite::params;
use serde::Serialize;

use crate::{DbErrExt, Store};
use types::NeboError;

/// States a verification still going is in.
pub const VERIFICATION_LIVE: [&str; 2] = ["running", "paused"];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verification {
    pub id: String,
    pub agent_id: String,
    pub session_key: String,
    pub chat_id: String,
    /// The reply the badge sits on, once the work is reported.
    pub message_id: Option<String>,
    pub workflow_run_id: Option<String>,
    pub activity_id: Option<String>,
    pub handoff_id: Option<String>,
    pub project_id: Option<String>,
    /// What was asked: the request the reviewers check the change against.
    pub spec: String,
    /// The owner's project folder.
    pub project_root: String,
    /// Where the work happens: the isolated copy, or the folder itself.
    pub work_dir: String,
    /// worktree | copy | none
    pub isolation: String,
    pub branch: String,
    pub base_ref: String,
    /// running | paused | verified | not_verified | stopped
    pub state: String,
    pub reason: String,
    pub rounds: i64,
    pub open_blockers: i64,
    pub open_majors: i64,
    pub open_minors: i64,
    pub open: serde_json::Value,
    pub deferred: serde_json::Value,
    pub summary: String,
    /// The session-key prefix of every reviewer the verification ran: its
    /// cost is theirs in the run ledger ([`Store::verification_cost`]).
    pub reviewer_prefix: String,
    /// The question a paused verification waits on; asked again when the
    /// work resumes after a restart.
    pub pending_ask: Option<serde_json::Value>,
    /// The commit the work landed as on the owner's branch; empty while it
    /// is still in its copy.
    pub landed_ref: String,
    pub landed_at: Option<i64>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

/// A verification as it starts.
#[derive(Debug, Clone, Default)]
pub struct NewVerification<'a> {
    pub id: &'a str,
    pub agent_id: &'a str,
    pub session_key: &'a str,
    pub chat_id: &'a str,
    pub workflow_run_id: Option<&'a str>,
    pub activity_id: Option<&'a str>,
    pub handoff_id: Option<&'a str>,
    pub project_id: Option<&'a str>,
    pub spec: &'a str,
    pub project_root: &'a str,
    pub work_dir: &'a str,
    pub isolation: &'a str,
    pub branch: &'a str,
    pub base_ref: &'a str,
    pub reviewer_prefix: &'a str,
}

/// What a verification's row says now: every field is written.
#[derive(Debug, Clone, Default)]
pub struct VerificationState<'a> {
    pub state: &'a str,
    pub reason: &'a str,
    pub rounds: i64,
    pub open_blockers: i64,
    pub open_majors: i64,
    pub open_minors: i64,
    pub open: serde_json::Value,
    pub deferred: serde_json::Value,
    pub summary: &'a str,
}

/// One step of a verification's history.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationRound {
    pub verification_id: String,
    pub round: i64,
    /// review | catch_up | land | live
    pub kind: String,
    pub lenses: serde_json::Value,
    pub base_ref: String,
    pub head_ref: String,
    pub diff_stat: String,
    pub findings: serde_json::Value,
    pub verified_ok: serde_json::Value,
    pub prior: serde_json::Value,
    pub checks: serde_json::Value,
    pub fixed: serde_json::Value,
    pub decisions: serde_json::Value,
    pub detail: serde_json::Value,
    /// What the step came to, in a word: clean, findings, failed, landed...
    pub outcome: String,
    /// The session-key prefix of this round's reviewers.
    pub reviewer_prefix: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

/// Which verifications [`Store::list_verifications`] reads. Every field
/// narrows; the default reads every one.
#[derive(Debug, Clone, Default)]
pub struct VerificationQuery<'a> {
    pub session_key: Option<&'a str>,
    pub chat_id: Option<&'a str>,
    pub workflow_run_id: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    /// The owner's project folder.
    pub project_root: Option<&'a str>,
    /// Only those still going.
    pub live: bool,
    /// At most this many, newest first (0 = no limit).
    pub limit: usize,
}

const COLUMNS: &str = "id, agent_id, session_key, chat_id, message_id, workflow_run_id, activity_id, handoff_id, \
     project_id, spec, project_root, work_dir, isolation, branch, base_ref, state, reason, rounds, open_blockers, \
     open_majors, open_minors, open_json, deferred_json, summary, reviewer_prefix, pending_ask_json, landed_ref, \
     landed_at, created_at, finished_at";

const ROUND_COLUMNS: &str = "verification_id, round, kind, lenses_json, base_ref, head_ref, diff_stat, findings_json, \
     verified_ok_json, prior_json, checks_json, fixed_json, decisions_json, detail_json, outcome, reviewer_prefix, \
     started_at, finished_at";

fn json(text: String) -> serde_json::Value {
    serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<Verification> {
    Ok(Verification {
        id: r.get(0)?,
        agent_id: r.get(1)?,
        session_key: r.get(2)?,
        chat_id: r.get(3)?,
        message_id: r.get(4)?,
        workflow_run_id: r.get(5)?,
        activity_id: r.get(6)?,
        handoff_id: r.get(7)?,
        project_id: r.get(8)?,
        spec: r.get(9)?,
        project_root: r.get(10)?,
        work_dir: r.get(11)?,
        isolation: r.get(12)?,
        branch: r.get(13)?,
        base_ref: r.get(14)?,
        state: r.get(15)?,
        reason: r.get(16)?,
        rounds: r.get(17)?,
        open_blockers: r.get(18)?,
        open_majors: r.get(19)?,
        open_minors: r.get(20)?,
        open: json(r.get(21)?),
        deferred: json(r.get(22)?),
        summary: r.get(23)?,
        reviewer_prefix: r.get(24)?,
        pending_ask: r.get::<_, Option<String>>(25)?.map(json),
        landed_ref: r.get(26)?,
        landed_at: r.get(27)?,
        created_at: r.get(28)?,
        finished_at: r.get(29)?,
    })
}

fn round_row(r: &rusqlite::Row) -> rusqlite::Result<VerificationRound> {
    Ok(VerificationRound {
        verification_id: r.get(0)?,
        round: r.get(1)?,
        kind: r.get(2)?,
        lenses: json(r.get(3)?),
        base_ref: r.get(4)?,
        head_ref: r.get(5)?,
        diff_stat: r.get(6)?,
        findings: json(r.get(7)?),
        verified_ok: json(r.get(8)?),
        prior: json(r.get(9)?),
        checks: json(r.get(10)?),
        fixed: json(r.get(11)?),
        decisions: json(r.get(12)?),
        detail: json(r.get(13)?),
        outcome: r.get(14)?,
        reviewer_prefix: r.get(15)?,
        started_at: r.get(16)?,
        finished_at: r.get(17)?,
    })
}

/// A JSON column's text: `null` is stored as the column's empty default.
fn text(v: &serde_json::Value, empty: &str) -> String {
    if v.is_null() {
        empty.to_string()
    } else {
        v.to_string()
    }
}

fn is_final(state: &str) -> bool {
    matches!(state, "verified" | "not_verified" | "stopped")
}

impl Store {
    pub fn create_verification(
        &self,
        v: &NewVerification<'_>,
        now: i64,
    ) -> Result<Verification, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO verifications (id, agent_id, session_key, chat_id, workflow_run_id, activity_id, handoff_id,
                 project_id, spec, project_root, work_dir, isolation, branch, base_ref, reviewer_prefix, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                v.id,
                v.agent_id,
                v.session_key,
                v.chat_id,
                v.workflow_run_id,
                v.activity_id,
                v.handoff_id,
                v.project_id,
                v.spec,
                v.project_root,
                v.work_dir,
                v.isolation,
                v.branch,
                v.base_ref,
                v.reviewer_prefix,
                now
            ],
        )
        .db_err("create_verification")?;
        drop(conn);
        self.get_verification(v.id)?.ok_or_else(|| {
            NeboError::Internal(format!("verification {} not found after insert", v.id))
        })
    }

    pub fn get_verification(&self, id: &str) -> Result<Option<Verification>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            &format!("SELECT {COLUMNS} FROM verifications WHERE id = ?1"),
            params![id],
            row,
        ) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e).db_err("get_verification"),
        }
    }

    /// Write the verification's state. A final state (verified,
    /// not_verified, stopped) sets `finished_at`; going back to running
    /// (the owner said continue) clears it.
    pub fn set_verification_state(
        &self,
        id: &str,
        s: &VerificationState<'_>,
        now: i64,
    ) -> Result<Option<Verification>, NeboError> {
        let finished = is_final(s.state).then_some(now);
        self.conn()?
            .execute(
                "UPDATE verifications SET state = ?2, reason = ?3, rounds = ?4, open_blockers = ?5, open_majors = ?6,
                     open_minors = ?7, open_json = ?8, deferred_json = ?9, summary = ?10,
                     finished_at = ?11
                 WHERE id = ?1",
                params![
                    id,
                    s.state,
                    s.reason,
                    s.rounds,
                    s.open_blockers,
                    s.open_majors,
                    s.open_minors,
                    text(&s.open, "[]"),
                    text(&s.deferred, "[]"),
                    s.summary,
                    finished
                ],
            )
            .db_err("set_verification_state")?;
        self.get_verification(id)
    }

    /// Where the work happens now (a copy made, or caught up and re-made).
    pub fn set_verification_work_dir(
        &self,
        id: &str,
        work_dir: &str,
        isolation: &str,
        branch: &str,
    ) -> Result<(), NeboError> {
        self.conn()?
            .execute(
                "UPDATE verifications SET work_dir = ?2, isolation = ?3, branch = ?4 WHERE id = ?1",
                params![id, work_dir, isolation, branch],
            )
            .db_err("set_verification_work_dir")?;
        Ok(())
    }

    /// The work landed on the owner's branch as `landed_ref`.
    pub fn set_verification_landed(
        &self,
        id: &str,
        landed_ref: &str,
        now: i64,
    ) -> Result<(), NeboError> {
        self.conn()?
            .execute(
                "UPDATE verifications SET landed_ref = ?2, landed_at = ?3 WHERE id = ?1",
                params![id, landed_ref, now],
            )
            .db_err("set_verification_landed")?;
        Ok(())
    }

    /// The reply the badge sits on.
    pub fn set_verification_message(&self, id: &str, message_id: &str) -> Result<(), NeboError> {
        self.conn()?
            .execute(
                "UPDATE verifications SET message_id = ?2 WHERE id = ?1",
                params![id, message_id],
            )
            .db_err("set_verification_message")?;
        Ok(())
    }

    /// Write one step of a verification's history (a step is written as it
    /// starts and again when it ends).
    pub fn put_verification_round(&self, r: &VerificationRound) -> Result<(), NeboError> {
        self.conn()?
            .execute(
                &format!(
                    "INSERT OR REPLACE INTO verification_rounds ({ROUND_COLUMNS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)"
                ),
                params![
                    r.verification_id,
                    r.round,
                    if r.kind.is_empty() { "review" } else { r.kind.as_str() },
                    text(&r.lenses, "[]"),
                    r.base_ref,
                    r.head_ref,
                    r.diff_stat,
                    text(&r.findings, "[]"),
                    text(&r.verified_ok, "[]"),
                    text(&r.prior, "[]"),
                    text(&r.checks, "[]"),
                    text(&r.fixed, "[]"),
                    text(&r.decisions, "[]"),
                    text(&r.detail, "{}"),
                    r.outcome,
                    r.reviewer_prefix,
                    r.started_at,
                    r.finished_at
                ],
            )
            .db_err("put_verification_round")?;
        Ok(())
    }

    /// A verification's history, in order.
    pub fn verification_rounds(&self, id: &str) -> Result<Vec<VerificationRound>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!("SELECT {ROUND_COLUMNS} FROM verification_rounds WHERE verification_id = ?1 ORDER BY round"))
            .db_err("verification_rounds")?;
        let rows = stmt
            .query_map(params![id], round_row)
            .db_err("verification_rounds")?;
        rows.collect::<Result<Vec<_>, _>>()
            .db_err("verification_rounds")
    }

    /// What the reviewers whose sessions start with `prefix` cost, from the
    /// run ledger: a verification's cost (its `reviewer_prefix`) or one
    /// round's (the round's). Microcents.
    pub fn reviewers_cost(&self, prefix: &str) -> Result<i64, NeboError> {
        if prefix.is_empty() {
            return Ok(0);
        }
        self.conn()?
            .query_row(
                "SELECT COALESCE(SUM(cost_microcents), 0) FROM run_usage
                 WHERE session_key IS NOT NULL AND substr(session_key, 1, length(?1)) = ?1",
                params![prefix],
                |r| r.get::<_, i64>(0),
            )
            .db_err("reviewers_cost")
    }

    /// The question a verification now waits on (`None`: none).
    pub fn set_verification_pending_ask(
        &self,
        id: &str,
        ask: Option<&serde_json::Value>,
    ) -> Result<(), NeboError> {
        self.conn()?
            .execute(
                "UPDATE verifications SET pending_ask_json = ?2 WHERE id = ?1",
                params![id, ask.map(|a| a.to_string())],
            )
            .db_err("set_verification_pending_ask")?;
        Ok(())
    }

    /// The verifications `q` names, newest first.
    pub fn list_verifications(
        &self,
        q: &VerificationQuery<'_>,
    ) -> Result<Vec<Verification>, NeboError> {
        let mut filters: Vec<String> = Vec::new();
        let mut args: Vec<String> = Vec::new();
        let mut by = |column: &str, value: Option<&str>| {
            if let Some(v) = value {
                args.push(v.to_string());
                filters.push(format!("{column} = ?{}", args.len()));
            }
        };
        by("session_key", q.session_key);
        by("chat_id", q.chat_id);
        by("workflow_run_id", q.workflow_run_id);
        by("agent_id", q.agent_id);
        by("project_root", q.project_root);
        if q.live {
            filters.push(format!(
                "state IN ('{}', '{}')",
                VERIFICATION_LIVE[0], VERIFICATION_LIVE[1]
            ));
        }
        let filter = if filters.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", filters.join(" AND "))
        };
        let limit = if q.limit == 0 {
            String::new()
        } else {
            format!("LIMIT {}", q.limit)
        };
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!("SELECT {COLUMNS} FROM verifications {filter} ORDER BY created_at DESC, rowid DESC {limit}"))
            .db_err("list_verifications")?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), row)
            .db_err("list_verifications")?;
        rows.collect::<Result<Vec<_>, _>>()
            .db_err("list_verifications")
    }

    /// Verifications still running when the process that ran them ended:
    /// they can never finish on their own. Each ends as stopped, with its
    /// copy kept where its row says, and every step of theirs left without
    /// an end is closed as stopped. A paused one waits on the owner: it
    /// stays paused, and its question is asked again when its work resumes.
    /// Returns how many were stopped.
    pub fn stop_orphaned_verifications(&self, booted: i64, now: i64) -> Result<usize, NeboError> {
        let conn = self.conn()?;
        let stopped = conn
            .execute(
                "UPDATE verifications SET state = 'stopped', reason = 'Nebo restarted while it was being checked',
                     finished_at = ?2
                 WHERE state = 'running' AND created_at < ?1",
                params![booted, now],
            )
            .db_err("stop_orphaned_verifications")?;
        conn.execute(
            "UPDATE verification_rounds SET finished_at = ?2, outcome = 'stopped'
             WHERE finished_at IS NULL AND started_at < ?1",
            params![booted, now],
        )
        .db_err("stop_orphaned_verifications rounds")?;
        Ok(stopped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let path =
            std::env::temp_dir().join(format!("nebo-verifications-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).unwrap()
    }

    fn new<'a>(id: &'a str, session: &'a str, root: &'a str) -> NewVerification<'a> {
        NewVerification {
            id,
            agent_id: "dev",
            session_key: session,
            chat_id: "chat-1",
            spec: "Add a total to the invoice page",
            project_root: root,
            work_dir: "/data/worktrees/site/v-1",
            isolation: "worktree",
            branch: "nebo/v-1",
            base_ref: "abc",
            ..Default::default()
        }
    }

    #[test]
    fn a_verification_keeps_its_rounds_and_ends_once_finished() {
        let s = store();
        let v = s
            .create_verification(&new("v-1", "agent:dev:web", "/site"), 10)
            .unwrap();
        assert_eq!((v.state.as_str(), v.finished_at), ("running", None));
        assert_eq!(v.open, serde_json::json!([]));

        let mut round = VerificationRound {
            verification_id: "v-1".into(),
            round: 1,
            kind: "review".into(),
            lenses: serde_json::json!(["correctness", "billing"]),
            findings: serde_json::json!([{"id": "r1-billing-1", "severity": "blocker"}]),
            started_at: 11,
            ..Default::default()
        };
        s.put_verification_round(&round).unwrap();
        round.outcome = "findings".into();
        round.finished_at = Some(20);
        s.put_verification_round(&round).unwrap();
        s.put_verification_round(&VerificationRound {
            verification_id: "v-1".into(),
            round: 2,
            kind: "land".into(),
            detail: serde_json::json!({"landedAs": "def"}),
            started_at: 30,
            ..Default::default()
        })
        .unwrap();
        let rounds = s.verification_rounds("v-1").unwrap();
        assert_eq!(rounds.len(), 2, "a step written twice is one step");
        assert_eq!(
            (rounds[0].outcome.as_str(), rounds[0].finished_at),
            ("findings", Some(20))
        );
        assert_eq!(rounds[0].verified_ok, serde_json::json!([]));
        assert_eq!(rounds[1].detail["landedAs"], "def");

        let done = s
            .set_verification_state(
                "v-1",
                &VerificationState {
                    state: "verified",
                    rounds: 2,
                    summary: "Reviewed twice.",
                    ..Default::default()
                },
                40,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            (done.state.as_str(), done.finished_at, done.rounds),
            ("verified", Some(40), 2)
        );
        assert_eq!(done.landed_ref, "", "still in its copy");
        s.set_verification_landed("v-1", "def", 41).unwrap();
        let landed = s.get_verification("v-1").unwrap().unwrap();
        assert_eq!(
            (landed.landed_ref.as_str(), landed.landed_at),
            ("def", Some(41))
        );
        assert!(
            s.list_verifications(&VerificationQuery {
                live: true,
                ..Default::default()
            })
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn work_still_going_in_a_project_is_listed_and_a_restart_stops_it() {
        let s = store();
        s.create_verification(&new("v-1", "agent:a:web", "/site"), 10)
            .unwrap();
        s.create_verification(&new("v-2", "agent:b:web", "/site"), 11)
            .unwrap();
        s.create_verification(&new("v-3", "agent:c:web", "/other"), 12)
            .unwrap();
        s.set_verification_state(
            "v-2",
            &VerificationState {
                state: "paused",
                ..Default::default()
            },
            13,
        )
        .unwrap();
        let live = s
            .list_verifications(&VerificationQuery {
                project_root: Some("/site"),
                live: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            live.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
            ["v-2", "v-1"]
        );

        assert_eq!(
            s.stop_orphaned_verifications(100, 101).unwrap(),
            2,
            "running ones stop; the paused one waits on the owner"
        );
        let v1 = s.get_verification("v-1").unwrap().unwrap();
        assert_eq!((v1.state.as_str(), v1.finished_at), ("stopped", Some(101)));
        assert_eq!(s.get_verification("v-2").unwrap().unwrap().state, "paused");
    }

    #[test]
    fn cost_comes_from_the_run_ledger_by_the_reviewers_sessions() {
        let s = store();
        let prefix = "subagent:agent:dev:web:sa-verify-v-1-";
        let mut v = new("v-1", "agent:dev:web", "/site");
        v.reviewer_prefix = prefix;
        let row = s.create_verification(&v, 10).unwrap();
        assert_eq!(row.reviewer_prefix, prefix);
        let spend = |key: &str, cost: i64| {
            s.record_run_usage(&crate::models::RunUsageEntry {
                agent_id: "dev".into(),
                session_key: Some(key.into()),
                run_id: None,
                run_type: "chat".into(),
                model_id: "m".into(),
                input_tokens: 1,
                output_tokens: 1,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cost_microcents: cost,
                outcome: None,
                exit_reason: None,
            })
            .unwrap();
        };
        spend(&format!("{prefix}r1-correctness"), 300);
        spend(&format!("{prefix}r1-security"), 200);
        spend(&format!("{prefix}r2-correctness"), 50);
        spend(
            "subagent:agent:dev:web:sa-verify-v-10-r1-correctness",
            9_999,
        );
        spend("agent:dev:web", 7_000);
        assert_eq!(
            s.reviewers_cost(prefix).unwrap(),
            550,
            "the verification's reviewers, and nobody else's"
        );
        assert_eq!(
            s.reviewers_cost(&format!("{prefix}r1-")).unwrap(),
            500,
            "one round's"
        );
        assert_eq!(s.reviewers_cost("").unwrap(), 0);
    }

    #[test]
    fn a_paused_verification_keeps_its_question_across_a_restart_and_open_steps_close() {
        let s = store();
        s.create_verification(&new("v-1", "agent:a:web", "/site"), 10)
            .unwrap();
        s.put_verification_round(&VerificationRound {
            verification_id: "v-1".into(),
            round: 1,
            started_at: 11,
            ..Default::default()
        })
        .unwrap();
        let ask = serde_json::json!({"question": "Land it anyway?", "options": ["Keep going", "Land it anyway", "Stop here"]});
        s.set_verification_pending_ask("v-1", Some(&ask)).unwrap();
        s.set_verification_state(
            "v-1",
            &VerificationState {
                state: "paused",
                ..Default::default()
            },
            12,
        )
        .unwrap();
        s.stop_orphaned_verifications(100, 101).unwrap();
        let row = s.get_verification("v-1").unwrap().unwrap();
        assert_eq!((row.state.as_str(), row.pending_ask), ("paused", Some(ask)));
        let rounds = s.verification_rounds("v-1").unwrap();
        assert_eq!(
            (rounds[0].outcome.as_str(), rounds[0].finished_at),
            ("stopped", Some(101)),
            "a step left open is closed"
        );
        s.set_verification_pending_ask("v-1", None).unwrap();
        assert_eq!(
            s.get_verification("v-1").unwrap().unwrap().pending_ask,
            None
        );
    }
}
