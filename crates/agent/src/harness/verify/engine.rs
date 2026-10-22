//! The verify loop: the end check of a turn that did code work.
//!
//! When the builder answers with no more calls, each project the turn
//! changed goes through a round: the project's own build and tests run in
//! the work's folder, then independent reviewers (one per lens, fresh, told
//! to refute) review the change: the whole change in round 1, the fixes
//! since in later rounds. Findings go back to the same builder, in the same
//! turn, as its next step; it fixes them in its copy and answers again, and
//! the next round reviews the fixes. A round with no blockers or majors,
//! passing checks, and the minors fixed or deferred with a reason is clean:
//! a copy then catches up with the owner's project, is checked again (and
//! reviewed again where catching up touched what it changed), and lands as
//! one commit. Then the builder reports to the owner from the record.
//!
//! The owner decides what only they can: a finding that is a product or
//! business call, a loop that stopped making progress (two rounds without a
//! new low in open blockers and majors, or a fixed finding back), edits of
//! theirs in the way of a landing, landing work that could not be verified.
//! A run nobody is watching can't ask: the work is then reported not
//! verified, with what it needs from them.
//!
//! The limits are everyone's: the employee's own spending limit for a run,
//! the workflow's turn budget, the prepaid balance (a review that cannot run
//! fails closed: not verified), and Stop.

use std::collections::HashSet;
use std::path::PathBuf;

use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::checks::{self, CheckRun};
use super::findings::{self, Finding, Kind, Severity};
use super::lenses::{self, Lens};
use super::review::{self, LensReview, RoundInput};
use super::shadow::{ChangeSet, Snapshot};
use super::work::{TurnWork, Work, WorkKind};
use super::Why;
use crate::harness::delegation::{Helpers, child::Parent};
use crate::harness::events::TurnEvent;
use crate::harness::turn_end::{EndCheck, EndVerdict, TurnEnd};
use crate::harness::{Harness, SeatRequest};
use crate::worktree::{self, MergeOutcome};

/// How a turn that ended before its checking did leaves its verifications.
pub enum Closed {
    /// Waiting on the owner: paused, for the turn that goes on.
    Waiting,
    /// The owner's Stop.
    Stopped,
    /// Ended otherwise: not verified, with why.
    Ended(Why),
}

/// What one turn's verify loop keeps between its rounds.
#[derive(Default)]
struct Shared {
    /// Verifications whose owner chose to land them after committing their
    /// own edits: edits still in the way then are said, not asked again.
    after_owner: HashSet<String>,
    /// The builder ran tools of its own this turn ([`Verify::mark_acted`]).
    acted: bool,
    /// Landings of each verification this turn that met a conflict the
    /// builder had not resolved: a builder that never stages a resolution
    /// stops the loop.
    conflicts: std::collections::HashMap<String, usize>,
    /// The run's tool context as its last step had it: checks and the
    /// owner's cards go through the same door, under the same permissions.
    tool_ctx: Option<tools::ToolContext>,
    /// Verifications whose builder was told to report.
    reported: HashSet<String>,
    /// Verifications whose open minors the builder was asked to fix or
    /// defer with a reason.
    minors_offered: HashSet<String>,
}

/// Who a turn's code work is for: what every work it starts records.
#[derive(Clone, Default)]
pub struct Builder {
    pub agent_id: String,
    pub session_key: String,
    pub chat_id: String,
    /// What was asked: the request the reviewers check the change against.
    pub spec: String,
    /// The owner's standing rules for the employee.
    pub owner_rules: String,
    pub workflow_run_id: Option<String>,
    pub activity_id: Option<String>,
    /// A linked coding agent's turn: it runs its own tools in its own
    /// folder, which is its work when that folder is a project.
    pub linked: bool,
}

impl Builder {
    fn starting<'a>(&'a self, store: &'a db::Store) -> super::work::Starting<'a> {
        super::work::Starting {
            store,
            agent_id: &self.agent_id,
            session_key: &self.session_key,
            chat_id: &self.chat_id,
            spec: &self.spec,
            owner_rules: &self.owner_rules,
            workflow_run_id: self.workflow_run_id.as_deref(),
            activity_id: self.activity_id.as_deref(),
        }
    }
}

/// One turn's verify loop: its works and what it keeps between rounds.
#[derive(Clone, Default)]
pub struct Verify {
    pub work: TurnWork,
    builder: Arc<Builder>,
    shared: Arc<Mutex<Shared>>,
}

/// Landings in one turn that may meet the same unresolved conflict before
/// the loop stops.
const CONFLICT_TRIES: usize = 3;

impl Shared {
    /// Count a landing of `id` that met a conflict; whether it is stuck.
    fn stuck(&mut self, id: &str) -> bool {
        let n = self.conflicts.entry(id.to_string()).or_default();
        *n += 1;
        *n >= CONFLICT_TRIES
    }
}

impl Verify {
    /// The turn's verify loop: the conversation's code work still in hand
    /// (`TurnWork::load`), and a helper's own copy when it works in one.
    pub async fn begin(store: &db::Store, builder: Builder, cwd: Option<&str>) -> Self {
        let work = TurnWork::load(&builder.starting(store), cwd, builder.linked).await;
        Self {
            work,
            builder: Arc::new(builder),
            shared: Default::default(),
        }
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The builder ran tools of its own this turn (a linked agent's calls
    /// reported in its stream, or the runner's own).
    pub fn mark_acted(&self) {
        self.shared().acted = true;
    }

    fn acted(&self) -> bool {
        self.shared().acted
    }

    /// The run's tool context at this step.
    pub fn set_tool_ctx(&self, ctx: tools::ToolContext) {
        self.shared().tool_ctx = Some(ctx);
    }

    /// Point a call at the turn's copies, starting work where it first
    /// changes a code folder (`TurnWork::route`).
    pub async fn route(
        &self,
        store: &db::Store,
        input: &mut serde_json::Value,
        cwd: &mut Option<String>,
        changes: bool,
    ) -> Result<super::work::Routed, String> {
        self.work
            .route(input, cwd, changes, &self.builder.starting(store))
            .await
    }

    /// After a call ran (`TurnWork::after_call`).
    pub async fn after_call(&self, store: &db::Store) -> Option<String> {
        self.work.after_call(&self.builder.starting(store)).await
    }

    /// The turn ended before its checking did (the owner's Stop, the step
    /// limit, a provider failure, a run left idle): every verification of it
    /// still running or waiting on the owner ends honestly, as stopped (`why`
    /// `None`, the owner's Stop) or not verified with why. Returns the ids
    /// it ended.
    pub async fn close(&self, store: &db::Store, why: Closed) -> Vec<String> {
        let mut ended = Vec::new();
        for work in self.work.snapshot().await {
            let Some(v) = store.get_verification(&work.id).ok().flatten() else { continue };
            if !matches!(v.state.as_str(), "running" | "paused") {
                continue;
            }
            let (state, reason) = match &why {
                // Waiting on the owner (an approval, a question): paused,
                // and the turn that goes on picks it up.
                Closed::Waiting => {
                    if v.state == "running" {
                        let _ = store.set_verification_state(&v.id, &state_of(&v, "paused", &v.reason), chrono::Utc::now().timestamp());
                        ended.push(v.id.clone());
                    }
                    continue;
                }
                Closed::Stopped => ("stopped", String::new()),
                Closed::Ended(w) => ("not_verified", w.text()),
            };
            let _ = store.set_verification_pending_ask(&v.id, None);
            let _ = store.set_verification_state(&v.id, &state_of(&v, state, &reason), chrono::Utc::now().timestamp());
            Work::clean_up_reviewers(&v.id);
            ended.push(v.id.clone());
        }
        ended
    }

    /// Save the work in this turn's copies where it outlives them
    /// (`worktree::save`): the system may clear them before the next turn.
    pub async fn save_copies(&self) {
        for work in self.work.snapshot().await {
            if let Some(iso) = &work.iso
                && let Err(e) = worktree::save(iso).await
            {
                tracing::warn!(verification = %work.id, error = %e, "a copy's work could not be saved");
            }
        }
    }

    /// The verifications whose builder was told to report this turn: the
    /// turn's last reply is their badge's.
    pub fn reported(&self) -> Vec<String> {
        self.shared().reported.iter().cloned().collect()
    }
}

/// What the verify check of a turn runs with.
pub struct VerifyCheck {
    pub harness: Harness,
    pub helpers: Arc<Helpers>,
    pub verify: Verify,
    pub session_key: String,
    pub agent_id: String,
    pub seat: SeatRequest,
    pub grant: types::permissions::Grant,
    pub platform: Option<String>,
    pub cancel: CancellationToken,
    /// The untrusted content the turn read: its reviewers carry it.
    pub taint: Vec<types::provenance::ProvenanceClass>,
}

/// What a question of the loop's is about, kept with it on the row: the
/// answer does the same thing however late it comes (a restart asks the
/// question again).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Purpose {
    /// A product or business call a reviewer found.
    Decision { title: String },
    /// The loop stopped making progress with `open` blocking issues.
    Stalled { open: usize },
    /// Reviewed, but no build or test could run.
    NoChecks,
    /// The owner's own edits are in the way of landing verified work.
    OwnerEdits,
}

const KEEP_GOING: &str = "Keep going";
const LAND_ANYWAY: &str = "Land it anyway";
const STOP_HERE: &str = "Stop here";
const KEEP_IN_COPY: &str = "Keep it in its copy";
const LAND_AFTER_MINE: &str = "Land it after I commit mine";
const KEEP_MINE: &str = "Keep mine, don't land it";

/// Where one work's round left it.
enum Step {
    /// The builder has more to do: what it is told.
    Continue(String),
    /// Finished (verified or not, landed or not); its row says how.
    Finished,
    /// Nothing changed after all: the work and its row are gone.
    Gone,
}

/// A finished verification's state, as its row says it.
fn finished_state(state: &str) -> bool {
    matches!(state, "verified" | "not_verified" | "stopped")
}

#[async_trait::async_trait]
impl EndCheck for VerifyCheck {
    fn name(&self) -> &'static str {
        "verify"
    }

