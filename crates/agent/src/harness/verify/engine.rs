//! The verify loop: the end check of a turn that did code work.
//!
//! When the builder answers with no more calls, each project the turn
//! changed goes through a round: the project's own build and tests run in
//! the work's folder (an app's own check is its page's live load, on a
//! phone and a desktop), then independent reviewers (one per lens, fresh, told
//! to refute) review the change: the whole change in round 1, the fixes
//! since in later rounds. Findings go back to the same builder, in the same
//! turn, as its next step; it fixes them in its copy and answers again, and
//! the next round reviews the fixes. A round with no blockers or majors,
//! passing checks, and the minors fixed or deferred with a reason is clean:
//! a copy then catches up with the owner's project, is checked again (and
//! reviewed again where catching up touched what it changed), and lands as
//! one commit. Then the builder reports to the owner from the record.
//!
//! A reviewer returns its verdict through its answer tool; one that can't
//! finish runs again, alone and fresh, in the same round: an incomplete
//! review never ends the loop. A round runs in a task of its own, so the
//! owner's "send now" (a queued message delivered while it runs) stops only
//! the builder's reply: the round goes on and the turn that answers him
//! joins it.
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

/// How a turn that ended before its checking did leaves its verifications.
#[derive(Debug, Clone, PartialEq)]
pub enum Closed {
    /// Waiting on the owner (an approval, a question, his Stop): paused,
    /// for the turn that goes on. A stop never closes a code check.
    Waiting,
    /// Ended otherwise: not verified, with why.
    Ended(Why),
    /// Answered without its end check (a reply with no words, a
    /// compaction): what was still being checked is
    /// not verified, the run having ended before its checking did; a question
    /// waiting on the owner stays for the turn that goes on. After a normal
    /// end check nothing is still being checked, and nothing changes.
    Unchecked,
}

/// What one turn's verify loop keeps between its rounds.
#[derive(Default, Clone)]
struct Shared {
    /// The builder ran tools of its own this turn ([`Verify::mark_acted`]).
    acted: bool,
    /// The run's tool context as its last step had it: checks and the
    /// owner's cards go through the same door, under the same permissions.
    tool_ctx: Option<tools::ToolContext>,
    /// Verifications whose builder was told to report.
    reported: HashSet<String>,
    /// Verifications whose open minors the builder was asked to fix or
    /// defer with a reason.
    minors_offered: HashSet<String>,
    /// Times this turn the owner sent each stalled verification on (his
    /// "Keep going", or his own words): at [`STALL_GO_ONS`] it stops.
    went_on: std::collections::HashMap<String, usize>,
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

/// Times in one turn the owner may send a stalled verification on before
/// the loop stops instead of asking again.
const STALL_GO_ONS: usize = 3;

impl Shared {
    /// Take in what a round run in a task of its own kept ([`InFlight`]).
    fn merge(&mut self, from: Shared) {
        self.reported.extend(from.reported);
        self.minors_offered.extend(from.minors_offered);
        for (id, n) in from.went_on {
            let mine = self.went_on.entry(id).or_default();
            *mine = (*mine).max(n);
        }
    }

    /// Whether the owner already sent `id` on [`STALL_GO_ONS`] times this
    /// turn after it stalled.
    fn gone_on_enough(&self, id: &str) -> bool {
        self.went_on.get(id).is_some_and(|n| *n >= STALL_GO_ONS)
    }

    /// Count the owner sending `id` on after it stalled.
    fn go_on(&mut self, id: &str) {
        *self.went_on.entry(id.to_string()).or_default() += 1;
    }
}

/// A verification's step running in a task of its own: its round's checks
/// and reviewers. It outlives the turn that started it when the owner sends
/// a queued message now (`detach_rounds`): that turn stops to hear him, the
/// round goes on, and the session's next end check joins it. A stop with
/// nothing queued stops it with the turn (the check pauses).
struct InFlight {
    session_key: String,
    result: futures::future::Shared<futures::future::BoxFuture<'static, (Step, Option<Work>, Shared)>>,
    /// Stops the round's checks and reviewers.
    stop: CancellationToken,
    /// The round's loop state: the tool context it asks the owner and runs
    /// its checks with is the one of the turn it reports to ([`attach`]).
    shared: Arc<Mutex<Shared>>,
    /// Carries what the round says on the stream of the turn it reports to;
    /// cut when that turn stops to hear the owner, so its stream ends.
    relay: tokio::task::AbortHandle,
    /// Told when a turn joins the round: a question it asked meanwhile is
    /// put to the owner on that turn.
    joined: Arc<tokio::sync::Notify>,
    /// Set by the owner's "send now": the turn's stop leaves the round
    /// running.
    detached: Arc<std::sync::atomic::AtomicBool>,
}

/// The steps in flight, by verification id.
static IN_FLIGHT: std::sync::LazyLock<Mutex<std::collections::HashMap<String, InFlight>>> =
    std::sync::LazyLock::new(Default::default);

fn in_flight() -> std::sync::MutexGuard<'static, std::collections::HashMap<String, InFlight>> {
    IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner())
}

/// Point a round's loop state at the turn `ctx` is of: its tool context,
/// whose stream the round speaks on through a relay. Returns the relay, cut
/// when that turn stops to hear the owner.
fn attach(shared: &Mutex<Shared>, ctx: Option<tools::ToolContext>) -> tokio::task::AbortHandle {
    let mut ctx = ctx;
    let (relay_tx, mut relay_rx) = tokio::sync::mpsc::channel::<ai::StreamEvent>(64);
    let stream = ctx.as_mut().and_then(|c| c.stream_tx.replace(relay_tx));
    shared.lock().unwrap_or_else(|p| p.into_inner()).tool_ctx = ctx;
    tokio::spawn(async move {
        while let Some(event) = relay_rx.recv().await {
            if let Some(stream) = &stream
                && stream.send(event).await.is_err()
            {
                break;
            }
        }
    })
    .abort_handle()
}

/// The owner sent a message queued on `session_key` now: the rounds its
/// turn is running go on when the turn stops to hear him, for the turn that
/// answers him to join. Returns how many go on.
pub fn detach_rounds(session_key: &str) -> usize {
    let flights = in_flight();
    let mut n = 0;
    for f in flights.values().filter(|f| f.session_key == session_key) {
        f.detached.store(true, std::sync::atomic::Ordering::SeqCst);
        n += 1;
    }
    n
}

