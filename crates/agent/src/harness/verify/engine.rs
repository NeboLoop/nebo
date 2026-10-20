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

use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::checks::{self, CheckRun};
use super::findings::{self, Finding, Kind, Severity};
use super::lenses::{self, Lens};
use super::review::{self, LensReview, RoundInput};
use super::shadow::{ChangeSet, Snapshot};
use super::work::{TurnWork, Work, WorkKind};
use crate::harness::delegation::{Helpers, child::Parent};
use crate::harness::events::TurnEvent;
use crate::harness::turn_end::{EndCheck, EndVerdict, TurnEnd};
use crate::harness::{Harness, SeatRequest};
use crate::worktree::{self, MergeOutcome};

/// The line a builder ends its reply with when it has resolved what
/// catching up handed it: the explicit word, never read from the files.
pub const RESOLVED_LINE: &str = "Conflicts resolved.";

/// What one turn's verify loop keeps between its rounds.
#[derive(Default)]
struct Shared {
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
}

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
        let works = self.verify.work.snapshot().await;
        if works.is_empty() {
            return EndVerdict::Stop;
        }
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
            .filter(|v| !self.verify.shared().reported.contains(&v.id))
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

/// The riskiest of `lenses`: the one a later round's single reviewer reads
/// the fixes through, beside correctness.
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
        if v.reason.is_empty() {
            String::new()
        } else {
            format!(" ({})", v.reason)
        }
    );
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
            "catch_up" => out.push_str(&format!("\n- Caught up with the project: {}.", r.outcome)),
            "land" => out.push_str(&format!("\n- Landing: {}.", r.outcome)),
            "live" => out.push_str(&format!("\n- Live check: {}.", r.outcome)),
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
         and major below in your working copy, then end your reply; the next round reviews your fixes. Where you believe a \
         finding is wrong, say why in your reply instead of changing the code. For each minor, fix it or say you defer it \
         and why.",
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

    fn tool_ctx(&self) -> tools::ToolContext {
        self.verify.shared().tool_ctx.clone().unwrap_or_default()
    }

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    /// Ask the owner one question with choices, the recommended one first,
    /// on the conversation's card (and the mobile app). `None` when nobody
    /// can answer in this run. The question waits on the row while it is
    /// asked, so a restart asks it again.
    async fn ask_owner(
        &self,
        id: &str,
        question: &str,
        context: &str,
        options: &[&str],
    ) -> Option<String> {
        let ask = json!({ "question": question, "context": context, "options": options });
        let store = self.store();
        let _ = store.set_verification_pending_ask(id, Some(&ask));
        let before = store.get_verification(id).ok().flatten();
        if let Some(v) = &before {
            let _ =
                store.set_verification_state(id, &state_of(v, "paused", &v.reason), Self::now());
            self.harness.verification_changed(id);
        }
        let result = self
            .harness
            .tools
            .execute(&self.tool_ctx(), "ask_owner", ask)
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
            run_taint: &[],
            cancel: self.cancel.child_token(),
            platform: self.platform.as_deref(),
        }
    }

    /// Finish a verification: its state and reason written, its copy kept or
    /// landed as the caller left it.
    fn finish(&self, v: &db::Verification, state: &str, reason: &str) {
        let _ =
            self.store()
                .set_verification_state(&v.id, &state_of(v, state, reason), Self::now());
    }

    /// One work's next step.
    async fn step(&self, mut work: Work, notes: &str) -> Step {
        let store = self.store().clone();
        let Some(v) = store.get_verification(&work.id).ok().flatten() else {
            return Step::Gone;
        };
        if finished_state(&v.state) {
            return Step::Finished;
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
            let answer = self
                .ask_owner(
                    &v.id,
                    ask["question"].as_str().unwrap_or(""),
                    ask["context"].as_str().unwrap_or(""),
                    &opts,
                )
                .await;
            if answer.is_none() {
                self.finish(
                    &v,
                    "not_verified",
                    "Needs your answer to a question it asked",
                );
                return Step::Finished;
            }
        }
        let head = match work.shadow.snapshot().await {
            Ok(h) => h,
            Err(e) => {
                self.finish(
                    &v,
                    "not_verified",
                    &format!("Could not read what changed: {e}"),
                );
                return Step::Finished;
            }
        };
        let whole = work
            .shadow
            .diff(&work.base, &head)
            .await
            .unwrap_or_default();
        if whole.is_empty() && v.rounds == 0 {
            // Nothing changed after all: no verification to show.
            if let Some(iso) = &work.iso {
                worktree::put_away(iso, "No changes").await;
            }
            let _ = store.delete_verification(&v.id);
            work.clean_up();
            self.verify.work.remove(&v.id).await;
            return Step::Gone;
        }

        // A conflict handed back on catching up: resolved only on the
        // builder's word.
        if let Some(iso) = work.iso.as_mut()
            && notes.lines().any(|l| l.trim() == RESOLVED_LINE)
        {
            iso.mark_resolved();
        }

        // The employee's own spending limit for a run holds here too.
        let cap_cents = store.agent_run_spend_cap_cents(&self.agent_id);
        if cap_cents > 0
            && store.reviewers_cost(&v.reviewer_prefix).unwrap_or(0) >= cap_cents * 1_000_000
        {
            self.finish(
                &v,
                "not_verified",
                "Reached this employee's spending limit for a run",
            );
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
                vec![riskiest(&all)]
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
        let mut row = db::VerificationRound {
            verification_id: v.id.clone(),
            round: round as i64,
            kind: "review".into(),
            lenses: json!(lenses.iter().map(|l| l.as_str()).collect::<Vec<_>>()),
            base_ref: work.last.as_ref().unwrap_or(&work.base).0.clone(),
            head_ref: head.0.clone(),
            diff_stat: reviewed.stat(),
            reviewer_prefix: prefix,
            started_at: started,
            ..Default::default()
        };
        let _ = store.put_verification_round(&row);

        // The plain-code step: the project's own build and tests.
        let checks = checks::detect(&work.work_dir, &whole.paths());
        let mut ctx = self.tool_ctx();
        ctx.cwd = Some(work.work_dir.to_string_lossy().into_owned());
        let runs = checks::run(
            &self.harness.tools,
            &ctx,
            &checks,
            &checks::build_cache(&work.project_root),
        )
        .await;

        // The reviewers: fresh, one per lens, all at once.
        let patch_file = Work::home(&v.id).join(format!("round-{round}.patch"));
        let _ = std::fs::create_dir_all(Work::home(&v.id));
        let _ = std::fs::write(&patch_file, &reviewed.patch);
        let prior_open: Vec<Finding> = serde_json::from_value(v.open.clone()).unwrap_or_default();
        let owner_rules = store
            .get_agent(&self.agent_id)
            .ok()
            .flatten()
            .and_then(|a| a.rules)
            .unwrap_or_default();
        let rules = super::rules::brief_rules(&work.project_root, &owner_rules);
        let input = RoundInput {
            verification_id: &v.id,
            round,
            delta: round > 1,
            lenses,
            spec: &v.spec,
            change: reviewed,
            whole_stat: &whole.stat(),
            patch_file: &patch_file,
            work_dir: &work.work_dir,
            build_dir: &checks::build_cache(&work.project_root),
            checks: &runs,
            prior_open: &prior_open,
            builder_notes: if round > 1 { notes } else { "" },
            rules: &rules,
        };
        let reviews: Vec<LensReview> =
            review::run_round(&self.helpers, &self.parent(), &input).await;

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
        let (decisions, defects): (Vec<Finding>, Vec<Finding>) =
            new.iter().cloned().partition(|f| f.kind == Kind::Decision);
        let open = findings::open_after(&prior_open, &priors, &defects);
        let fixed: Vec<String> = priors
            .iter()
            .filter(|p| p.state == findings::PriorState::Fixed)
            .map(|p| p.id.clone())
            .collect();
        let deferred: Vec<findings::Prior> = priors
            .iter()
            .filter(|p| p.state == findings::PriorState::Deferred)
            .cloned()
            .collect();
        let checks_failed = runs.iter().filter(|r| !r.passed).count();
        let blocking_now = findings::blocking(&open) + checks_failed;

        // Progress, against the best so far.
        let earlier = store.verification_rounds(&v.id).unwrap_or_default();
        let mut counts: Vec<usize> = earlier
            .iter()
            .filter(|r| r.kind == "review" && r.round < round as i64)
            .filter_map(|r| r.detail["openBlocking"].as_u64().map(|n| n as usize))
            .collect();
        counts.push(blocking_now);
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
        row.detail =
            json!({ "openBlocking": blocking_now, "nested": nested, "failedReviewers": failed });
        row.outcome = if !failed.is_empty() {
            "review incomplete".into()
        } else if blocking_now == 0 {
            "clean".into()
        } else {
            format!("{blocking_now} blocking")
        };
        row.finished_at = Some(Self::now());

        // The owner's calls, asked now, with the recommended one first.
        let mut decided: Vec<String> = Vec::new();
        let mut unanswered: Vec<Finding> = Vec::new();
        for d in &decisions {
            let mut options: Vec<&str> = Vec::new();
            if let Some(r) = d.recommended.as_deref() {
                options.push(r);
            }
            for o in &d.options {
                if Some(o.as_str()) != d.recommended.as_deref() {
                    options.push(o);
                }
            }
            let context = format!(
                "{}\n\nWhy it matters: {}\n\nWhere: {}",
                d.repro, d.impact, d.evidence
            );
            match self.ask_owner(&v.id, &d.title, &context, &options).await {
                Some(answer) => decided.push(format!(
                    "The owner decided \"{}\": {answer}. Carry it out.",
                    d.title
                )),
                None => unanswered.push(d.clone()),
            }
        }
        row.decisions = json!(decided);
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
            self.finish(
                &after,
                "not_verified",
                &format!("A review could not finish ({})", failed.join("; ")),
            );
            return RoundEnd::Finished;
        }
        if !unanswered.is_empty() {
            let titles: Vec<&str> = unanswered.iter().map(|d| d.title.as_str()).collect();
            self.finish(
                &after,
                "not_verified",
                &format!("Needs your decision: {}", titles.join("; ")),
            );
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
                match self
                    .ask_owner(
                        &v.id,
                        &question,
                        &context,
                        &["Keep going", "Land it anyway", "Stop here"],
                    )
                    .await
                    .as_deref()
                {
                    Some("Keep going") => {}
                    Some("Land it anyway") => {
                        return match self
                            .land_unverified(
                                work,
                                &after,
                                "Landed without passing review, at your choice",
                            )
                            .await
                        {
                            Some(text) => RoundEnd::Continue(text),
                            None => RoundEnd::Finished,
                        };
                    }
                    Some(_) => {
                        self.finish(
                            &after,
                            "not_verified",
                            &format!("Stopped by you with {blocking_now} open issue(s)"),
                        );
                        return RoundEnd::Finished;
                    }
                    None => {
                        self.finish(
                            &after,
                            "not_verified",
                            &format!("Stopped making progress with {blocking_now} open issue(s)"),
                        );
                        return RoundEnd::Finished;
                    }
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
        if runs.is_empty() {
            // Reviewed, but no build or test could run: not verified.
            let reason = "No tests or build could run";
            if work.kind == WorkKind::Isolated {
                let question = format!(
                    "The change to {} was reviewed, but the project has no build or tests that could run, so it isn't verified. \
                     Land it anyway?",
                    work.project_root.display()
                );
                return match self
                    .ask_owner(
                        &v.id,
                        &question,
                        &whole.stat(),
                        &["Keep it in its copy", "Land it anyway"],
                    )
                    .await
                    .as_deref()
                {
                    Some("Land it anyway") => {
                        match self.land_unverified(work, &after, reason).await {
                            Some(text) => RoundEnd::Continue(text),
                            None => RoundEnd::Finished,
                        }
                    }
                    _ => {
                        self.finish(&after, "not_verified", reason);
                        RoundEnd::Finished
                    }
                };
            }
            self.finish(&after, "not_verified", reason);
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
            WorkKind::AppPages => {
                self.live_check_app(&work, &v).await;
                self.finish(&v, "verified", "");
                return Step::Finished;
            }
            WorkKind::HelperCopy | WorkKind::Folder => {
                self.finish(&v, "verified", "");
                return Step::Finished;
            }
        }
        let Some(mut iso) = work.iso.clone() else {
            self.finish(&v, "verified", "");
            return Step::Finished;
        };
        let message = landing_message(&v.spec);
        let round = store
            .verification_rounds(&v.id)
            .map(|r| r.len() as i64 + 1)
            .unwrap_or(1);
        let caught = worktree::catch_up(&mut iso, &message).await;
        work.iso = Some(iso.clone());
        self.verify.work.put(work.clone()).await;
        let _ = store
            .set_verification_isolation(&v.id, &serde_json::to_value(&iso).unwrap_or_default());
        let mut step = db::VerificationRound {
            verification_id: v.id.clone(),
            round,
            kind: "catch_up".into(),
            started_at: Self::now(),
            ..Default::default()
        };
        let caught = match caught {
            Ok(c) => c,
            Err(outcome) => {
                step.outcome = outcome_word(&outcome);
                step.detail =
                    json!({ "outcome": worktree::render_outcome("the change", &outcome) });
                step.finished_at = Some(Self::now());
                let _ = store.put_verification_round(&step);
                return self.not_landed(&work, &v, outcome, notes).await;
            }
        };
        step.detail = json!({ "onto": caught.onto, "overlap": caught.overlap, "conflicts": caught.conflicts });
        if !caught.conflicts.is_empty() {
            step.outcome = format!("conflicts in {}", caught.conflicts.join(", "));
            step.finished_at = Some(Self::now());
            let _ = store.put_verification_round(&step);
            return Step::Continue(format!(
                "Your change is verified, but catching it up with the owner's project met their own changes to the same \
                 lines in {}. Resolve them in your working copy ({}), keeping what the owner meant and what your change \
                 does, then end your reply with the line \"{RESOLVED_LINE}\" on its own. The resolution is checked again.",
                caught.conflicts.join(", "),
                work.work_dir.display()
            ));
        }
        // Re-test in the copy as it is now: what was verified is what lands.
        let mut ctx = self.tool_ctx();
        ctx.cwd = Some(work.work_dir.to_string_lossy().into_owned());
        let checks = checks::detect(&work.work_dir, &caught.changed);
        let runs = checks::run(
            &self.harness.tools,
            &ctx,
            &checks,
            &checks::build_cache(&work.project_root),
        )
        .await;
        step.checks = json!(runs);
        step.outcome = if caught.onto.is_empty() {
            "up to date".into()
        } else {
            format!("caught up to {}", &caught.onto[..caught.onto.len().min(10)])
        };
        step.finished_at = Some(Self::now());
        let _ = store.put_verification_round(&step);
        if runs.iter().any(|r| !r.passed) {
            return Step::Continue(findings_brief(
                &work.project_root.display().to_string(),
                round as u32,
                &[],
                &[],
                &runs,
                &[],
            ));
        }
        // Catching up changed what the change touches: a delta review of it.
        let head = work
            .shadow
            .snapshot()
            .await
            .unwrap_or_else(|_| work.last.clone().unwrap_or_else(|| work.base.clone()));
        if !caught.overlap.is_empty() && Some(&head) != work.last.as_ref() {
            return Step::Continue(format!(
                "Catching up with the owner's project changed files your change touches ({}). The next round reviews the \
                 result: end your reply when you have looked it over in your copy.",
                caught.overlap.join(", ")
            ));
        }
        let landed = worktree::land(&iso).await;
        self.record_landing(&v, round + 1, &landed);
        if let MergeOutcome::Merged { commit, .. } = &landed {
            let _ = store.set_verification_landed(&v.id, commit, Self::now());
            self.finish(&v, "verified", "");
            work.clean_up();
            return Step::Finished;
        }
        if matches!(landed, MergeOutcome::NoChanges) {
            self.finish(&v, "verified", "");
            return Step::Finished;
        }
        self.not_landed(&work, &v, landed, notes).await
    }

    fn record_landing(&self, v: &db::Verification, round: i64, landed: &MergeOutcome) {
        let _ = self.store().put_verification_round(&db::VerificationRound {
            verification_id: v.id.clone(),
            round,
            kind: "land".into(),
            outcome: outcome_word(landed),
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

    /// A verified change that could not land: the builder fixes what is its
    /// to fix; the owner decides what is theirs.
    async fn not_landed(
        &self,
        work: &Work,
        v: &db::Verification,
        outcome: MergeOutcome,
        _notes: &str,
    ) -> Step {
        match outcome {
            MergeOutcome::HookFailed { output, .. } => Step::Continue(format!(
                "The project's own commit hooks refused your change when it was about to land:\n```\n{output}\n```\nFix it in your \
                 working copy ({}) and end your reply; it is checked again.",
                work.work_dir.display()
            )),
            MergeOutcome::RetestFailed { error, .. } => Step::Continue(format!(
                "After catching up, the project's checks failed:\n{error}\nFix it in your working copy and end your reply."
            )),
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
                match self
                    .ask_owner(
                        &v.id,
                        &question,
                        &context,
                        &["Land it after I commit mine", "Keep mine, don't land it"],
                    )
                    .await
                    .as_deref()
                {
                    Some("Land it after I commit mine") => {
                        let landed = match work.iso.as_ref() {
                            Some(iso) => worktree::land(iso).await,
                            None => MergeOutcome::NoChanges,
                        };
                        self.record_landing(
                            v,
                            self.store()
                                .verification_rounds(&v.id)
                                .map(|r| r.len() as i64 + 1)
                                .unwrap_or(1),
                            &landed,
                        );
                        if let MergeOutcome::Merged { commit, .. } = &landed {
                            let _ =
                                self.store()
                                    .set_verification_landed(&v.id, commit, Self::now());
                            self.finish(v, "verified", "");
                        } else {
                            self.finish(
                                v,
                                "verified",
                                "Not landed yet: your edits are still in the way",
                            );
                        }
                        Step::Finished
                    }
                    _ => {
                        self.finish(
                            v,
                            "verified",
                            "Not landed: you kept your own edits; the change waits in its copy",
                        );
                        Step::Finished
                    }
                }
            }
            MergeOutcome::OwnerBusy { why, .. } => {
                self.finish(
                    v,
                    "verified",
                    &format!("Not landed yet: {why}; the change waits in its copy"),
                );
                Step::Finished
            }
            other => {
                self.finish(
                    v,
                    "verified",
                    &format!(
                        "Not landed: {}",
                        worktree::render_outcome("the change", &other)
                            .trim_start_matches("- the change: ")
                    ),
                );
                Step::Finished
            }
        }
    }

    /// Land work that isn't verified, at the owner's choice: the same
    /// landing, the badge "Not verified" with why. `Some` is what the builder
    /// is told when the landing needs it.
    async fn land_unverified(
        &self,
        work: &mut Work,
        v: &db::Verification,
        reason: &str,
    ) -> Option<String> {
        let Some(mut iso) = work.iso.clone() else {
            self.finish(v, "not_verified", reason);
            return None;
        };
        let mut isos = [iso.clone()];
        let outcome = worktree::merge_all(&mut isos, &landing_message(&v.spec), None)
            .await
            .remove(0)
            .1;
        [iso] = isos;
        work.iso = Some(iso);
        self.record_landing(
            v,
            self.store()
                .verification_rounds(&v.id)
                .map(|r| r.len() as i64 + 1)
                .unwrap_or(1),
            &outcome,
        );
        if let MergeOutcome::Merged { commit, .. } = &outcome {
            let _ = self
                .store()
                .set_verification_landed(&v.id, commit, Self::now());
        }
        self.finish(v, "not_verified", reason);
        None
    }

    /// An app's pages after a clean round: reloaded where they are open and
    /// checked live (its page loads, its console has no errors).
    async fn live_check_app(&self, work: &Work, v: &db::Verification) {
        let store = self.store();
        let app = store
            .list_agents(1_000, 0)
            .unwrap_or_default()
            .into_iter()
            .find(|a| {
                tools::app_dev::served_dir(a)
                    .is_some_and(|d| std::path::Path::new(d) == work.work_dir)
            });
        let Some(app) = app else { return };
        let ctx = self.tool_ctx();
        let reload = self
            .harness
            .tools
            .execute(&ctx, tools::app_dev::APP_RELOAD, json!({ "app": app.name }))
            .await;
        let status = self
            .harness
            .tools
            .execute(&ctx, tools::app_dev::APP_STATUS, json!({ "app": app.name }))
            .await;
        let round = store
            .verification_rounds(&v.id)
            .map(|r| r.len() as i64 + 1)
            .unwrap_or(1);
        let _ = store.put_verification_round(&db::VerificationRound {
            verification_id: v.id.clone(),
            round,
            kind: "live".into(),
            outcome: if reload.is_error || status.is_error {
                "the page didn't check out".into()
            } else {
                "the page reloaded and checked out".into()
            },
            detail: json!({ "reload": reload.content, "status": status.content }),
            started_at: Self::now(),
            finished_at: Some(Self::now()),
            ..Default::default()
        });
    }
}

/// One word for a landing's outcome, for the record.
fn outcome_word(o: &MergeOutcome) -> String {
    match o {
        MergeOutcome::NoChanges => "nothing to land".into(),
        MergeOutcome::Merged { commit, .. } => {
            format!("landed as {}", &commit[..commit.len().min(10)])
        }
        MergeOutcome::Conflict { files, .. } => format!("conflicts in {}", files.join(", ")),
        MergeOutcome::OwnerEdits { files, .. } => format!("your edits in {}", files.join(", ")),
        MergeOutcome::OwnerBusy { why, .. } => why.clone(),
        MergeOutcome::HookFailed { .. } => "refused by the project's commit hooks".into(),
        MergeOutcome::RetestFailed { .. } => "checks failed after catching up".into(),
        MergeOutcome::NotAttempted { .. } => "not tried".into(),
        MergeOutcome::Failed { error, .. } => error.clone(),
    }
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