    async fn check(&self, end: &TurnEnd<'_>) -> EndVerdict {
        // A linked agent's project counts as its work only when the agent
        // itself ran tools this turn: the owner's own edits meanwhile are
        // not its change.
        if self.verify.acted() {
            self.verify.work.start_folder_if_changed(&self.verify.builder.starting(&self.harness.store)).await;
        } else {
            self.verify.work.drop_folder().await;
        }
        let works = self.verify.work.snapshot().await;
        if works.is_empty() {
            return EndVerdict::Stop;
        }
        // The project's checks and the reviewers take minutes with no model
        // step: the run is waiting, never idle.
        let ctx = self.tool_ctx();
        let _waiting = ctx.waiting.enter();
        let notes = end
            .transcript
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let mut told: Vec<String> = Vec::new();
        for work in works {
            let id = work.id.clone();
            match self.step(work, &notes).await {
                Step::Continue(text) => told.push(text),
                Step::Finished | Step::Gone => {}
            }
            self.harness.verification_changed(&id);
        }
        if !told.is_empty() {
            return EndVerdict::Continue(TurnEvent::Verify(told.join("\n\n")));
        }
        // Everything is settled: the builder reports, once, from the record.
        let store = &self.harness.store;
        let unreported: Vec<db::Verification> = self
            .verify
            .work
            .snapshot()
            .await
            .iter()
            .filter_map(|w| store.get_verification(&w.id).ok().flatten())
            // Reported once: the row keeps the reply that reported it (its
            // badge), so a later turn never reports it again.
            .filter(|v| v.message_id.is_none() && !self.verify.shared().reported.contains(&v.id))
            .collect();
        if unreported.is_empty() {
            return EndVerdict::Stop;
        }
        let mut reported = self.verify.shared();
        for v in &unreported {
            reported.reported.insert(v.id.clone());
        }
        drop(reported);
        let records: Vec<String> = unreported
            .iter()
            .map(|v| record_for_report(store, v))
            .collect();
        EndVerdict::Continue(TurnEvent::Verify(format!(
            "The checking is done. Now report to the owner in plain words: what you built; what each round of review \
             caught and what you fixed; what is verified and what is not (never more than the record below says); \
             whether it landed in their project or waits in its copy; and anything that needs them. If the work \
             deploys or releases, wait until it is live and check its health, logs and data before you say it works, \
             and say what you checked.\n\n{}",
            records.join("\n\n")
        )))
    }
}

/// The riskiest of `lenses`: the one a later round reads the fixes through,
/// beside correctness.
fn riskiest(lenses: &[Lens]) -> Lens {
    [
        Lens::Security,
        Lens::Billing,
        Lens::Data,
        Lens::Performance,
        Lens::Experience,
    ]
    .into_iter()
    .find(|l| lenses.contains(l))
    .unwrap_or(Lens::Correctness)
}