/// The owner's Stop on `session_key` with nothing queued: the rounds it
/// has running stop with its turn, joined or not.
pub fn stop_rounds(session_key: &str) {
    for f in in_flight().values().filter(|f| f.session_key == session_key) {
        f.stop.cancel();
    }
}

/// Whether verification `id` has a step running in its own task.
fn is_in_flight(id: &str) -> bool {
    in_flight().contains_key(id)
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

    /// Start the work a call begins where it first changes a code folder
    /// (`TurnWork::work_on`).
    pub async fn work_on(
        &self,
        store: &db::Store,
        input: &serde_json::Value,
        cwd: Option<&str>,
        changes: bool,
    ) -> Result<super::work::Begun, String> {
        self.work.work_on(input, cwd, changes, &self.builder.starting(store)).await
    }

    /// After a call ran, with what its routing watched
    /// (`TurnWork::after_call`).
    pub async fn after_call(&self, store: &db::Store, watched: super::work::Watched) -> Option<String> {
        self.work.after_call(&self.builder.starting(store), watched).await
    }

    /// The turn ended before its checking did (the step limit, a provider
    /// failure, a run left idle): every verification of it still running
    /// or waiting on the owner ends honestly, not verified with why; one
    /// waiting on the owner (his Stop included) is paused for the turn that
    /// goes on. Returns the ids it settled.
    pub async fn close(&self, store: &db::Store, why: Closed) -> Vec<String> {
        let mut ended = Vec::new();
        for work in self.work.snapshot().await {
            let Some(v) = store.get_verification(&work.id).ok().flatten() else { continue };
            // A round that goes on past this turn (the owner's "send now")
            // is still being checked: the turn that answers him joins it.
            if !matches!(v.state.as_str(), "running" | "paused") || is_in_flight(&v.id) {
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
                Closed::Ended(w) => ("not_verified", w.text()),
                Closed::Unchecked if v.state == "paused" => continue,
                Closed::Unchecked => ("not_verified", Why::new("turn_ended").text()),
            };
            let _ = store.set_verification_pending_ask(&v.id, None);
            let _ = store.set_verification_state(&v.id, &state_of(&v, state, &reason), chrono::Utc::now().timestamp());
            Work::clean_up_reviewers(std::path::Path::new(&v.project_root), &v.id);
            ended.push(v.id.clone());
        }
        ended
    }

    /// The verifications whose builder was told to report this turn: the
    /// turn's last reply is their badge's.
    pub fn reported(&self) -> Vec<String> {
        self.shared().reported.iter().cloned().collect()
    }
}

/// What the verify check of a turn runs with.
#[derive(Clone)]
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
    /// The loop stopped making progress with `open` blocking issues;
    /// `brief` is what the builder is told if it goes on (the round's
    /// findings), kept so his words after a restart go on top of the
    /// same brief.
    Stalled { open: usize, brief: String },
    /// Reviewers ran [`REVIEW_TRIES`] times and still could not finish.
    ReviewFailed,
}

const KEEP_GOING: &str = "Keep going";
const STOP_HERE: &str = "Stop here";
/// The message key a choice of the loop's is worded by in the owner's
/// language (`verify.ask.*` in the apps); `None` for a reviewer's own
/// choices, shown as written.
fn choice_key(label: &str) -> Option<String> {
    let key = match label {
        KEEP_GOING => "keepGoing",
        STOP_HERE => "stopHere",
        _ => return None,
    };
    Some(format!("verify.ask.{key}"))
}

/// One of the loop's questions for the owner: its English words (the
/// model, the logs, and the fallback), and the message keys and values his
/// apps word its question and context by in his language (`verify.ask.*`,
/// the card's `questionKey` and `questionValues`). A reviewer's call is
/// asked in its own words and has none.
struct Question {
    text: String,
    words: serde_json::Value,
}

impl Question {
    /// A question of the loop's own, worded by `verify.ask.<key>`.
    fn keyed(key: &str, values: serde_json::Value, text: String) -> Self {
        Question { text, words: json!({ "questionKey": format!("verify.ask.{key}"), "questionValues": values }) }
    }

    /// A question in someone else's words (a reviewer's call).
    fn as_written(text: &str) -> Self {
        Question { text: text.to_string(), words: serde_json::Value::Null }
    }
}
/// The owner's answer to one of the loop's questions: a choice the card
/// offered, or his own words (typed in the conversation, read by the bot as
/// the answer: `handlers::asks::answer_in_words`).
#[derive(Debug, Clone, PartialEq)]
enum Reply {
    Picked(String),
    Words(String),
}

impl Reply {
    fn text(&self) -> &str {
        match self {
            Reply::Picked(t) | Reply::Words(t) => t,
        }
    }

    /// What `ask_owner` returned: `picked` is a choice, `own_words` (or an
    /// open question's `response`) his own words.
    fn read(result: &serde_json::Value) -> Option<Self> {
        if let Some(p) = result["picked"].as_str() {
            return Some(Reply::Picked(p.to_string()));
        }
        result["own_words"].as_str().or(result["response"].as_str()).map(|w| Reply::Words(w.to_string()))
    }
}

/// The builder told what the owner said in his own words to a question of
/// the loop's that offered choices: it does that, and the next round checks
/// it. `brief` is what the builder would be
/// told had he sent it on without words (a stalled loop's findings): his
/// words go on top of it, never in place of it.
fn owner_said(words: &str, brief: &str) -> String {
    let said = format!(
        "The owner answered in his own words: \"{}\". Do what he says, then end your reply; the next round \
         checks it.",
        words.trim()
    );
    if brief.trim().is_empty() {
        return said;
    }
    format!("{}\n\n{said} Where his words and what is above differ, his words win.", brief.trim_end())
}

/// His own words to a question that offered choices (keep going, stop
/// here) are instructions, never a choice guessed from them: the
/// builder does what he says and the work is checked again; to a stalled
/// loop they go on top of its brief ([`owner_said`]). A call of his
/// (`Decision`) is carried out as he put it, words or choice.
fn instruction(purpose: &Purpose, answer: Option<&Reply>) -> Option<String> {
    match (purpose, answer) {
        // His words to reviewers that could not finish send them on.
        (Purpose::Decision { .. } | Purpose::ReviewFailed, _) => None,
        (Purpose::Stalled { brief, .. }, Some(Reply::Words(words))) => Some(owner_said(words, brief)),
        _ => None,
    }
}