/// How a finished verification reads in the builder's report brief.
fn record_for_report(store: &db::Store, v: &db::Verification) -> String {
    let rounds = store.verification_rounds(&v.id).unwrap_or_default();
    let mut out = format!(
        "Project {}: {}{}.",
        v.project_root,
        match v.state.as_str() {
            "verified" => "VERIFIED",
            _ => "NOT VERIFIED",
        },
        match Why::read(&v.reason) {
            Some(why) => format!(" ({})", super::english(&why)),
            None => String::new(),
        }
    );
    let said = |r: &db::VerificationRound| Why::read(&r.outcome).map(|w| super::english(&w)).unwrap_or_default();
    for r in &rounds {
        let findings: Vec<Finding> = serde_json::from_value(r.findings.clone()).unwrap_or_default();
        let checks: Vec<CheckRun> = serde_json::from_value(r.checks.clone()).unwrap_or_default();
        match r.kind.as_str() {
            "review" => out.push_str(&format!(
                "\n- Round {}: {} reviewer(s) ({}); {} finding(s) ({} blocking); checks: {}.",
                r.round,
                r.lenses.as_array().map(|a| a.len()).unwrap_or(0),
                r.lenses
                    .as_array()
                    .map(|a| a
                        .iter()
                        .filter_map(|l| l.as_str())
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default(),
                findings.len(),
                findings.iter().filter(|f| f.severity.blocks()).count(),
                if checks.is_empty() {
                    "none could run".to_string()
                } else {
                    checks
                        .iter()
                        .map(|c| {
                            format!("{} {}", c.label, if c.passed { "passed" } else { "failed" })
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            )),
            "catch_up" => out.push_str(&format!("\n- Caught up with the project: {}.", said(r))),
            "land" => out.push_str(&format!("\n- Landing: {}.", said(r))),
            "live" => out.push_str(&format!("\n- Live check: {}.", said(r))),
            _ => {}
        }
    }
    let open: Vec<Finding> = serde_json::from_value(v.open.clone()).unwrap_or_default();
    for f in open.iter().filter(|f| f.severity.blocks()) {
        out.push_str(&format!(
            "\n- Still open [{}]: {} ({})",
            severity_word(f.severity),
            f.title,
            f.evidence
        ));
    }
    if v.landed_ref.is_empty() {
        out.push_str(&format!(
            "\n- Not in the project yet: the work is in its copy at {}.",
            v.work_dir
        ));
    } else {
        out.push_str(&format!(
            "\n- Landed in the project as one commit {}.",
            &v.landed_ref[..v.landed_ref.len().min(10)]
        ));
    }
    out
}

fn severity_word(s: Severity) -> &'static str {
    match s {
        Severity::Blocker => "blocker",
        Severity::Major => "major",
        Severity::Minor => "minor",
    }
}

/// What the builder is told after a round with findings.
fn findings_brief(
    project: &str,
    round: u32,
    lenses: &[Lens],
    open: &[Finding],
    checks: &[CheckRun],
    decided: &[String],
) -> String {
    let mut out = format!(
        "Independent reviewers checked your change to {project} (round {round}: {}). It isn't done yet. Fix every blocker \
         and major below in your working copy; a blocker or major can't be deferred. For each minor, fix it or defer it \
         with a reason. End your reply with one line per finding: `<id>: fixed: what you changed`, `<id>: deferred: why` \
         (minors only), or `<id>: disputed: why it is not a defect` (and leave that code as it is). The next round's \
         reviewers check each line against the code.",
        lenses
            .iter()
            .map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    for f in open {
        out.push_str(&format!(
            "\n- {} [{}] {}: {}\n  Repro: {}\n  Impact: {}\n  Fix: {}",
            f.id,
            severity_word(f.severity),
            f.title,
            f.evidence,
            f.repro,
            f.impact,
            f.fix
        ));
    }
    let failed: Vec<&CheckRun> = checks.iter().filter(|c| !c.passed).collect();
    if !failed.is_empty() {
        out.push_str("\n\nThe project's own checks failed (blockers):");
        for c in failed {
            out.push_str(&format!(
                "\n- {} ({}):\n```\n{}\n```",
                c.label, c.command, c.tail
            ));
        }
    }
    for d in decided {
        out.push_str(&format!("\n\n{d}"));
    }
    out
}

impl VerifyCheck {
    fn store(&self) -> &Arc<db::Store> {
        &self.harness.store
    }

    /// Make a round's folder for its reviewers: the copy of `head` and the
    /// patch, under the verification's folder with the working copies.
    /// Returns that folder.
    async fn review_folder(&self, id: &str, round: u32, reviewed: &ChangeSet, work: &Work, head: &Snapshot) -> Result<PathBuf, String> {
        let scratch = Work::copies(id)?;
        let round_dir = review::round_dir(&scratch, round);
        let _ = std::fs::remove_dir_all(&round_dir);
        std::fs::create_dir_all(&round_dir).map_err(|e| format!("create {}: {e}", round_dir.display()))?;
        // The lenses' one build folder is made here, before any of them
        // runs: a confined command reaches only folders that exist when it
        // starts (Linux binds them into an empty, read-only root).
        let build = review::build_dir(&scratch);
        std::fs::create_dir_all(&build).map_err(|e| format!("create {}: {e}", build.display()))?;
        let patch = review::patch_file(&round_dir);
        std::fs::write(&patch, &reviewed.patch).map_err(|e| format!("write {}: {e}", patch.display()))?;
        work.shadow.export(head, &review::project_dir(&round_dir)).await?;
        Ok(scratch)
    }

    /// A round whose reviewers could not be given what they read ends the
    /// verification unreviewed: nothing passes on a review that didn't
    /// happen.
    fn review_incomplete(&self, v: &db::Verification, row: &mut db::VerificationRound, error: &str) -> RoundEnd {
        tracing::warn!(verification = %v.id, error = %error, "the reviewers' copy could not be made");
        row.outcome = Why::new("review_incomplete").text();
        row.finished_at = Some(Self::now());
        let _ = self.store().put_verification_round(row);
        self.finish(v, "not_verified", Some(Why::new("review_incomplete")));
        RoundEnd::Finished
    }

    fn tool_ctx(&self) -> tools::ToolContext {
        self.verify.shared().tool_ctx.clone().unwrap_or_default()
    }

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    /// Ask the owner one question with choices, the recommended one first,
    /// on the conversation's card (and the mobile app). `None` when nobody
    /// can answer in this run. The question waits on the row while it is
    /// asked, so a restart asks it again. A question with no choices is
    /// never asked: the owner would have nothing to pick but Skip.
    async fn ask_owner(
        &self,
        id: &str,
        question: &str,
        context: &str,
        options: &[&str],
        purpose: &Purpose,
    ) -> Option<String> {
        if options.iter().all(|o| o.trim().is_empty()) {
            tracing::error!(verification = %id, question, "a question for the owner with no choices was not asked");
            return None;
        }
        let ask = json!({ "question": question, "context": context, "options": options, "purpose": purpose });
        let store = self.store();
        let _ = store.set_verification_pending_ask(id, Some(&ask));
        let before = store.get_verification(id).ok().flatten();
        if let Some(v) = &before {
            let _ =
                store.set_verification_state(id, &state_of(v, "paused", &v.reason), Self::now());
            self.harness.verification_changed(id);
        }
        let mut call = ask.clone();
        if let Some(o) = call.as_object_mut() {
            o.remove("purpose");
        }
        let result = self
            .harness
            .tools
            .execute(&self.tool_ctx(), "ask_owner", call)
            .await;
        let _ = store.set_verification_pending_ask(id, None);
        if let Some(v) = store.get_verification(id).ok().flatten() {
            let _ =
                store.set_verification_state(id, &state_of(&v, "running", &v.reason), Self::now());
        }
        if result.is_error {
            return None;
        }
        let answer: serde_json::Value = serde_json::from_str(&result.content).unwrap_or_default();
        answer["picked"]
            .as_str()
            .or(answer["own_words"].as_str())
            .or(answer["response"].as_str())
            .map(str::to_string)
    }

    /// The builder's turn as the parent of its reviewers.
    fn parent(&self) -> Parent<'_> {
        Parent {
            session_key: &self.session_key,
            seat: &self.seat,
            grant: Some(&self.grant),
            run_taint: &self.taint,
            cancel: self.cancel.child_token(),
            platform: self.platform.as_deref(),
        }
    }

    /// The owner's Stop came while a round's checks or reviewers ran: the
    /// round and the verification end as stopped.
    fn stopped(&self, v: &db::Verification, row: &mut db::VerificationRound) -> RoundEnd {
        row.finished_at = Some(Self::now());
        let _ = self.store().put_verification_round(row);
        self.finish(v, "stopped", None);
        RoundEnd::Finished
    }

    /// Write a round's record as it stands and tell the apps.
    fn record_round(&self, row: &db::VerificationRound) {
        let _ = self.store().put_verification_round(row);
        self.harness.verification_changed(&row.verification_id);
    }

    /// Finish a verification: its state and why written (a code the apps
    /// word), its copy kept or landed as the caller left it.
    fn finish(&self, v: &db::Verification, state: &str, why: Option<Why>) {
        let reason = why.map(|w| w.text()).unwrap_or_default();
        let _ =
            self.store()
                .set_verification_state(&v.id, &state_of(v, state, &reason), Self::now());
        // Its reviewers are done: their copies and shared build folder go.
        Work::clean_up_reviewers(&v.id);
    }

    /// One work's next step.
    async fn step(&self, mut work: Work, notes: &str) -> Step {
        let store = self.store().clone();
        let Some(mut v) = store.get_verification(&work.id).ok().flatten() else {
            return Step::Gone;
        };
        if finished_state(&v.state) {
            // Finished, but its copy changed since (the builder kept working
            // in it): checked again over the whole change, and the verdict
            // it was reported on no longer holds.
            let changed = v.landed_ref.is_empty()
                && v.state != "stopped"
                && work.work_dir.is_dir()
                && match work.shadow.snapshot().await {
                    Ok(now) => Some(&now) != work.last.as_ref(),
                    Err(_) => false,
                };
            if !changed {
                return Step::Finished;
            }
            let _ = store.reopen_verification(&v.id);
            match store.get_verification(&v.id).ok().flatten() {
                Some(reopened) => v = reopened,
                None => return Step::Gone,
            }
        }
        // A question a restart left unanswered is asked again.
        if v.state == "paused"
            && let Some(ask) = v.pending_ask.clone()
        {
            let options: Vec<String> = ask["options"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|o| o.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let opts: Vec<&str> = options.iter().map(String::as_str).collect();
            let purpose = serde_json::from_value::<Purpose>(ask["purpose"].clone()).ok();
            let answer = self
                .ask_owner(
                    &v.id,
                    ask["question"].as_str().unwrap_or(""),
                    ask["context"].as_str().unwrap_or(""),
                    &opts,
                    purpose.as_ref().unwrap_or(&Purpose::NoChecks),
                )
                .await;
            // The answer does what it did when it was first asked.
            match &purpose {
                Some(p) => {
                    if let Some(step) = self.answered(&mut work, &v, p, answer.as_deref()).await {
                        return step;
                    }
                }
                None if answer.is_none() => {
                    self.finish(&v, "not_verified", Some(Why::new("needs_answer")));
                    return Step::Finished;
                }
                None => {}
            }
        }
        let head = match work.shadow.snapshot().await {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(verification = %v.id, error = %e, "what changed could not be read");
                self.finish(&v, "not_verified", Some(Why::new("unreadable")));
                return Step::Finished;
            }
        };
        // Nothing changed since a clean review (no blocking issue, no call
        // of the owner's to carry out), under the same rules: its verdict
        // stands and it goes on to land; no reviewer reads the same bytes
        // again.
        if v.rounds > 0 && work.last.as_ref() == Some(&head) {
            let rules = rules_hash(&self.review_rules(&work).await);
            let clean = store
                .verification_rounds(&v.id)
                .unwrap_or_default()
                .iter()
                .rev()
                .find(|r| r.kind == "review")
                .is_some_and(|r| {
                    r.detail["openBlocking"].as_u64() == Some(0)
                        && r.decisions.as_array().is_none_or(|d| d.is_empty())
                        && r.detail["rulesHash"].as_str() == Some(rules.as_str())
                });
            if clean {
                return self.land(work, notes).await;
            }
        }
        let whole = work
            .shadow
            .diff(&work.base, &head)
            .await
            .unwrap_or_default();
        if whole.is_empty() && v.rounds == 0 {
            // Nothing changed after all: no verification to show.
            if let Some(iso) = work.iso.as_mut() {
                worktree::put_away(iso, "No changes", false).await;
            }
            let _ = store.delete_verification(&v.id);
            work.clean_up();
            self.verify.work.remove(&v.id).await;
            return Step::Gone;
        }

        // The employee's own spending limit for a run holds here too.
        let cap_cents = store.agent_run_spend_cap_cents(&self.agent_id);
        if cap_cents > 0
            && store.reviewers_cost(&v.reviewer_prefix).unwrap_or(0) >= cap_cents * 1_000_000
        {
            self.finish(&v, "not_verified", Some(Why::new("spend_limit")));
            return Step::Finished;
        }

        let round = v.rounds as u32 + 1;
        let delta_from = work.last.clone().unwrap_or_else(|| work.base.clone());
        let reviewed = if round == 1 {
            whole.clone()
        } else {
            work.shadow
                .diff(&delta_from, &head)
                .await
                .unwrap_or_default()
        };
        let first = store
            .verification_rounds(&v.id)
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.kind == "review");
        let lenses: Vec<Lens> = match &first {
            None => lenses::assess(&whole).lenses,
            Some(r) => {
                let all: Vec<Lens> = r
                    .lenses
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|l| l.as_str().and_then(Lens::parse))
                    .collect();
                // The fixes are read through correctness and the riskiest
                // lens, of the first round's and of what the fixes touch.
                let mut all = all;
                all.extend(lenses::assess(&reviewed).lenses);
                let mut later = vec![Lens::Correctness];
                if riskiest(&all) != Lens::Correctness {
                    later.push(riskiest(&all));
                }
                later
            }
        };
        let nested = work.shadow.nested_repositories().await;
        match self
            .review_round(
                &mut work, &v, round, &lenses, &whole, &reviewed, &head, notes, &nested,
            )
            .await
        {
            RoundEnd::Continue(text) => Step::Continue(text),
            RoundEnd::Finished => Step::Finished,
            RoundEnd::Clean => self.land(work, notes).await,
        }
    }
}

/// How a review round ended.
enum RoundEnd {
    Continue(String),
    Finished,
    /// No blockers or majors, the checks passed, the minors settled.
    Clean,
}

impl From<Step> for RoundEnd {
    fn from(step: Step) -> Self {
        match step {
            Step::Continue(text) => RoundEnd::Continue(text),
            Step::Finished | Step::Gone => RoundEnd::Finished,
        }
    }
}

/// `v`'s state as it is, with `state` and `reason`.
fn state_of<'a>(
    v: &'a db::Verification,
    state: &'a str,
    reason: &'a str,
) -> db::VerificationState<'a> {
    let open: Vec<Finding> = serde_json::from_value(v.open.clone()).unwrap_or_default();
    db::VerificationState {
        state,
        reason,
        rounds: v.rounds,
        open_blockers: open
            .iter()
            .filter(|f| f.severity == Severity::Blocker)
            .count() as i64,
        open_majors: open
            .iter()
            .filter(|f| f.severity == Severity::Major)
            .count() as i64,
        open_minors: open
            .iter()
            .filter(|f| f.severity == Severity::Minor)
            .count() as i64,
        open: v.open.clone(),
        deferred: v.deferred.clone(),
        summary: &v.summary,
    }
}

impl VerifyCheck {
    /// One round: the project's checks, then the reviewers; its record
    /// written; what the builder, or the owner, does next.
    #[allow(clippy::too_many_arguments)]
    async fn review_round(
        &self,
        work: &mut Work,
        v: &db::Verification,
        round: u32,
        lenses: &[Lens],
        whole: &ChangeSet,
        reviewed: &ChangeSet,
        head: &Snapshot,
        notes: &str,
        nested: &[String],
    ) -> RoundEnd {
        let store = self.store().clone();
        let started = Self::now();
        let prefix = format!("{}r{round}-", v.reviewer_prefix);
        // Its step in the history: after every step so far (a landing's
        // included), never over one.
        let at = next_step(&store, &v.id);
        let mut row = db::VerificationRound {
            verification_id: v.id.clone(),
            round: at,
            kind: "review".into(),
            lenses: json!(lenses.iter().map(|l| l.as_str()).collect::<Vec<_>>()),
            base_ref: work.last.as_ref().unwrap_or(&work.base).0.clone(),
            head_ref: head.0.clone(),
            diff_stat: reviewed.stat(),
            reviewer_prefix: prefix,
            started_at: started,
            ..Default::default()
        };
        // The plain-code step: the project's own build and tests, within
        // the build folders' disk cap. The round's record says, as it goes,
        // which checks it runs and what each came to, then each reviewer's
        // result as it arrives: the owner watches the round, not a spinner.
        super::work::cap_builds();
        let checks = checks::detect(&work.work_dir, &whole.paths());
        row.detail = json!({
            "whole": whole.stat(),
            "checksPlanned": checks.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(),
        });
        self.record_round(&row);
        let mut ctx = self.tool_ctx();
        ctx.cwd = Some(work.work_dir.to_string_lossy().into_owned());
        let build = match checks::build_cache(&work.project_root) {
            Ok(build) => build,
            Err(e) => return self.review_incomplete(v, &mut row, &e),
        };
        let mut runs: Vec<CheckRun> = Vec::with_capacity(checks.len());
        for check in &checks {
            runs.extend(checks::run(&self.harness.tools, &ctx, std::slice::from_ref(check), &build).await);
            row.checks = json!(runs);
            self.record_round(&row);
            if self.cancel.is_cancelled() {
                break;
            }
        }
        if self.cancel.is_cancelled() {
            return self.stopped(v, &mut row);
        }

        // The reviewers: fresh, one per lens, all at once, each reading the
        // round's copy of exactly what was snapshotted (a throwaway export
        // of `head`) and its patch, sharing the verification's one build
        // folder, all with the working copies (`Work::copies`): nothing a reviewer runs can
        // change the builder's copy, the one that lands.
        let scratch = match self.review_folder(&v.id, round, reviewed, work, head).await {
            Ok(scratch) => scratch,
            Err(e) => return self.review_incomplete(v, &mut row, &e),
        };
        let round_dir = review::round_dir(&scratch, round);
        let (patch_file, review_dir) = (review::patch_file(&round_dir), review::project_dir(&round_dir));
        let prior_open: Vec<Finding> = serde_json::from_value(v.open.clone()).unwrap_or_default();
        let rules = self.review_rules(work).await;
        let input = RoundInput {
            verification_id: &v.id,
            round,
            delta: round > 1,
            lenses,
            spec: &v.spec,
            change: reviewed,
            whole_stat: &whole.stat(),
            patch_file: &patch_file,
            work_dir: &review_dir,
            scratch: &scratch,
            checks: &runs,
            prior_open: &prior_open,
            claims: &if round > 1 { findings::claims(notes, &prior_open) } else { Vec::new() },
            check_files: &review::check_files(&reviewed.paths()),
            rules: &rules,
        };
        let mut reviewers = serde_json::Map::new();
        for lens in lenses {
            reviewers.insert(lens.as_str().into(), json!("running"));
        }
        row.detail["reviewers"] = json!(reviewers);
        self.record_round(&row);
        let live = Mutex::new(row.clone());
        let heard = |r: &LensReview| {
            let mut live = live.lock().unwrap_or_else(|p| p.into_inner());
            let state = match &r.review {
                Ok(review) => {
                    append(&mut live.findings, json!(review.findings));
                    append(&mut live.verified_ok, json!(review.verified_ok));
                    if review.findings.is_empty() { "clean" } else { "found" }
                }
                Err(_) => "failed",
            };
            live.detail["reviewers"][r.lens.as_str()] = json!(state);
            self.record_round(&live);
        };
        let reviews: Vec<LensReview> =
            review::run_round(&self.helpers, &self.parent(), &input, &heard).await;
        row = live.into_inner().unwrap_or_else(|p| p.into_inner());
        let _ = std::fs::remove_dir_all(&round_dir);
        if self.cancel.is_cancelled() {
            return self.stopped(v, &mut row);
        }
        // What was reviewed and checked must be what is in the copy now: a
        // check that rewrote a file (a lockfile, generated code) changed it.
        let drifted = match work.shadow.snapshot().await {
            Ok(now) if &now != head => Some(work.shadow.diff(head, &now).await.map(|c| c.stat()).unwrap_or_default()),
            Ok(_) => None,
            Err(e) => Some(format!("the copy could not be read again: {e}")),
        };

        // A reviewer that couldn't finish (stopped, failed, no balance)
        // leaves the round unreviewed: nothing passes on a review that
        // didn't happen.
        let failed: Vec<String> = reviews
            .iter()
            .filter_map(|r| {
                r.review
                    .as_ref()
                    .err()
                    .map(|e| format!("{}: {e}", r.lens.as_str()))
            })
            .collect();
        let mut new: Vec<Finding> = Vec::new();
        let mut held = Vec::new();
        let mut priors = Vec::new();
        for r in reviews.iter().filter_map(|r| r.review.as_ref().ok()) {
            new.extend(r.findings.iter().cloned());
            held.extend(r.verified_ok.iter().cloned());
            priors.extend(r.prior.iter().cloned());
        }
        // Progress, against the best so far.
        let earlier = store.verification_rounds(&v.id).unwrap_or_default();
        // The owner's calls already made, by the finding that asked each: the
        // same call is never asked twice, and one that keeps coming back
        // counts against progress.
        let asked_before: Vec<(Finding, String)> = earlier
            .iter()
            .filter(|r| r.round < at)
            .flat_map(|r| r.decisions.as_array().cloned().unwrap_or_default())
            .filter_map(|d| Some((serde_json::from_value::<Finding>(d["finding"].clone()).ok()?, d["answer"].as_str()?.to_string())))
            .collect();
        // Settled only now, against the calls he already made: one raised
        // again without its choices still carries his answer.
        let answered: Vec<Finding> = asked_before.iter().map(|(f, _)| f.clone()).collect();
        findings::settle(&mut new, &answered);
        let (decisions, defects): (Vec<Finding>, Vec<Finding>) =
            new.iter().cloned().partition(|f| f.kind == Kind::Decision);
        let open = findings::open_after(&prior_open, &priors, &defects);
        let fixed: Vec<String> = priors
            .iter()
            .filter(|p| p.state == findings::PriorState::Fixed)
            .map(|p| p.id.clone())
            .collect();
        let deferred = findings::accepted_deferrals(&prior_open, &priors);
        let checks_failed = runs.iter().filter(|r| !r.passed).count();
        let blocking_now = findings::blocking(&open) + checks_failed;

        let mut calls: Vec<Finding> = Vec::new();
        for d in decisions {
            if !calls.iter().any(|c| c.same_call(&d)) {
                calls.push(d);
            }
        }
        let repeated = calls.iter().filter(|d| asked_before.iter().any(|(f, _)| f.same_call(d))).count();
        let mut counts = progress_counts(&earlier, at);
        // A round the checks rewrote can't pass: it counts against progress
        // like an open issue, so checks that rewrite differently every run
        // stop the loop.
        let progress_count = blocking_now + repeated + usize::from(drifted.is_some());
        counts.push(progress_count);
        let fixed_findings: Vec<Finding> = earlier
            .iter()
            .flat_map(|r| {
                serde_json::from_value::<Vec<Finding>>(r.findings.clone()).unwrap_or_default()
            })
            .filter(|f| {
                fixed.contains(&f.id)
                    || earlier.iter().any(|r| {
                        r.fixed
                            .as_array()
                            .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(&f.id)))
                    })
            })
            .collect();
        let came_back = findings::came_back(&fixed_findings, &defects);

        row.findings = json!(new);
        row.verified_ok = json!(held);
        row.prior = json!(priors);
        row.checks = json!(runs);
        row.fixed = json!(fixed);
        for (key, value) in [
            ("openBlocking", json!(progress_count)),
            ("nested", json!(nested)),
            ("failedReviewers", json!(failed)),
            ("rulesHash", json!(rules_hash(&rules))),
        ] {
            row.detail[key] = value;
        }
        row.outcome = if !failed.is_empty() {
            Why::new("review_incomplete")
        } else if blocking_now == 0 {
            Why::new("review_clean")
        } else {
            Why::new("review_blocking").with("count", blocking_now)
        }
        .text();
        row.finished_at = Some(Self::now());

        // The owner's calls, asked now, with the recommended one first.
        let mut decided: Vec<String> = Vec::new();
        let mut unanswered: Vec<Finding> = Vec::new();
        let mut asked: Vec<serde_json::Value> = Vec::new();
        for d in &calls {
            if let Some((_, answer)) = asked_before.iter().find(|(f, _)| f.same_call(d)) {
                decided.push(format!(
                    "The owner already decided \"{}\": {answer}. Carry it out, and don't raise it again.",
                    d.title
                ));
                asked.push(json!({ "finding": d, "answer": answer }));
                continue;
            }
            let options = d.choices();
            let context = format!(
                "{}\n\nWhy it matters: {}\n\nWhere: {}",
                d.repro, d.impact, d.evidence
            );
            match self.ask_owner(&v.id, &d.title, &context, &options, &Purpose::Decision { title: d.title.clone() }).await {
                Some(answer) => {
                    decided.push(format!("The owner decided \"{}\": {answer}. Carry it out.", d.title));
                    asked.push(json!({ "finding": d, "answer": answer }));
                }
                None => unanswered.push(d.clone()),
            }
        }
        row.decisions = json!(asked);
        let _ = store.put_verification_round(&row);

        let mut v_open = open.clone();
        v_open.extend(unanswered.iter().cloned());
        let mut deferred_all: Vec<serde_json::Value> =
            serde_json::from_value(v.deferred.clone()).unwrap_or_default();
        deferred_all.extend(deferred.iter().map(|d| json!(d)));
        let after = db::Verification {
            rounds: round as i64,
            open: json!(v_open),
            deferred: json!(deferred_all),
            ..v.clone()
        };
        let _ = store.set_verification_state(&v.id, &state_of(&after, "running", ""), Self::now());
        work.last = Some(head.clone());
        self.verify.work.put(work.clone()).await;

        if !failed.is_empty() {
            tracing::warn!(verification = %v.id, failed = %failed.join("; "), "a review could not finish");
            self.finish(&after, "not_verified", Some(Why::new("review_incomplete")));
            return RoundEnd::Finished;
        }
        if !unanswered.is_empty() {
            let titles: Vec<&str> = unanswered.iter().map(|d| d.title.as_str()).collect();
            self.finish(&after, "not_verified", Some(Why::new("needs_decision").with("items", titles.join("; "))));
            return RoundEnd::Finished;
        }
        if blocking_now > 0 {
            if findings::stalled(&counts, came_back) {
                let question = format!(
                    "The review of {} keeps finding problems: {blocking_now} blocking issue{} still open after {round} rounds, \
                     without getting fewer. What should happen?",
                    work.project_root.display(),
                    if blocking_now == 1 { " is" } else { "s are" }
                );
                let context = open
                    .iter()
                    .filter(|f| f.severity.blocks())
                    .map(|f| format!("- {} ({})", f.title, f.evidence))
                    .collect::<Vec<_>>()
                    .join("\n");
                let purpose = Purpose::Stalled { open: blocking_now };
                let answer = self.ask_owner(&v.id, &question, &context, &[KEEP_GOING, LAND_ANYWAY, STOP_HERE], &purpose).await;
                if let Some(step) = self.answered(work, &after, &purpose, answer.as_deref()).await {
                    return step.into();
                }
            }
            return RoundEnd::Continue(findings_brief(
                &work.project_root.display().to_string(),
                round,
                lenses,
                &open,
                &runs,
                &decided,
            ));
        }
        if !decided.is_empty() {
            return RoundEnd::Continue(findings_brief(
                &work.project_root.display().to_string(),
                round,
                lenses,
                &open,
                &runs,
                &decided,
            ));
        }
        // The minors: fixed, or deferred with a reason, once.
        let minors: Vec<Finding> = open
            .iter()
            .filter(|f| f.severity == Severity::Minor)
            .cloned()
            .collect();
        if !minors.is_empty() && self.verify.shared().minors_offered.insert(v.id.clone()) {
            return RoundEnd::Continue(findings_brief(
                &work.project_root.display().to_string(),
                round,
                lenses,
                &minors,
                &runs,
                &[],
            ));
        }
        if let Some(stat) = &drifted {
            // Never landed unless byte-identical to what was reviewed: the
            // changes the checks made go back to the builder, and the next
            // round reviews them.
            return RoundEnd::Continue(format!(
                "The project's own checks changed files in your working copy while this round ran, so what was \
                 reviewed is not what is there now:\n{stat}\nLook at those changes in your copy, keep or undo them, and \
                 end your reply; the next round reviews them."
            ));
        }
        if runs.is_empty() {
            // Reviewed, but no build or test could run: not verified.
            let reason = Why::new("no_checks");
            if work.kind == WorkKind::Isolated {
                let question = format!(
                    "The change to {} was reviewed, but the project has no build or tests that could run, so it isn't verified. \
                     Land it anyway?",
                    work.project_root.display()
                );
                let answer = self.ask_owner(&v.id, &question, &whole.stat(), &[KEEP_IN_COPY, LAND_ANYWAY], &Purpose::NoChecks).await;
                return self.answered(work, &after, &Purpose::NoChecks, answer.as_deref()).await.map_or(RoundEnd::Finished, RoundEnd::from);
            }
            self.finish(&after, "not_verified", Some(reason));
            return RoundEnd::Finished;
        }
        RoundEnd::Clean
    }
}