/// Whether the owner's answer to a stalled loop sends it on: his "Keep
/// going", or his own words.
fn went_on(answer: Option<&Reply>) -> bool {
    matches!(answer, Some(Reply::Words(_))) || answer.map(Reply::text) == Some(KEEP_GOING)
}

/// Where one work's round left it.
#[derive(Clone)]
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
            match self.run_step(work, &notes).await {
                Some(Step::Continue(text)) => told.push(text),
                Some(Step::Finished | Step::Gone) => {}
                // The owner sent a queued message now: the turn stops to
                // hear him and the round goes on without it.
                None => return EndVerdict::Exit(crate::harness::turn::TurnExit::Cancelled),
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
        EndVerdict::Continue(TurnEvent::Verify(report_brief(&unreported, &records)))
    }
}

/// What the builder is told once every verification of the turn is
/// settled: report from the record. The work is in the project's own files
/// (a helper's, in its copy, which its parent lands), so the brief never
/// tells the builder the owner must review or merge it.
fn report_brief(settled: &[db::Verification], records: &[String]) -> String {
    let in_copies = settled.iter().all(|v| v.work_kind == WorkKind::HelperCopy.as_str());
    let lead = if in_copies {
        "The checking is done. Now report in plain words: what you built; what each round of review caught and what \
         you fixed; what is verified and what is not (never more than the record below says). Your changes are in \
         your own copy, which the one who started you lands."
    } else {
        "The checking is done. Now report to the owner in plain words: what you built; what each round of review \
         caught and what you fixed; what is verified and what is not (never more than the record below says); and any \
         decision of theirs it still needs. Your changes are in the project's own files already: there is nothing \
         for the owner to review or merge."
    };
    format!(
        "{lead} If the work deploys or releases, wait until it is live and check its health, logs and data before you \
         say it works, and say what you checked.\n\n{}",
        records.join("\n\n")
    )
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
            "live" => out.push_str(&format!("\n- Live check: {}.", said(r))),
            _ => {}
        }
    }
    // Every open finding in full, minors included, from the row's own
    // open list: the counts above never name findings this leaves out.
    let open: Vec<Finding> = serde_json::from_value(v.open.clone()).unwrap_or_default();
    if !open.is_empty() {
        out.push_str(&format!("\n- Still open ({}):", open.len()));
        for f in &open {
            out.push_str(&finding_block(f).replace("\n", "\n  "));
        }
    }
    if v.work_kind == WorkKind::HelperCopy.as_str() {
        out.push_str(&format!("\n- In its own copy at {}, which the one who started it lands.", v.work_dir));
    } else {
        out.push_str(&format!("\n- In the project's own files at {}.", v.work_dir));
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

/// One open finding in full, as every message to the builder carries it:
/// its id, severity, title, evidence, repro, impact and fix. The builder
/// never has to look for a finding anywhere else.
fn finding_block(f: &Finding) -> String {
    format!(
        "\n- {} [{}] {}: {}\n  Repro: {}\n  Impact: {}\n  Fix: {}",
        f.id,
        severity_word(f.severity),
        f.title,
        f.evidence,
        f.repro,
        f.impact,
        f.fix
    )
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
         and major below where it is; a blocker or major can't be deferred. For each minor, fix it or defer it \
         with a reason. End your reply with one line per finding: `<id>: fixed: what you changed`, `<id>: deferred: why` \
         (minors only), or `<id>: disputed: why it is not a defect` (and leave that code as it is). The next round's \
         reviewers check each line against the code, and it runs as soon as you end your reply: keep going, never \
         stop to report or to wait for the owner.",
        lenses
            .iter()
            .map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    for f in open {
        out.push_str(&finding_block(f));
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

    /// One work's next step, run in a task of its own ([`InFlight`]), or
    /// the one already running for it (started by an earlier turn the
    /// owner's "send now" stopped). The turn's stop stops it too, unless the
    /// owner sent a queued message now: then it goes on and `None` says so.
    /// A round whose code changed while it ran says its findings are from
    /// the version it started on; the next round reviews the code as it is.
    async fn run_step(&self, work: Work, notes: &str) -> Option<Step> {
        let id = work.id.clone();
        let (result, stop, detached, relay) = {
            let mut flights = in_flight();
            let ctx = self.verify.shared().tool_ctx.clone();
            let joining = flights.contains_key(&id);
            let flight = flights.entry(id.clone()).or_insert_with(|| {
                let stop = CancellationToken::new();
                // The round keeps its loop state in a copy merged back when
                // it is joined, and speaks on this turn's stream through a
                // relay the turn can cut.
                let shared = Arc::new(Mutex::new(self.verify.shared().clone()));
                let relay = attach(&shared, ctx.clone());
                let verify = Verify { shared: shared.clone(), ..self.verify.clone() };
                let me = VerifyCheck { cancel: stop.clone(), verify, ..self.clone() };
                let (notes, id) = (notes.to_string(), id.clone());
                let task = tokio::spawn(async move {
                    let step = me.step(work, &notes).await;
                    let work = me.verify.work.snapshot().await.into_iter().find(|w| w.id == id);
                    let kept = me.verify.shared().clone();
                    (step, work, kept)
                });
                let result = futures::FutureExt::shared(futures::FutureExt::boxed(async move {
                    task.await.unwrap_or_else(|e| {
                        tracing::error!(error = %e, "a verification's step ended without a result");
                        (Step::Finished, None, Shared::default())
                    })
                }));
                InFlight {
                    session_key: self.session_key.clone(),
                    result,
                    stop,
                    shared,
                    relay,
                    joined: Default::default(),
                    detached: Default::default(),
                }
            });
            // This turn is the one it reports to now: its questions and
            // checks go through this turn.
            if joining {
                flight.relay.abort();
                flight.relay = attach(&flight.shared, ctx);
                flight.joined.notify_one();
            }
            flight.detached.store(false, std::sync::atomic::Ordering::SeqCst);
            (flight.result.clone(), flight.stop.clone(), flight.detached.clone(), flight.relay.clone())
        };
        let (step, done, kept) = tokio::select! {
            out = result.clone() => out,
            _ = self.cancel.cancelled() => {
                if detached.load(std::sync::atomic::Ordering::SeqCst) {
                    tracing::info!(verification = %id, "the owner sent a message now: the round goes on");
                    // The round no longer speaks on this turn's stream: the
                    // turn ends now, and its stream with it.
                    relay.abort();
                    return None;
                }
                stop.cancel();
                result.await
            }
        };
        in_flight().remove(&id);
        self.verify.shared().merge(kept);
        let Some(work) = done else { return Some(step) };
        self.verify.work.put(work.clone()).await;
        let changed = match (work.shadow.snapshot().await, &work.last) {
            (Ok(now), Some(reviewed)) => &now != reviewed,
            _ => false,
        };
        Some(match step {
            Step::Continue(text) if changed => Step::Continue(format!(
                "This round reviewed your code as it was when the round started; you changed it since, so what \
                 follows is about that earlier version. Fix what still applies; the next round reviews your code as \
                 it is now.\n\n{text}"
            )),
            other => other,
        })
    }

    /// Make a round's folder for its reviewers: the read-only copy of `head`
    /// and the patch, in the verification's folder in the project's own
    /// Nebo folder ([`Work::copies`]). Returns that folder.
    async fn review_folder(&self, id: &str, round: u32, reviewed: &ChangeSet, work: &Work, head: &Snapshot) -> Result<PathBuf, String> {
        let scratch = Work::copies(&work.project_root, id)?;
        let round_dir = review::round_dir(&scratch, round);
        let _ = std::fs::remove_dir_all(&round_dir);
        std::fs::create_dir_all(&round_dir).map_err(|e| format!("create {}: {e}", round_dir.display()))?;
        let patch = review::patch_file(&round_dir);
        std::fs::write(&patch, &reviewed.patch).map_err(|e| format!("write {}: {e}", patch.display()))?;
        work.shadow.export(head, &review::project_dir(&round_dir)).await?;
        Ok(scratch)
    }

    /// A round whose reviewers could not be given what they read ends the
    /// verification unreviewed: nothing passes on a review that didn't
    /// happen. No reviewer could read it, so each one failed, never "no
    /// issues".
    fn review_incomplete(&self, v: &db::Verification, row: &mut db::VerificationRound, lenses: &[Lens], error: &str) -> RoundEnd {
        tracing::warn!(verification = %v.id, error = %error, "the reviewers' copy could not be made");
        for lens in lenses {
            row.detail["reviewers"][lens.as_str()] = json!("failed");
        }
        row.outcome = Why::new("review_incomplete").text();
        row.finished_at = Some(Self::now());
        let _ = self.store().put_verification_round(row);
        self.finish(v, "not_verified", Some(Why::new("review_incomplete")));
        RoundEnd::Finished
    }

    fn tool_ctx(&self) -> tools::ToolContext {
        self.verify.shared().tool_ctx.clone().unwrap_or_default()
    }

    /// Whether `v`'s reviewers have spent the employee's own spending limit
    /// for a run.
    fn over_spend_cap(&self, v: &db::Verification) -> bool {
        let store = self.store();
        let cap_cents = store.agent_run_spend_cap_cents(&self.agent_id);
        cap_cents > 0 && store.reviewers_cost(&v.reviewer_prefix).unwrap_or(0) >= cap_cents * 1_000_000
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
        question: &Question,
        context: &str,
        options: &[&str],
        purpose: &Purpose,
    ) -> Option<Reply> {
        if options.iter().all(|o| o.trim().is_empty()) {
            tracing::error!(verification = %id, question = %question.text, "a question for the owner with no choices was not asked");
            return None;
        }
        // In the owner's language: the question's keys and each choice's.
        let mut words = question.words.clone();
        if words.is_object() {
            words["optionKeys"] = json!(options.iter().map(|o| choice_key(o)).collect::<Vec<_>>());
        }
        let ask = json!({ "question": question.text, "context": context, "options": options, "purpose": purpose, "words": words });
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
        // A round that went on past the turn that started it (the owner's
        // "send now") asks on the turn that joins it: until one does, the
        // question waits, and the owner's Stop ends the wait.
        let joined = in_flight().get(id).map(|f| f.joined.clone());
        let mut ctx = self.tool_ctx();
        while let Some(joined) = joined.as_ref().filter(|_| ctx.cancel_token.is_cancelled()) {
            tokio::select! {
                _ = joined.notified() => {}
                _ = self.cancel.cancelled() => break,
            }
            ctx = self.tool_ctx();
        }
        let result = self.harness.tools.execute(&ctx, "ask_owner", call).await;
        let _ = store.set_verification_pending_ask(id, None);
        if let Some(v) = store.get_verification(id).ok().flatten() {
            let _ =
                store.set_verification_state(id, &state_of(&v, "running", &v.reason), Self::now());
        }
        if result.is_error {
            return None;
        }
        Reply::read(&serde_json::from_str(&result.content).unwrap_or_default())
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
        Work::clean_up_reviewers(std::path::Path::new(&v.project_root), &v.id);
    }

    /// One work's next step.
    async fn step(&self, mut work: Work, notes: &str) -> Step {
        let store = self.store().clone();
        let Some(mut v) = store.get_verification(&work.id).ok().flatten() else {
            return Step::Gone;
        };
        if finished_state(&v.state) {
            // Finished or stopped, but its copy changed since (the builder
            // kept working in it), or it deployed again: checked again over
            // the whole change, and the verdict it was reported on no longer
            // holds. A stop never ends code work for good (owner, 10-23).
            let changed = v.landed_ref.is_empty()
                && work.work_dir.is_dir()
                && (v.deploy_unchecked
                    || match work.shadow.snapshot().await {
                        Ok(now) => Some(&now) != work.last.as_ref(),
                        Err(_) => false,
                    });
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
            let question = Question { text: ask["question"].as_str().unwrap_or("").to_string(), words: ask["words"].clone() };
            let context = ask["context"].as_str().unwrap_or("");
            let purpose = serde_json::from_value::<Purpose>(ask["purpose"].clone()).ok();
            // The answer does what it did when it was first asked; his own
            // words to a stalled loop go on top of the brief it was asked
            // with.
            match &purpose {
                Some(Purpose::Stalled { open, brief }) => {
                    if let Some(step) = self.stalled(&v, &question, context, *open, brief).await {
                        return step;
                    }
                }
                Some(p) => {
                    let answer = self.ask_owner(&v.id, &question, context, &opts, p).await;
                    if let Some(step) = self.answered(&v, p, answer.as_ref()).await {
                        return step;
                    }
                }
                // A question this loop no longer asks: it waits on nothing.
                None => {
                    let _ = store.set_verification_pending_ask(&v.id, None);
                    self.finish(&v, "not_verified", Some(Why::new("needs_answer")));
                    return Step::Finished;
                }
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
        // again. A deploy since is checked first, by a round of checks
        // alone.
        let mut unchanged_clean = false;
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
            if clean && !v.deploy_unchecked {
                return self.land(work, notes).await;
            }
            unchanged_clean = clean;
        }
        let whole = work
            .shadow
            .diff(&work.base, &head)
            .await
            .unwrap_or_default();
        if whole.is_empty() && v.rounds == 0 && v.deployed_apps.is_empty() {
            // Nothing changed after all: no verification to show.
            let _ = store.delete_verification(&v.id);
            work.clean_up();
            self.verify.work.remove(&v.id).await;
            return Step::Gone;
        }

        // The employee's own spending limit for a run holds here too.
        if self.over_spend_cap(&v) {
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
        // A round of checks alone, with no reviewer: a deploy after a clean
        // review, or one with no change to read and no finding for a
        // reviewer to close. The first round that reviewed has lenses.
        let checks_alone = unchanged_clean || (whole.is_empty() && v.open.as_array().is_none_or(|o| o.is_empty()));
        let first = store
            .verification_rounds(&v.id)
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.kind == "review" && r.lenses.as_array().is_some_and(|l| !l.is_empty()));
        let lenses: Vec<Lens> = match &first {
            _ if checks_alone => Vec::new(),
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
                &mut work, &v, round, &lenses, first.is_some(), &whole, &reviewed, &head, notes, &nested,
            )
            .await
        {
            RoundEnd::Continue(text) => Step::Continue(text),
            RoundEnd::Finished => Step::Finished,
            RoundEnd::Clean => self.land(work, notes).await,
        }
    }
}

/// Runs of a round's reviewers that could not finish before the owner is
/// asked whether they run again.
const REVIEW_TRIES: u32 = 3;

/// Why reviewers that could not finish stopped running again.
enum RerunEnd {
    /// The employee's spending limit for a run was reached.
    SpendLimit,
    /// The owner's answer when asked (`None`: nobody could answer).
    Owner(Option<Reply>),
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
    /// One round: the project's checks, then the reviewers (none when
    /// `lenses` is empty: a round of checks alone); its record written; what
    /// the builder, or the owner, does next. `delta`: an earlier round
    /// reviewed the change, so this one reviews what changed since.
    #[allow(clippy::too_many_arguments)]
    async fn review_round(
        &self,
        work: &mut Work,
        v: &db::Verification,
        round: u32,
        lenses: &[Lens],
        delta: bool,
        whole: &ChangeSet,
        reviewed: &ChangeSet,
        head: &Snapshot,
        notes: &str,
        nested: &[String],
    ) -> RoundEnd {
        let store = self.store().clone();
        // An app's pages no app is served from: it can't be checked live,
        // and no round of checks proves otherwise.
        let own = (work.kind == WorkKind::AppPages).then(|| store.app_served_from(&work.work_dir.to_string_lossy()).ok().flatten());
        if own.as_ref().is_some_and(Option::is_none) {
            self.finish(v, "not_verified", Some(Why::new("live_failed")));
            return RoundEnd::Finished;
        }
        let own = own.flatten();
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
        // The plain-code step: the project's own build and tests. An app's own check is its live load,
        // after the checks its `checks/` folder declares; each app the work
        // deployed into (kept on its row) has its live load among the
        // work's checks. The round's record says, as it goes, which checks
        // it runs and what each came to, then each reviewer's result as it
        // arrives: the owner watches the round, not a spinner.
        let holds_reload =
            !tools::app_dev::withheld(&store, &self.agent_id).iter().any(|t| t == tools::app_dev::APP_RELOAD);
        let checks = match &own {
            Some(app) => app_checks(app, &work.work_dir),
            None => checks::detect(&work.work_dir, &whole.paths()),
        };
        let mut lives: Vec<Live> = own.iter().map(|a| Live::new(a.clone(), true, holds_reload)).collect();
        for id in &v.deployed_apps {
            if own.as_ref().is_some_and(|a| &a.id == id) {
                continue;
            }
            // An app removed since has nothing left to check.
            if let Some(app) = store.get_agent(id).ok().flatten() {
                lives.push(Live::new(app, false, holds_reload));
            }
        }
        let mut planned: Vec<String> = checks.iter().map(|c| c.label.clone()).collect();
        planned.extend(lives.iter().map(|l| l.label.clone()));
        row.detail = json!({ "whole": whole.stat(), "checksPlanned": planned });
        self.record_round(&row);
        let mut ctx = self.tool_ctx();
        ctx.cwd = Some(work.work_dir.to_string_lossy().into_owned());
        let build = match checks::build_cache(&work.project_root) {
            Ok(build) => build,
            Err(e) => return self.review_incomplete(v, &mut row, lenses, &e),
        };
        let mut runs: Vec<CheckRun> = Vec::with_capacity(planned.len());
        for check in &checks {
            runs.extend(checks::run(&self.harness.tools, &ctx, std::slice::from_ref(check), &build).await);
            row.checks = json!(runs);
            self.record_round(&row);
            if self.cancel.is_cancelled() {
                break;
            }
        }
        // A live check that could not run (no browser to load the app in,
        // or a page without the check script): the work can't be verified.
        let mut live_unavailable = None;
        for live in &lives {
            if self.cancel.is_cancelled() {
                break;
            }
            match self.live_check(live).await {
                Ok(run) => {
                    runs.push(run);
                    row.checks = json!(runs);
                    self.record_round(&row);
                }
                Err(why) => {
                    // Never shown as running: it isn't one of the checks.
                    planned.retain(|p| p != &live.label);
                    row.detail["checksPlanned"] = json!(planned);
                    self.record_round(&row);
                    live_unavailable.get_or_insert(why);
                }
            }
        }
        if self.cancel.is_cancelled() {
            return self.stopped(v, &mut row);
        }
        // Its deploys' live loads ran: a deploy from now on is checked anew.
        let _ = store.set_verification_deploys_checked(&v.id);

        let prior_open: Vec<Finding> = serde_json::from_value(v.open.clone()).unwrap_or_default();
        let rules = self.review_rules(work).await;
        // The reviewers: fresh, one per lens, all at once, each reading the
        // round's copy of exactly what was snapshotted (a throwaway export
        // of `head`) and its patch, sharing the verification's one build
        // folder, all in the project's Nebo folder (`Work::copies`): nothing a
        // reviewer runs can change the builder's files. A round of checks
        // alone has none.
        let mut rerun_end: Option<RerunEnd> = None;
        let reviews: Vec<LensReview> = if lenses.is_empty() {
            Vec::new()
        } else {
            let scratch = match self.review_folder(&v.id, round, reviewed, work, head).await {
                Ok(scratch) => scratch,
                Err(e) => return self.review_incomplete(v, &mut row, lenses, &e),
            };
            let round_dir = review::round_dir(&scratch, round);
            let (patch_file, review_dir) = (review::patch_file(&round_dir), review::project_dir(&round_dir));
            let input = RoundInput {
                verification_id: &v.id,
                round,
                attempt: 1,
                delta,
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
            let mut reviews: Vec<LensReview> =
                review::run_round(&self.helpers, &self.parent(), &input, &heard).await;
            // A reviewer that couldn't finish runs again, fresh, in the same
            // round, the others' reviews kept: an incomplete review never
            // ends the loop. After [`REVIEW_TRIES`] runs the owner decides
            // whether they run again; with nobody to ask, or past the
            // employee's spending limit, the round ends incomplete.
            let mut attempt = 1;
            loop {
                let unfinished: Vec<Lens> = reviews.iter().filter(|r| r.review.is_err()).map(|r| r.lens).collect();
                if unfinished.is_empty() || self.cancel.is_cancelled() {
                    break;
                }
                if self.over_spend_cap(v) {
                    rerun_end = Some(RerunEnd::SpendLimit);
                    break;
                }
                if attempt % REVIEW_TRIES == 0 {
                    if self.verify.shared().gone_on_enough(&v.id) {
                        break;
                    }
                    let why: Vec<String> = reviews
                        .iter()
                        .filter_map(|r| r.review.as_ref().err().map(|e| format!("- {}: {e}", r.lens.as_str())))
                        .collect();
                    let project = work.project_root.display().to_string();
                    let question = Question::keyed(
                        "reviewFailed",
                        json!({ "project": project, "count": unfinished.len(), "tries": attempt }),
                        format!(
                            "The review of {project} could not finish: {} reviewer{} failed {attempt} times. Run {} again?",
                            unfinished.len(),
                            if unfinished.len() == 1 { "" } else { "s" },
                            if unfinished.len() == 1 { "it" } else { "them" }
                        ),
                    );
                    let purpose = Purpose::ReviewFailed;
                    let answer = self.ask_owner(&v.id, &question, &why.join("\n"), &[KEEP_GOING, STOP_HERE], &purpose).await;
                    if !went_on(answer.as_ref()) {
                        rerun_end = Some(RerunEnd::Owner(answer));
                        break;
                    }
                    self.verify.shared().go_on(&v.id);
                }
                attempt += 1;
                tracing::info!(verification = %v.id, round, attempt, lenses = ?unfinished, "reviewers that could not finish run again");
                {
                    let mut live = live.lock().unwrap_or_else(|p| p.into_inner());
                    for lens in &unfinished {
                        live.detail["reviewers"][lens.as_str()] = json!("running");
                    }
                    self.record_round(&live);
                }
                let again = RoundInput { lenses: &unfinished, attempt, ..input };
                for rerun in review::run_round(&self.helpers, &self.parent(), &again, &heard).await {
                    if let Some(slot) = reviews.iter_mut().find(|r| r.lens == rerun.lens) {
                        *slot = rerun;
                    }
                }
            }
            row = live.into_inner().unwrap_or_else(|p| p.into_inner());
            let _ = std::fs::remove_dir_all(&round_dir);
            if self.cancel.is_cancelled() {
                return self.stopped(v, &mut row);
            }
            reviews
        };
        // What was reviewed and checked must be what is in the copy now: a
        // check that rewrote a file (a lockfile, generated code) changed it.
        let drifted = match work.shadow.snapshot().await {
            Ok(now) if &now != head => Some(work.shadow.diff(head, &now).await.map(|c| c.stat()).unwrap_or_default()),
            Ok(_) => None,
            Err(e) => Some(format!("what changed could not be read again: {e}")),
        };

        // A reviewer that still couldn't finish after its runs again
        // (stopped, failed, no balance) leaves the round unreviewed: nothing
        // passes on a review that didn't happen.
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
        let checks_failed: usize = runs.iter().map(CheckRun::problems).sum();
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
            match self.ask_owner(&v.id, &Question::as_written(&d.title), &context, &options, &Purpose::Decision { title: d.title.clone() }).await {
                Some(answer) => {
                    decided.push(format!("The owner decided \"{}\": {}. Carry it out.", d.title, answer.text()));
                    asked.push(json!({ "finding": d, "answer": answer.text() }));
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
            match rerun_end {
                Some(RerunEnd::SpendLimit) => self.finish(&after, "not_verified", Some(Why::new("spend_limit"))),
                Some(RerunEnd::Owner(answer)) => {
                    if let Some(step) = self.answered(&after, &Purpose::ReviewFailed, answer.as_ref()).await {
                        return step.into();
                    }
                }
                None => self.finish(&after, "not_verified", Some(Why::new("review_incomplete"))),
            }
            return RoundEnd::Finished;
        }
        if !unanswered.is_empty() {
            let titles: Vec<&str> = unanswered.iter().map(|d| d.title.as_str()).collect();
            self.finish(&after, "not_verified", Some(Why::new("needs_decision").with("items", titles.join("; "))));
            return RoundEnd::Finished;
        }
        if blocking_now > 0 {
            let brief = findings_brief(
                &work.project_root.display().to_string(),
                round,
                lenses,
                &v_open,
                &runs,
                &decided,
            );
            if findings::stalled(&counts, came_back) {
                let project = work.project_root.display().to_string();
                let question = Question::keyed(
                    "stalled",
                    json!({ "project": project, "open": blocking_now, "rounds": round }),
                    format!(
                        "The review of {project} keeps finding problems: {blocking_now} blocking issue{} still open after \
                         {round} rounds, without getting fewer. What should happen?",
                        if blocking_now == 1 { " is" } else { "s are" }
                    ),
                );
                let context = open
                    .iter()
                    .filter(|f| f.severity.blocks())
                    .map(|f| format!("- {} ({})", f.title, f.evidence))
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Some(step) = self.stalled(&after, &question, &context, blocking_now, &brief).await {
                    return step.into();
                }
            }
            return RoundEnd::Continue(brief);
        }
        if !decided.is_empty() {
            return RoundEnd::Continue(findings_brief(
                &work.project_root.display().to_string(),
                round,
                lenses,
                &v_open,
                &runs,
                &decided,
            ));
        }
        // The minors: fixed, or deferred with a reason, once.
        let minors = v_open.iter().any(|f| f.severity == Severity::Minor);
        if minors && self.verify.shared().minors_offered.insert(v.id.clone()) {
            return RoundEnd::Continue(findings_brief(
                &work.project_root.display().to_string(),
                round,
                lenses,
                &v_open,
                &runs,
                &[],
            ));
        }
        if let Some(stat) = &drifted {
            // Never landed unless byte-identical to what was reviewed: the
            // changes the checks made go back to the builder, and the next
            // round reviews them.
            return RoundEnd::Continue(format!(
                "Your files changed while this round ran (the project's own checks, or your own edits), so what was \
                 reviewed is not what is there now:\n{stat}\nLook at those changes, keep or undo them, and \
                 end your reply; the next round reviews them."
            ));
        }
        if let Some(why) = live_unavailable {
            self.finish(&after, "not_verified", Some(why));
            return RoundEnd::Finished;
        }
        if runs.is_empty() {
            // Reviewed, but no build or test could run: not verified.
            self.finish(&after, "not_verified", Some(Why::new("no_checks")));
            return RoundEnd::Finished;
        }
        RoundEnd::Clean
    }
}

impl VerifyCheck {
    /// A clean round: the work is verified where it is (the project, an
    /// app's pages, or a helper's copy, which its parent lands).
    async fn land(&self, work: Work, _notes: &str) -> Step {
        let Some(v) = self.store().get_verification(&work.id).ok().flatten() else {
            return Step::Gone;
        };
        self.finish(&v, "verified", None);
        Step::Finished
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

    /// The loop stopped making progress with `open` blocking issues: the
    /// owner is asked whether it goes on or stops. Once he
    /// has sent it on [`STALL_GO_ONS`] times this turn and it stalls again,
    /// it stops as stalled instead of asking again. `brief`: what the
    /// builder is told if it goes on. `None`: the round goes on.
    async fn stalled(&self, v: &db::Verification, question: &Question, context: &str, open: usize, brief: &str) -> Option<Step> {
        if self.verify.shared().gone_on_enough(&v.id) {
            self.finish(v, "not_verified", Some(Why::new("stalled").with("open", open)));
            return Some(Step::Finished);
        }
        let purpose = Purpose::Stalled { open, brief: brief.to_string() };
        let answer = self.ask_owner(&v.id, question, context, &[KEEP_GOING, STOP_HERE], &purpose).await;
        if went_on(answer.as_ref()) {
            self.verify.shared().go_on(&v.id);
        }
        self.answered(v, &purpose, answer.as_ref()).await
    }

    /// What the owner's answer to one of the loop's questions does, the same
    /// whenever it comes. `None`: the round goes on.
    async fn answered(&self, v: &db::Verification, purpose: &Purpose, answer: Option<&Reply>) -> Option<Step> {
        if let Some(told) = instruction(purpose, answer) {
            return Some(Step::Continue(told));
        }
        let answer = answer.map(Reply::text);
        match purpose {
            Purpose::Decision { title } => Some(match answer {
                Some(a) => Step::Continue(format!("The owner decided \"{title}\": {a}. Carry it out.")),
                None => {
                    self.finish(v, "not_verified", Some(Why::new("needs_decision").with("items", title.as_str())));
                    Step::Finished
                }
            }),
            Purpose::Stalled { open, .. } => match answer {
                Some(KEEP_GOING) => None,
                Some(_) => {
                    self.finish(v, "not_verified", Some(Why::new("stopped_by_owner").with("open", *open)));
                    Some(Step::Finished)
                }
                None => {
                    self.finish(v, "not_verified", Some(Why::new("stalled").with("open", *open)));
                    Some(Step::Finished)
                }
            },
            Purpose::ReviewFailed => match answer {
                Some(STOP_HERE) | None => {
                    self.finish(v, "not_verified", Some(Why::new("review_incomplete")));
                    Some(Step::Finished)
                }
                // Run again: a later round reviews the change.
                Some(_) => None,
            },
        }
    }

    /// An app's live check, run every round like any other check: its page
    /// loaded fresh on a phone and a desktop, in a view of its own, through
    /// `app_reload(app, check: true)`, the one live check
    /// ([`tools::app_publish::live_load`]). It passes only when every view
    /// loaded with zero errors of the app's own and drew something; what
    /// failed is its output, which the builder fixes, each error one open
    /// problem. `Err`: it could not run at all (no browser to load it in, or
    /// a page without the check script).
    async fn live_check(&self, live: &Live) -> Result<CheckRun, Why> {
        let started = std::time::Instant::now();
        let mut ctx = self.tool_ctx();
        // Nobody answers a question in the middle of a check.
        ctx.cannot_wait = true;
        let result = self
            .harness
            .tools
            .execute(&ctx, tools::app_dev::APP_RELOAD, json!({ "app": live.app.id, "check": true }))
            .await;
        let checked = result.payload.as_ref().filter(|p| p["kind"] == tools::app_dev::LIVE_CHECK);
        match checked.and_then(|p| p["couldNotRun"].as_str()) {
            Some(tools::app_dev::LIVE_NO_BROWSER) => return Err(Why::new("live_unavailable")),
            Some(_) => return Err(Why::new("live_no_script")),
            None => {}
        }
        let passed = !result.is_error && checked.is_some_and(|p| p["passed"] == true);
        let errors = checked
            .and_then(|p| p["errors"].as_array())
            .map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        Ok(CheckRun {
            label: live.label.clone(),
            command: live.command.clone(),
            passed,
            tail: checks::tail(&result.content),
            duration_ms: started.elapsed().as_millis() as u64,
            errors,
        })
    }
}

/// One app's live check in a round: of the app whose pages are the work,
/// or of one the work deployed into.
struct Live {
    app: db::models::Agent,
    /// Its row among the round's checks.
    label: String,
    /// How the builder runs it: the call when it holds `app_reload`.
    command: String,
}

impl Live {
    /// `app`'s live check: the work's own app (`own`), or one it deployed
    /// into, named. `holds_reload`: the builder can run it itself.
    fn new(app: db::models::Agent, own: bool, holds_reload: bool) -> Self {
        Live {
            label: if own {
                "Live load on a phone and a desktop".into()
            } else {
                format!("Live load of {} on a phone and a desktop", app.name)
            },
            command: if holds_reload {
                super::work::live_check_call(&app.id)
            } else {
                "run by Nebo when you finish".into()
            },
            app,
        }
    }
}

/// The checks an app declares beside its live load: the project in its
/// app folder's `checks/` (its tests, run by the same rules as any
/// project's, [`checks::detect`]), never inside the served `pages`.
fn app_checks(app: &db::models::Agent, pages: &std::path::Path) -> Vec<checks::Check> {
    match tools::app_history::app_folder(app).map(|f| f.join("checks")) {
        Some(dir) if dir.is_dir() && !dir.starts_with(pages) => checks::detect(&dir, &[]),
        _ => Vec::new(),
    }
}

/// `items` (a JSON array) added to the end of the array `into`.
fn append(into: &mut serde_json::Value, items: serde_json::Value) {
    let mut all = into.as_array().cloned().unwrap_or_default();
    all.extend(items.as_array().cloned().unwrap_or_default());
    *into = serde_json::Value::Array(all);
}

/// The rules a review read, as a hash kept on its step: a review is skipped
/// on unchanged work only under the same rules.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every question and choice of the loop's is worded in the owner's
    /// language by a key the apps hold (`verify.ask.*`), and its English is
    /// the apps' English for it: the fallback says what the key says.
    #[test]
    fn the_loops_questions_are_worded_by_keys_the_apps_hold() {
        let en: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../app/src/lib/i18n/locales/en.json")).unwrap(),
        )
        .unwrap();
        let held = |key: &str| en["verify"]["ask"][key.trim_start_matches("verify.ask.")].as_str().map(str::to_string);
        for label in [KEEP_GOING, STOP_HERE] {
            let key = choice_key(label).expect("each choice of the loop's has a key");
            assert_eq!(held(&key).as_deref(), Some(label), "{key}");
        }
        assert_eq!(choice_key("cents"), None, "a reviewer's own choice shows as written");
        for key in ["stalled", "reviewFailed"] {
            let q = Question::keyed(key, json!({}), String::new());
            assert!(held(q.words["questionKey"].as_str().unwrap()).is_some(), "{key}");
        }
        assert!(Question::as_written("Round to cents?").words.is_null());
    }

    /// The card's choice is a choice; his typed words are his own words.
    #[test]
    fn an_answer_is_a_choice_or_his_own_words() {
        assert_eq!(Reply::read(&json!({"picked": "Keep going"})), Some(Reply::Picked("Keep going".into())));
        assert_eq!(Reply::read(&json!({"own_words": "add a test first"})), Some(Reply::Words("add a test first".into())));
        assert_eq!(Reply::read(&json!({"response": "yes"})), Some(Reply::Words("yes".into())));
        assert_eq!(Reply::read(&json!({})), None);
    }

    /// Review of #998: "keep going" typed to a stalled loop stopped it, and
    /// anything but the exact choice did the wrong thing on the others.
    /// Typed words are instructions on every question but a call of his.
    #[test]
    fn typed_words_are_instructions_never_a_guessed_choice() {
        let words = Reply::Words("keep going, but add a test for the total".into());
        for purpose in [Purpose::Stalled { open: 2, brief: String::new() }] {
            let told = instruction(&purpose, Some(&words)).expect("words are an instruction");
            assert!(told.contains("add a test for the total") && told.contains("next round checks it"), "{purpose:?}");
            assert_eq!(instruction(&purpose, Some(&Reply::Picked(STOP_HERE.into()))), None, "a choice does what it says");
            assert_eq!(instruction(&purpose, None), None);
        }
        assert_eq!(instruction(&Purpose::Decision { title: "Pins".into() }, Some(&words)), None, "a call is carried out as he put it");
    }

    /// Review of #998: his words to a stalled loop replaced its findings,
    /// so the builder never heard what was still open. They go on top of
    /// the findings, which stay whole.
    #[test]
    fn his_words_to_a_stalled_loop_go_on_top_of_its_findings() {
        let brief = "Independent reviewers checked your change.\n- f1 [blocker] total() drops cents: total.py:1";
        let told = instruction(&Purpose::Stalled { open: 1, brief: brief.into() }, Some(&Reply::Words("use Decimal".into()))).unwrap();
        assert!(told.starts_with(brief), "the findings stay whole and first: {told}");
        assert!(told.contains("\"use Decimal\"") && told.contains("his words win"), "{told}");
    }

    /// Review of #998: a stalled loop the owner kept sending on asked him
    /// again every round without end. He sends it on at most
    /// [`STALL_GO_ONS`] times a turn, by choice or in words; a Stop is no
    /// go-on.
    #[test]
    fn a_stalled_loop_is_sent_on_a_bounded_number_of_times() {
        assert!(went_on(Some(&Reply::Picked(KEEP_GOING.into()))));
        assert!(went_on(Some(&Reply::Words("try a Decimal".into()))));
        assert!(!went_on(Some(&Reply::Picked(STOP_HERE.into()))) && !went_on(None));
        let mut shared = Shared::default();
        for _ in 0..STALL_GO_ONS {
            assert!(!shared.gone_on_enough("v1"));
            shared.go_on("v1");
        }
        assert!(shared.gone_on_enough("v1"));
        assert!(!shared.gone_on_enough("v2"), "counted per verification");
    }
}