impl VerifyCheck {
    /// A clean round: the work is verified. A copy this loop made catches up
    /// with the owner's project and lands; an app's pages are reloaded and
    /// checked live; a helper's copy waits for its parent to land it.
    async fn land(&self, mut work: Work, notes: &str) -> Step {
        let store = self.store().clone();
        let Some(v) = store.get_verification(&work.id).ok().flatten() else {
            return Step::Gone;
        };
        match work.kind {
            WorkKind::Isolated => {}
            // An app's pages are live where they are: verified only when
            // the page reloads and checks out.
            WorkKind::AppPages => {
                match self.live_check_app(&work, &v).await {
                    Err(why) => self.finish(&v, "not_verified", Some(why)),
                    Ok(()) => self.finish(&v, "verified", None),
                }
                return Step::Finished;
            }
            WorkKind::HelperCopy | WorkKind::Folder => {
                self.finish(&v, "verified", None);
                return Step::Finished;
            }
        }
        self.land_reviewed(&mut work, &v, notes).await
    }

    /// Land a copy this loop made, through the one landing path
    /// ([`worktree::land_one`]): exactly what was reviewed, compared by
    /// content; caught up with the owner's project, reviewed again when
    /// anything of it changed; re-tested; landed as one commit. A landing
    /// that fails for want of review counts against progress.
    async fn land_reviewed(&self, work: &mut Work, v: &db::Verification, notes: &str) -> Step {
        let store = self.store().clone();
        let Some(mut iso) = work.iso.clone() else {
            self.finish(v, "verified", None);
            return Step::Finished;
        };
        let round = next_step(&store, &v.id);
        // Only what was reviewed and tested lands, byte for byte.
        let reviewed = match (work.shadow.snapshot().await, work.last.clone()) {
            (Ok(now), Some(last)) if now == last => match work.shadow.diff(&work.base, &last).await {
                Ok(change) => work.shadow.contents(&last, &change.paths()).await.ok(),
                Err(_) => None,
            },
            _ => None,
        };
        let Some(reviewed) = reviewed else {
            return self
                .no_progress(
                    work,
                    v,
                    round,
                    None,
                    "Your working copy changed after it was reviewed, so it can't land as it is. End your reply; the \
                     next round reviews what changed."
                        .into(),
                )
                .await;
        };
        // A landing waiting for another one on the project says so in the
        // verification's history; the owner's Stop ends the wait.
        let waiting = {
            let (store, id) = (store.clone(), v.id.clone());
            std::sync::Arc::new(move || {
                let _ = store.put_verification_round(&db::VerificationRound {
                    verification_id: id.clone(),
                    round: round + 1,
                    kind: "land".into(),
                    outcome: Why::new("waiting_to_land").text(),
                    started_at: Self::now(),
                    ..Default::default()
                });
            }) as std::sync::Arc<dyn Fn() + Send + Sync>
        };
        let landing = worktree::Landing { only: Some(reviewed), stop: self.cancel.clone(), waiting: Some(waiting) };
        let message = landing_message(&v.spec);
        let started = Self::now();
        // Re-test in the copy as it is after catching up: what was verified
        // is what lands.
        let runs: Arc<Mutex<Vec<CheckRun>>> = Default::default();
        let (tools, ctx, work_dir, build) = (
            self.harness.tools.clone(),
            {
                let mut ctx = self.tool_ctx();
                ctx.cwd = Some(work.work_dir.to_string_lossy().into_owned());
                ctx
            },
            work.work_dir.clone(),
            checks::build_cache(&work.project_root),
        );
        let ran = runs.clone();
        let retest = move |iso: &worktree::Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> {
            let (tools, ctx, work_dir, build, ran, iso) = (tools.clone(), ctx.clone(), work_dir.clone(), build.clone(), ran.clone(), iso.clone());
            Box::pin(async move {
                let changed = worktree::changed_files(&iso).await;
                let found = checks::detect(&work_dir, &changed);
                let done = checks::run(&tools, &ctx, &found, &build?).await;
                let failed: Vec<String> = done.iter().filter(|r| !r.passed).map(|r| format!("{} failed:\n{}", r.label, r.tail)).collect();
                *ran.lock().unwrap_or_else(|p| p.into_inner()) = done;
                if failed.is_empty() { Ok(()) } else { Err(failed.join("\n\n")) }
            })
        };
        let (caught, landed) = worktree::land_one(&mut iso, &message, Some(&retest), &landing).await;
        work.iso = Some(iso.clone());
        self.verify.work.put(work.clone()).await;
        let _ = store.set_verification_isolation(&v.id, &serde_json::to_value(&iso).unwrap_or_default());
        let runs = runs.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(caught) = &caught {
            let outcome = if !caught.conflicts.is_empty() {
                Why::new("conflicts").with("files", caught.conflicts.join(", "))
            } else if caught.onto.is_empty() {
                Why::new("up_to_date")
            } else {
                Why::new("caught_up").with("commit", super::short(&caught.onto))
            };
            let _ = store.put_verification_round(&db::VerificationRound {
                verification_id: v.id.clone(),
                round,
                kind: "catch_up".into(),
                started_at: started,
                finished_at: Some(Self::now()),
                checks: json!(runs),
                outcome: outcome.text(),
                detail: json!({ "onto": caught.onto, "overlap": caught.overlap, "conflicts": caught.conflicts }),
                ..Default::default()
            });
        }
        match landed {
            MergeOutcome::Conflict { files, .. } => {
                if self.verify.shared().stuck(&v.id) {
                    self.finish(v, "not_verified", Some(Why::new("stalled").with("open", files.len())));
                    return Step::Finished;
                }
                Step::Continue(format!(
                    "Your change is verified, but catching it up with the owner's project met their own changes to the same \
                     lines in {}. Resolve them in your working copy ({}), keeping what the owner meant and what your change \
                     does, then stage each resolved file with `git add` (or `git rm` to keep a deletion): staging it is how \
                     you say it is resolved. The resolution is reviewed and checked again.",
                    files.join(", "),
                    work.work_dir.display()
                ))
            }
            MergeOutcome::NeedsReview { ref files, .. } => {
                let text = format!(
                    "What would land differs from what was reviewed in {}: catching up with the owner's project or the \
                     project's commit hooks changed it. The next round reviews the result: end your reply when you have \
                     looked it over in your copy.",
                    files.join(", ")
                );
                self.no_progress(work, v, round + 1, Some(&landed), text).await
            }
            MergeOutcome::RetestFailed { .. } => {
                let text = findings_brief(&work.project_root.display().to_string(), round as u32, &[], &[], &runs, &[]);
                self.no_progress(work, v, round + 1, Some(&landed), text).await
            }
            MergeOutcome::Unreviewed { ref files, .. } => {
                let text = format!(
                    "Your working copy holds changes nobody reviewed ({}): too big to snapshot, a repository of its own, or \
                     made after the review. Remove them from your copy, or move them into the change so they are reviewed, \
                     and end your reply.",
                    files.join(", ")
                );
                self.no_progress(work, v, round + 1, Some(&landed), text).await
            }
            MergeOutcome::Merged { ref commit, .. } => {
                self.record_landing(v, round + 1, &landed);
                let _ = store.set_verification_landed(&v.id, commit, Self::now());
                self.finish(v, "verified", None);
                work.clean_up();
                Step::Finished
            }
            MergeOutcome::NoChanges => {
                self.record_landing(v, round + 1, &landed);
                self.finish(v, "verified", None);
                Step::Finished
            }
            other => {
                self.record_landing(v, round + 1, &other);
                self.not_landed(work, v, other, notes).await
            }
        }
    }

    /// Every rule a review reads: the employee's own, as they are now, and
    /// the project's as they were when the work began (the work can't
    /// rewrite the rules it is reviewed against).
    async fn review_rules(&self, work: &Work) -> String {
        let owner_rules = self
            .store()
            .get_agent(&self.agent_id)
            .ok()
            .flatten()
            .and_then(|a| a.rules)
            .unwrap_or_default();
        let mut at_base = std::collections::HashMap::new();
        for rel in super::rules::rules_paths() {
            if let Some(text) = work.shadow.read(&work.base, &rel).await {
                at_base.insert(rel, text);
            }
        }
        super::rules::rules_section(&owner_rules, &super::rules::project_rules_from(|rel| at_base.get(rel).cloned()))
    }

    fn record_landing(&self, v: &db::Verification, round: i64, landed: &MergeOutcome) {
        let _ = self.store().put_verification_round(&db::VerificationRound {
            verification_id: v.id.clone(),
            round,
            kind: "land".into(),
            outcome: super::landing_why(landed).text(),
            detail: match landed {
                MergeOutcome::Merged {
                    files,
                    commit,
                    caught_up_to,
                } => json!({ "landedAs": commit, "caughtUpTo": caught_up_to, "files": files }),
                other => json!({ "outcome": worktree::render_outcome("the change", other) }),
            },
            started_at: Self::now(),
            finished_at: Some(Self::now()),
            ..Default::default()
        });
    }

    /// A verified change that could not land for want of review (what
    /// would land isn't what was reviewed, holds changes nobody reviewed,
    /// or failed its checks or hooks): back to the builder with `text`, and
    /// no progress. Its landing step counts as one open issue, so landings
    /// that keep failing without a new low pause for the owner after
    /// [`findings::ROUNDS_WITHOUT_PROGRESS`] tries, like a review that
    /// stalls. `landed`: `None` when the copy changed after the review.
    async fn no_progress(&self, work: &mut Work, v: &db::Verification, round: i64, landed: Option<&MergeOutcome>, text: String) -> Step {
        let store = self.store().clone();
        let (outcome, said) = match landed {
            Some(l) => (super::landing_why(l), worktree::render_outcome("the change", l)),
            None => (Why::new("changed_after_review"), "The copy changed after it was reviewed.".to_string()),
        };
        let _ = store.put_verification_round(&db::VerificationRound {
            verification_id: v.id.clone(),
            round,
            kind: "land".into(),
            outcome: outcome.text(),
            detail: json!({ "outcome": said, "openBlocking": 1 }),
            started_at: Self::now(),
            finished_at: Some(Self::now()),
            ..Default::default()
        });
        if findings::stalled(&progress_counts(&store.verification_rounds(&v.id).unwrap_or_default(), i64::MAX), false) {
            let question = format!(
                "The change to {} passed review, but it can't land as it was reviewed: {} It hasn't got closer in the last {} \
                 tries. What should happen?",
                work.project_root.display(),
                said,
                findings::ROUNDS_WITHOUT_PROGRESS
            );
            let purpose = Purpose::Stalled { open: 1 };
            let answer = self.ask_owner(&v.id, &question, &text, &[KEEP_GOING, LAND_ANYWAY, STOP_HERE], &purpose).await;
            if let Some(step) = self.answered(work, v, &purpose, answer.as_deref()).await {
                return step;
            }
        }
        Step::Continue(text)
    }

    /// A verified change that could not land: the builder fixes what is its
    /// to fix; the owner decides what is theirs.
    async fn not_landed(
        &self,
        work: &mut Work,
        v: &db::Verification,
        outcome: MergeOutcome,
        _notes: &str,
    ) -> Step {
        match outcome {
            MergeOutcome::HookFailed { ref output, .. } => {
                let text = format!(
                    "The project's own commit hooks refused your change when it was about to land:\n```\n{output}\n```\nFix it in your \
                     working copy ({}) and end your reply; it is checked again.",
                    work.work_dir.display()
                );
                // Its landing step, recorded already, is the one that counts.
                let round = next_step(self.store(), &v.id) - 1;
                self.no_progress(work, v, round, Some(&outcome), text).await
            }
            MergeOutcome::RetestFailed { error, .. } => Step::Continue(format!(
                "After catching up, the project's checks failed:\n{error}\nFix it in your working copy and end your reply."
            )),
            MergeOutcome::OwnerEdits { .. } if self.verify.shared().after_owner.remove(&v.id) => {
                self.finish(v, "verified", Some(Why::new("edits_in_way")));
                Step::Finished
            }
            MergeOutcome::OwnerEdits { files, .. } => {
                let question = format!(
                    "This change is verified, and you have edits of your own in {} that it also touches. Nothing of yours was \
                     overwritten. What should happen?",
                    files.join(", ")
                );
                let context = format!(
                    "The change waits in its copy at {}.",
                    work.work_dir.display()
                );
                let answer = self.ask_owner(&v.id, &question, &context, &[LAND_AFTER_MINE, KEEP_MINE], &Purpose::OwnerEdits).await;
                self.answered(work, v, &Purpose::OwnerEdits, answer.as_deref()).await.unwrap_or(Step::Finished)
            }
            other => {
                // Why it did not land, as a code; the raw words (a git
                // error) are in the landing's step and the log.
                tracing::info!(verification = %v.id, outcome = %worktree::render_outcome("the change", &other), "verified work not landed");
                self.finish(v, "verified", Some(super::landing_why(&other)));
                Step::Finished
            }
        }
    }

    /// What the owner's answer to one of the loop's questions does, the same
    /// whenever it comes. `None`: the round goes on.
    async fn answered(&self, work: &mut Work, v: &db::Verification, purpose: &Purpose, answer: Option<&str>) -> Option<Step> {
        match purpose {
            Purpose::Decision { title } => Some(match answer {
                Some(a) => Step::Continue(format!("The owner decided \"{title}\": {a}. Carry it out.")),
                None => {
                    self.finish(v, "not_verified", Some(Why::new("needs_decision").with("items", title.as_str())));
                    Step::Finished
                }
            }),
            Purpose::Stalled { open } => match answer {
                Some(KEEP_GOING) => None,
                Some(LAND_ANYWAY) => {
                    Some(self.land_unverified(work, v, Why::new("landed_unreviewed")).await.map_or(Step::Finished, Step::Continue))
                }
                Some(_) => {
                    self.finish(v, "not_verified", Some(Why::new("stopped_by_owner").with("open", *open)));
                    Some(Step::Finished)
                }
                None => {
                    self.finish(v, "not_verified", Some(Why::new("stalled").with("open", *open)));
                    Some(Step::Finished)
                }
            },
            Purpose::NoChecks => Some(match answer {
                Some(LAND_ANYWAY) => self.land_unverified(work, v, Why::new("no_checks")).await.map_or(Step::Finished, Step::Continue),
                _ => {
                    self.finish(v, "not_verified", Some(Why::new("no_checks")));
                    Step::Finished
                }
            }),
            Purpose::OwnerEdits => Some(match answer {
                // Landed through the one path: only if the copy is still what
                // was reviewed, caught up and re-tested again.
                Some(LAND_AFTER_MINE) => {
                    self.verify.shared().after_owner.insert(v.id.clone());
                    Box::pin(self.land_reviewed(work, v, "")).await
                }
                _ => {
                    self.finish(v, "verified", Some(Why::new("kept_own_edits")));
                    Step::Finished
                }
            }),
        }
    }

    /// Land work that isn't verified, at the owner's choice: the same
    /// landing, the badge "Not verified" with why. `Some` is what the builder
    /// is told when the landing needs it.
    async fn land_unverified(
        &self,
        work: &mut Work,
        v: &db::Verification,
        reason: Why,
    ) -> Option<String> {
        let Some(mut iso) = work.iso.clone() else {
            self.finish(v, "not_verified", Some(reason));
            return None;
        };
        let landing = worktree::Landing { stop: self.cancel.clone(), ..Default::default() };
        let (_, outcome) = worktree::land_one(&mut iso, &landing_message(&v.spec), None, &landing).await;
        work.iso = Some(iso);
        self.record_landing(
            v,
            next_step(self.store(), &v.id),
            &outcome,
        );
        if let MergeOutcome::Merged { commit, .. } = &outcome {
            let _ = self
                .store()
                .set_verification_landed(&v.id, commit, Self::now());
        }
        self.finish(v, "not_verified", Some(reason));
        None
    }

    /// An app's pages after a clean round: reloaded where they are open and
    /// checked live in a view of their own. It checks out only when the app
    /// exists, its page loads (its folder, its entry page, every file it
    /// loads) and that load had zero errors. No view that can be opened is
    /// a failure, never a pass; no browser to open one in is said as such
    /// (`live_unavailable`). `Err` is why it did not check out.
    async fn live_check_app(&self, work: &Work, v: &db::Verification) -> Result<(), Why> {
        let store = self.store();
        let app = store
            .list_agents(1_000, 0)
            .unwrap_or_default()
            .into_iter()
            .find(|a| {
                tools::app_dev::served_dir(a)
                    .is_some_and(|d| std::path::Path::new(d) == work.work_dir)
            });
        let ctx = self.tool_ctx();
        let Some(app) = app else {
            self.record_live(v, Why::new("live_failed"), json!({ "problem": "no app is served from these pages" }));
            return Err(Why::new("live_failed"));
        };
        // The owner's open views load the new build too.
        let reload = self
            .harness
            .tools
            .execute(&ctx, tools::app_dev::APP_RELOAD, json!({ "app": app.name }))
            .await;
        use tools::app_publish::LiveLoadError;
        let loaded = tools::app_publish::live_load(self.harness.tools.browser().as_ref(), &app).await;
        let (problem, why, seen) = match &loaded {
            Err(LiveLoadError::NoBrowser(e)) => (Some("the bot has no browser to open the app in"), "live_unavailable", json!(e)),
            Err(LiveLoadError::NotOpened(e)) => {
                (tools::app_dev::live_problem(&work.work_dir, 0).or(Some("no view of the app could be opened")), "live_failed", json!(e))
            }
            Err(LiveLoadError::NoScript(e)) => (Some(tools::app_publish::NO_SCRIPT), "live_no_script", json!(e)),
            Ok(failed) => (tools::app_dev::live_problem(&work.work_dir, failed.len()), "live_failed", json!(failed)),
        };
        let why = Why::new(if problem.is_none() { "live_ok" } else { why });
        self.record_live(v, why.clone(), json!({ "problem": problem, "errors": seen, "reload": reload.content }));
        if problem.is_none() { Ok(()) } else { Err(why) }
    }

    fn record_live(&self, v: &db::Verification, outcome: Why, detail: serde_json::Value) {
        let store = self.store();
        let round = next_step(store, &v.id);
        let _ = store.put_verification_round(&db::VerificationRound {
            verification_id: v.id.clone(),
            round,
            kind: "live".into(),
            outcome: outcome.text(),
            detail,
            started_at: Self::now(),
            finished_at: Some(Self::now()),
            ..Default::default()
        });
    }
}

/// The rules a review read, as a hash kept on its step: a review is skipped
/// on unchanged work only under the same rules.
/// `items` (a JSON array) added to the end of the array `into`.
fn append(into: &mut serde_json::Value, items: serde_json::Value) {
    let mut all = into.as_array().cloned().unwrap_or_default();
    all.extend(items.as_array().cloned().unwrap_or_default());
    *into = serde_json::Value::Array(all);
}

fn rules_hash(rules: &str) -> String {
    format!("{:016x}", tools::checkpoint::fingerprint(rules.as_bytes()))
}

/// The next step of a verification's history: after every step so far.
fn next_step(store: &db::Store, id: &str) -> i64 {
    store.verification_rounds(id).unwrap_or_default().iter().map(|r| r.round).max().unwrap_or(0) + 1
}

/// The open blockers and majors of each round before `before`, oldest
/// first, for [`findings::stalled`]: each review's, and one for each
/// landing that failed for want of review.
fn progress_counts(rows: &[db::VerificationRound], before: i64) -> Vec<usize> {
    rows.iter()
        .filter(|r| (r.kind == "review" || r.kind == "land") && r.round < before)
        .filter_map(|r| r.detail["openBlocking"].as_u64().map(|n| n as usize))
        .collect()
}

/// The one commit a change lands as: the request's first line, as a
/// sentence.
fn landing_message(spec: &str) -> String {
    let line = spec
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Code changes made by an employee");
    let line: String = line.chars().take(72).collect();
    let mut chars = line.chars();
    match chars.next() {
        Some(c) => format!("{}{}", c.to_uppercase(), chars.as_str()),
        None => "Code changes made by an employee".into(),
    }
}
