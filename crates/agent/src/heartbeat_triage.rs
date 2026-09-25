//! Heartbeat triage: one typed decision (Jev through Janus,
//! [`ai::DecideClient`]) before a scheduled or heartbeat fire starts a full
//! run, so an employee whose world has not changed does not pay for a turn.
//!
//! Where it sits: the engine's `drive` (server `engine.rs`), after a timer
//! fire became a queued run and before that run reaches the chat runner or
//! the workflow runner. The server reads what it can cheaply know about the
//! binding from the local store into a [`Binding`]; everything after that is
//! here.
//!
//! The order is the rule, and every step before the decision is code:
//!
//! 1. Flags win. If [`Flags::changed_anything`] is true, the fire runs and
//!    Jev is never asked. Triage only ever considers a skip when the flags
//!    say nothing changed.
//! 2. The floor. Never more than [`MAX_CONSECUTIVE_SKIPS`] skips in a row
//!    for one binding, and never a skip once more than [`max_skip_span`]
//!    has passed since its last real run. A stuck "skip" can never silence
//!    an employee. A last run that ended with a standing outcome (it said
//!    there was nothing to do, and why) is not a change and not a failure:
//!    its span is [`STANDING_FLOOR_MULTIPLE`] cadences, capped at
//!    [`STANDING_FLOOR_CAP`].
//! 3. One decision, one call ([`gate_from`]): does the job still need to
//!    run now although nothing changed, and is anything urgent — the
//!    thresholds in code, looser after a standing outcome
//!    ([`STANDING_SKIP_WORTH`], [`STANDING_SKIP_URGENT`]). In the same call:
//!    does the last outcome say the duty cannot be done until the owner adds
//!    or connects something, and which of the employee's declared needs
//!    ([`MISSING_NEED_HOLD`], [`WHICH_NEED_CONFIDENCE`]). A missing need
//!    holds the fire ([`Gate::Hold`]) and the caller tells the owner once;
//!    the model is not paid to rediscover the wall every cadence.
//!
//! A wrong skip looks exactly like a right one, so every skip is logged at
//! info with `site="heartbeat_triage"` and the numbers that made it.
//!
//! Fails OPEN: triage off, no client, an error, a timeout, a rate limit or
//! an incomplete answer all run the fire as before.
//!
//! Switch: `NEBO_DECIDE_TRIAGE` — `0` turns it off, `shadow` decides and
//! logs `would_skip`/`would_run` without ever skipping; unset (or anything
//! else) is on. Default ON (see [`mode`]).

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::Duration;

use ai::{DecideClient, Decision, Question};
use tracing::{debug, info};

use tools::truncate_str;
/// What the env switch says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    /// Decide and log; never act on it.
    Shadow,
    On,
}

// ── Thresholds: UNTUNED ──────────────────────────────────────────────────
//
// Set by hand before any shadow run. `NEBO_DECIDE_TRIAGE=shadow` logs both
// Nouls on every decided fire; the shadow data sets these, the way the
// memory gate and the old tool guardrail had theirs set from their first
// shadow runs. A Noul carries no separate confidence: the value is the
// certainty.

/// `worth_a_run` at or under this (with `urgent` quiet) skips.
pub const WORTH_A_RUN_SKIP_CEILING: f64 = 0.25;
/// `urgent` must be at or under this to skip.
pub const URGENT_SKIP_CEILING: f64 = 0.1;

// A binding whose last run ended with a standing outcome already proved
// "nothing to do, and why" once, so its bar to skip is looser. A wrong skip
// is still bounded by the standing floor and by flags winning. First live
// samples (owner's desktop, 2026-09-23), all run under the ordinary bar:
//
//   voicemail-and-missed-sweep  "No telephony plugin installed…"  worth 0.64  urgent 0.83
//   inventory-alerts            "zero orders…"                    worth 0.47  urgent 0.36
//   order-issue-scan            "No orders found…"                worth 0.30  urgent 0.04
//   publish-queue               (Social Media Manager)            worth 0.25  urgent 0.18
//
// Nothing had changed for any of them. Under these ceilings the last three
// skip; the first still runs on `urgent`.

/// UNTUNED. `worth_a_run` at or under this (with `urgent` under
/// [`STANDING_SKIP_URGENT`]) skips a fire after a standing outcome.
pub const STANDING_SKIP_WORTH: f64 = 0.5;
/// UNTUNED. `urgent` must be at or under this to skip after a standing
/// outcome.
pub const STANDING_SKIP_URGENT: f64 = 0.5;

// A last outcome that says the duty cannot be done until the owner adds or
// connects something is not "nothing to do": it is a wall only the owner
// can take down. First live sample (owner's desktop, 2026-09-24): a sweep
// whose run said "No telephony plugin available for voicemail or call log
// retrieval…" was scored urgent 0.83 and re-ran every 15 minutes, each run
// rediscovering the same wall.

/// UNTUNED. `missing_need` at or over this holds the fire and the owner is
/// told. High on purpose: a hold stops the duty until the owner acts, and a
/// wrong hold looks like a quiet employee. It is bounded like a skip: a hold
/// counts toward [`MAX_CONSECUTIVE_SKIPS`] and the floor's span, and any
/// flag (a changed setting, a plugin installed or an account connected)
/// runs the fire unasked. Set from shadow data like the other ceilings.
pub const MISSING_NEED_HOLD: f64 = 0.8;
/// UNTUNED. `which_need` names one of the employee's declared needs only at
/// or over this confidence; under it (or `other`/`unclear`) the owner is
/// told the duty needs something connected, without naming what.
pub const WHICH_NEED_CONFIDENCE: f64 = 0.6;
/// The most of the last outcome quoted to the owner: its first sentence,
/// capped here. Everything else the owner reads is written in code.
const CLAUSE_CAP: usize = 120;

// ── The floor: enforced in code, before the decision ─────────────────────

/// At most this many skips in a row for one binding; the next fire runs.
pub const MAX_CONSECUTIVE_SKIPS: u32 = 5;
/// Never skip once this long has passed since the binding's last real run.
/// A binding's own cadence times [`SPAN_CADENCES`] lowers it (see
/// [`max_skip_span`]).
pub const MAX_SKIP_SPAN: Duration = Duration::from_secs(30 * 60);
/// A binding that fires every N seconds may go at most N × this without a
/// real run, when that is shorter than [`MAX_SKIP_SPAN`].
pub const SPAN_CADENCES: u64 = 5;
/// UNTUNED. A binding whose last run ended with a standing outcome may go
/// its cadence × this without a real run (instead of [`max_skip_span`]'s
/// 30 minutes): the run already said why there was nothing to do, so the
/// question each fire is only whether that still holds.
pub const STANDING_FLOOR_MULTIPLE: u32 = 8;
/// UNTUNED. The standing-outcome span never exceeds this: at least one real
/// run a day, whatever the cadence.
pub const STANDING_FLOOR_CAP: Duration = Duration::from_secs(24 * 3600);

/// Char-boundary-safe caps on the state.
const PURPOSE_CAP: usize = 2_000;
const OUTCOME_CAP: usize = 300;

/// What the env switch says. `NEBO_DECIDE_TRIAGE`: `0`/`false`/`off`/`no`
/// turns triage off, `shadow` logs without acting, anything else (or unset)
/// is on.
pub fn mode() -> Mode {
    mode_from(std::env::var("NEBO_DECIDE_TRIAGE").ok().as_deref())
}

fn mode_from(value: Option<&str>) -> Mode {
    match value.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if v == "shadow" => Mode::Shadow,
        Some(v) if matches!(v.as_str(), "0" | "false" | "off" | "no") => Mode::Off,
        _ => Mode::On,
    }
}

/// Cheap facts about what changed for one binding since its last real run,
/// computed in code from the local store. No external service is asked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    /// No real run of this binding is on record (a one-shot, a new binding,
    /// or history triage cannot read): always runs.
    pub first_run: bool,
    /// The last real run did not end cleanly: it failed, was cancelled or
    /// interrupted, or its workflow has not finished. Something to follow
    /// up, not nothing to do. A run that ended with a standing outcome
    /// ([`Binding::standing`]) ended cleanly and does not set this.
    pub last_run_failed: bool,
    /// Messages addressed to the employee (its chats, threads and channels,
    /// not its own workflow sessions) since the last run started.
    pub new_messages: i64,
    /// Other work of the employee that started or ended since then: chat
    /// turns, cases and case turns, workflow runs started by an event, a
    /// watch, the owner or the employee, and a schedule's run-now. Timer
    /// fires are not counted — the binding's own, and its sibling schedules'
    /// and heartbeats' (the clock is not a change) — nor are sub-agents.
    pub other_runs: i64,
    /// Assignments handed to the employee since then.
    pub new_assignments: i64,
    /// The employee's settings or instructions changed since then, or a
    /// plugin was installed, turned on or off or updated, or one of the
    /// employee's plugin accounts was connected or changed: what a held
    /// fire waits on.
    pub settings_changed: bool,
}

impl Flags {
    /// Whether anything the flags can see has changed. True runs the fire
    /// without asking the decision model.
    pub fn changed_anything(&self) -> bool {
        self.first_run
            || self.last_run_failed
            || self.new_messages > 0
            || self.other_runs > 0
            || self.new_assignments > 0
            || self.settings_changed
    }

    /// The flags that are set, for the log: `other_runs:2,settings_changed`.
    /// Empty when none is.
    pub fn set_names(&self) -> String {
        let counted = |n: i64, name: &str| (n > 0).then(|| format!("{name}:{n}"));
        [
            self.first_run.then(|| "first_run".to_string()),
            self.last_run_failed.then(|| "last_run_failed".to_string()),
            counted(self.new_messages, "new_messages"),
            counted(self.other_runs, "other_runs"),
            counted(self.new_assignments, "new_assignments"),
            self.settings_changed.then(|| "settings_changed".to_string()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(",")
    }
}

/// One fire about to start, as triage sees it.
#[derive(Debug, Clone)]
pub struct Binding {
    /// The fire's timer target: `cron:<id>`, `hb:<agent>:<binding>` or
    /// `heartbeat:agent:<id>`. The per-binding counters are keyed by it.
    pub key: String,
    pub agent_id: String,
    /// What the binding is for: the heartbeat content, the job's prompt, or
    /// the workflow binding's description.
    pub purpose: String,
    /// How the last real run ended, in the status words the store keeps
    /// (`completed`, `done`, ...), or empty when unknown. A standing outcome
    /// reads `done`.
    pub last_status: String,
    /// The first line of the last real run's output, or, for a standing
    /// outcome, the reason the run gave. Empty when none.
    pub last_outcome: String,
    /// The last real run ended with a standing outcome: the step evaluator
    /// or the employee ended it because there was nothing to do, and said
    /// why (`last_outcome`). Not a flag: it widens the floor's span
    /// ([`max_skip_span`]) and the decision judges the reason.
    pub standing: bool,
    /// Seconds since the last real run started. `None`: no run on record.
    pub since_last_run: Option<i64>,
    /// Seconds between fires, when the binding's schedule says.
    pub cadence: Option<Duration>,
    pub flags: Flags,
    /// The workflow binding this fire runs, when it is one. Only a binding
    /// can be held on a missing need (the owner is told once per binding).
    pub duty: Option<String>,
    /// What the employee declares it may need: its `requires.interfaces`,
    /// its `requires.plugins`, and the binding's watch plugin. The options
    /// of `which_need`.
    pub declared: Vec<Declared>,
}

/// One thing an employee declares it may need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declared {
    /// A capability (`telephony`, `mail`): some plugin must provide it.
    Capability(String),
    /// A plugin by name.
    Plugin(String),
}

impl Declared {
    pub fn name(&self) -> &str {
        match self {
            Declared::Capability(n) | Declared::Plugin(n) => n,
        }
    }

    fn criterion(&self) -> String {
        match self {
            Declared::Capability(n) => format!("the missing thing is something that provides {n}, or an account or access for {n}"),
            Declared::Plugin(n) => format!("the missing thing is the {n} plugin, or an account or access on it"),
        }
    }
}

/// The need a held fire stands on, for the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldNeed {
    /// The declared need the decision named with confidence, or None.
    pub which: Option<Declared>,
    /// The last outcome's first sentence, capped: the one piece of the
    /// run's own words the owner is shown.
    pub clause: String,
}

/// What triage decided for one fire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Nothing changed and nothing is due: the fire does not run.
    Skip,
    /// Run the fire exactly as before (also the fail-open answer).
    Run,
    /// The last outcome says the duty cannot be done until the owner adds
    /// or connects something: the fire does not run, and the caller tells
    /// the owner (once per need).
    Hold(HeldNeed),
}

/// Per-binding counts since boot.
#[derive(Debug, Clone, Copy, Default)]
struct Tally {
    consecutive_skips: u32,
    skipped: u64,
    ran: u64,
}

static TALLY: Mutex<Option<HashMap<String, Tally>>> = Mutex::new(None);

fn tally(key: &str) -> Tally {
    let guard = TALLY.lock().unwrap_or_else(|p| p.into_inner());
    guard.as_ref().and_then(|m| m.get(key).copied()).unwrap_or_default()
}

/// Record one fire's fate and return the binding's counts after it. A
/// `would_skip` in shadow counts as a skip here, so the shadow log shows
/// the floor the enabled triage would hit.
fn record(key: &str, skipped: bool) -> Tally {
    let mut guard = TALLY.lock().unwrap_or_else(|p| p.into_inner());
    let t = guard.get_or_insert_with(HashMap::new).entry(key.to_string()).or_default();
    if skipped {
        t.consecutive_skips += 1;
        t.skipped += 1;
    } else {
        t.consecutive_skips = 0;
        t.ran += 1;
    }
    *t
}

/// The longest a binding may go without a real run: [`MAX_SKIP_SPAN`], or
/// its cadence × [`SPAN_CADENCES`] when that is shorter. After a standing
/// outcome (`standing`): its cadence × [`STANDING_FLOOR_MULTIPLE`], capped
/// at [`STANDING_FLOOR_CAP`]; with no cadence, [`MAX_SKIP_SPAN`].
pub fn max_skip_span(cadence: Option<Duration>, standing: bool) -> Duration {
    match cadence {
        Some(c) if !c.is_zero() && standing => STANDING_FLOOR_CAP.min(c.saturating_mul(STANDING_FLOOR_MULTIPLE)),
        Some(c) if !c.is_zero() => MAX_SKIP_SPAN.min(c.saturating_mul(SPAN_CADENCES as u32)),
        _ => MAX_SKIP_SPAN,
    }
}

/// The floor, in code: a skip is allowed only under
/// [`MAX_CONSECUTIVE_SKIPS`] in a row and within [`max_skip_span`] of the
/// last real run. No run on record allows no skip.
pub fn floor_allows_skip(consecutive_skips: u32, since_last_run: Option<i64>, cadence: Option<Duration>, standing: bool) -> bool {
    let Some(since) = since_last_run else { return false };
    consecutive_skips < MAX_CONSECUTIVE_SKIPS && since <= max_skip_span(cadence, standing).as_secs() as i64
}

/// Skip only when `worth_a_run` is at or under [`WORTH_A_RUN_SKIP_CEILING`]
/// and `urgent` at or under [`URGENT_SKIP_CEILING`]; after a standing
/// outcome (`standing`), at or under [`STANDING_SKIP_WORTH`] and
/// [`STANDING_SKIP_URGENT`]. A missing answer is a `Run`: triage never
/// skips on what it cannot read.
pub fn verdict_from(decision: &Decision, standing: bool) -> Gate {
    let (Some(worth), Some(urgent)) = (
        decision.answer("worth_a_run").and_then(|a| a.noul),
        decision.answer("urgent").and_then(|a| a.noul),
    ) else {
        return Gate::Run;
    };
    let (worth_ceiling, urgent_ceiling) = if standing {
        (STANDING_SKIP_WORTH, STANDING_SKIP_URGENT)
    } else {
        (WORTH_A_RUN_SKIP_CEILING, URGENT_SKIP_CEILING)
    };
    if worth <= worth_ceiling && urgent <= urgent_ceiling {
        Gate::Skip
    } else {
        Gate::Run
    }
}

/// The need the decision reads the last outcome as standing on, or None:
/// `missing_need` at or over [`MISSING_NEED_HOLD`]. `which_need` names a
/// declared need only at or over [`WHICH_NEED_CONFIDENCE`]. A missing
/// answer holds nothing.
pub fn held_need(decision: &Decision, b: &Binding) -> Option<HeldNeed> {
    b.duty.as_ref()?;
    let missing = decision.answer("missing_need").and_then(|a| a.noul)?;
    if missing < MISSING_NEED_HOLD {
        return None;
    }
    let which = decision
        .answer("which_need")
        .filter(|a| a.confidence.unwrap_or(0.0) >= WHICH_NEED_CONFIDENCE)
        .and_then(|a| a.choice.as_deref())
        .and_then(|c| b.declared.iter().find(|d| d.name() == c))
        .cloned();
    Some(HeldNeed { which, clause: first_clause(&b.last_outcome) })
}

/// The whole decision for one fire: a missing need holds it; otherwise
/// [`verdict_from`].
pub fn gate_from(decision: &Decision, b: &Binding) -> Gate {
    match held_need(decision, b) {
        Some(need) => Gate::Hold(need),
        None => verdict_from(decision, b.standing),
    }
}

/// The first sentence of an outcome, capped at [`CLAUSE_CAP`].
fn first_clause(outcome: &str) -> String {
    let line = outcome.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let sentence = line.find(". ").map_or(line, |i| &line[..=i]);
    if sentence.len() <= CLAUSE_CAP {
        sentence.to_string()
    } else {
        format!("{}…", truncate_str(sentence, CLAUSE_CAP).trim_end())
    }
}

/// Elapsed time as words, so the decision never does arithmetic.
pub fn elapsed_label(secs: i64) -> String {
    let secs = secs.max(0);
    let plural = |n: i64, unit: &str| format!("about {n} {unit}{}", if n == 1 { "" } else { "s" });
    match secs {
        s if s < 90 => "about a minute".to_string(),
        s if s < 3600 => plural((s + 30) / 60, "minute"),
        s if s < 36 * 3600 => plural((s + 1800) / 3600, "hour"),
        s => plural((s + 43_200) / 86_400, "day"),
    }
}

/// The state the decision reads. It is asked only when every flag is quiet,
/// so the checks are listed as what was found unchanged.
pub fn state(b: &Binding) -> serde_json::Value {
    let purpose = b.purpose.trim();
    let outcome = b.last_outcome.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    serde_json::json!({
        "purpose": if purpose.is_empty() { "none" } else { truncate_str(purpose, PURPOSE_CAP) },
        "last_run_status": if b.last_status.is_empty() { "unknown" } else { b.last_status.as_str() },
        "last_run_outcome": if outcome.is_empty() { "none recorded" } else { truncate_str(outcome, OUTCOME_CAP) },
        "time_since_last_run": b.since_last_run.map(elapsed_label).unwrap_or_else(|| "unknown".into()),
        "unchanged_since_last_run": [
            "no new messages to this employee",
            "no other work by this employee started or finished, apart from its own scheduled runs",
            "no new assignments to this employee",
            "no change to this employee's settings or instructions, to the installed plugins, or to its connected accounts",
            "the last run ended cleanly",
        ],
    })
}

fn questions(b: &Binding) -> BTreeMap<&'static str, Question> {
    let mut asked = BTreeMap::from([
        (
            "worth_a_run",
            Question::noul(
                "The last run ended with the outcome in `last_run_outcome`. Nothing in `unchanged_since_last_run` has changed since. Given that outcome and the standing duties in `purpose`, this fire still needs to run now: a deadline or a set time is due, a duty to check something outside this list on a schedule is due whatever happened, or a promise to act is now due.",
            ),
        ),
        (
            "urgent",
            Question::noul(
                "`purpose` or `last_run_outcome` describes something that needs attention right now: a problem left open, someone waiting on an answer, or a deadline today.",
            ),
        ),
    ]);
    if b.duty.is_none() {
        return asked;
    }
    let mut options: Vec<(String, String)> = b
        .declared
        .iter()
        .filter(|d| !matches!(d.name(), "other" | "unclear"))
        .map(|d| (d.name().to_string(), d.criterion()))
        .collect();
    options.push(("other".into(), "something is missing, but none of the options above".into()));
    options.push(("unclear".into(), "the outcome does not say what is missing, or nothing is".into()));
    let options: Vec<(&str, &str)> = options.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    asked.insert(
        "missing_need",
        Question::noul(
            "`last_run_outcome` says this duty cannot be done until something is added or connected — a plugin, an account, access or a setting the owner controls.",
        ),
    );
    asked.insert(
        "which_need",
        Question::choice(
            "If `last_run_outcome` says something must be added or connected before this duty can be done, which one it is.",
            &options,
        ),
    );
    asked
}

fn log_run(b: &Binding, reason: &str, tally: Tally) {
    debug!(
        site = "heartbeat_triage",
        binding = %b.key,
        agent = %b.agent_id,
        outcome = "run",
        reason,
        flags = %b.flags.set_names(),
        standing = b.standing,
        consecutive_skips = tally.consecutive_skips,
        since_last_run = b.since_last_run.unwrap_or(-1),
        skipped = tally.skipped,
        ran = tally.ran,
        "heartbeat triage"
    );
}

/// Decide whether one fire runs. `mode` is [`mode`] at the call site;
/// `timeout` bounds the one decision call. Fails OPEN: off, no client, any
/// error, a timeout or an incomplete answer returns [`Gate::Run`]. Only
/// [`Mode::On`] ever returns [`Gate::Skip`].
pub async fn triage(decide: Option<&DecideClient>, mode: Mode, b: &Binding, timeout: Duration) -> Gate {
    if mode == Mode::Off {
        return Gate::Run;
    }
    if b.flags.changed_anything() {
        log_run(b, "changed", record(&b.key, false));
        return Gate::Run;
    }
    let before = tally(&b.key);
    if !floor_allows_skip(before.consecutive_skips, b.since_last_run, b.cadence, b.standing) {
        log_run(b, "floor", record(&b.key, false));
        return Gate::Run;
    }
    let Some(client) = decide else {
        log_run(b, "no_client", record(&b.key, false));
        return Gate::Run;
    };

    let trace = ai::RequestTrace { agent_id: b.agent_id.clone(), ..ai::RequestTrace::new("heartbeat_triage") };
    let state = state(b);
    let questions = questions(b);
    let decision = match tokio::time::timeout(timeout, client.decide(&trace, &state, &questions)).await {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => {
            debug!(site = "heartbeat_triage", binding = %b.key, error = %e, "triage call failed; running");
            log_run(b, "error", record(&b.key, false));
            return Gate::Run;
        }
        Err(_) => {
            debug!(site = "heartbeat_triage", binding = %b.key, timeout_ms = timeout.as_millis() as u64, "triage timed out; running");
            log_run(b, "timeout", record(&b.key, false));
            return Gate::Run;
        }
    };
    let noul = |name: &str| decision.answer(name).and_then(|a| a.noul);
    let missing_need = noul("missing_need").unwrap_or(-1.0);
    let which = decision.answer("which_need");
    let which_need = which.and_then(|a| a.choice.clone()).unwrap_or_default();
    let which_confidence = which.and_then(|a| a.confidence).unwrap_or(-1.0);
    let verdict = gate_from(&decision, b);
    let (worth, urgent) = match (noul("worth_a_run"), noul("urgent")) {
        (Some(w), Some(u)) => (w, u),
        _ if matches!(verdict, Gate::Hold(_)) => (-1.0, -1.0),
        _ => {
            debug!(site = "heartbeat_triage", binding = %b.key, model = %decision.model, missing_need, "triage answer incomplete; running");
            log_run(b, "incomplete", record(&b.key, false));
            return Gate::Run;
        }
    };

    let shadow = mode == Mode::Shadow;
    let outcome = match (&verdict, shadow) {
        (Gate::Skip, false) => "skip",
        (Gate::Skip, true) => "would_skip",
        (Gate::Hold(_), false) => "hold",
        (Gate::Hold(_), true) => "would_hold",
        (Gate::Run, false) => "run",
        (Gate::Run, true) => "would_run",
    };
    // A hold counts as a skip: the floor bounds it the same way.
    let after = record(&b.key, verdict != Gate::Run);
    macro_rules! decided {
        ($level:ident) => {
            $level!(
                site = "heartbeat_triage",
                binding = %b.key,
                agent = %b.agent_id,
                outcome,
                worth_a_run = worth,
                urgent,
                missing_need,
                which_need = %which_need,
                which_confidence,
                standing = b.standing,
                consecutive_skips = after.consecutive_skips,
                since_last_run = b.since_last_run.unwrap_or(-1),
                skipped = after.skipped,
                ran = after.ran,
                model = %decision.model,
                input_tokens = decision.usage.input_tokens,
                cost_micro = decision.usage.cost_micro,
                "heartbeat triage"
            )
        };
    }
    if verdict == Gate::Run {
        decided!(debug);
    } else {
        decided!(info);
    }
    if shadow { Gate::Run } else { verdict }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ai::Answer;

    fn noul(p: f64) -> Answer {
        Answer {
            kind: "noul".into(),
            choice: None,
            score: None,
            noul: Some(p),
            confidence: None,
            probabilities: BTreeMap::new(),
        }
    }

    fn decision(worth: f64, urgent: f64) -> Decision {
        Decision {
            model: "jev-1.13.0".into(),
            answers: HashMap::from([
                ("worth_a_run".to_string(), noul(worth)),
                ("urgent".to_string(), noul(urgent)),
            ]),
            usage: Default::default(),
        }
    }

    /// A binding with nothing changed, well inside the floor.
    fn quiet(key: &str) -> Binding {
        Binding {
            key: key.into(),
            agent_id: "agent-1".into(),
            purpose: "Check the status of workflow run 5b11 and report results.".into(),
            last_status: "done".into(),
            last_outcome: "The workflow no longer exists.\nMore detail.".into(),
            since_last_run: Some(120),
            cadence: Some(Duration::from_secs(120)),
            standing: false,
            flags: Flags::default(),
            duty: None,
            declared: Vec::new(),
        }
    }

    /// A client whose every call is counted; it has no bearer, so a call
    /// fails as Auth before any network.
    fn counting_client() -> (DecideClient, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let client = DecideClient::new("http://127.0.0.1:1", move || {
            c.fetch_add(1, Ordering::SeqCst);
            None
        });
        (client, calls)
    }

    const T: Duration = Duration::from_secs(2);

    #[test]
    fn the_switch_defaults_on_and_zero_turns_it_off() {
        assert_eq!(mode_from(None), Mode::On);
        assert_eq!(mode_from(Some("1")), Mode::On);
        assert_eq!(mode_from(Some("anything")), Mode::On);
        assert_eq!(mode_from(Some("0")), Mode::Off);
        assert_eq!(mode_from(Some(" Off ")), Mode::Off);
        assert_eq!(mode_from(Some("false")), Mode::Off);
        assert_eq!(mode_from(Some("shadow")), Mode::Shadow);
        assert_eq!(mode_from(Some("SHADOW")), Mode::Shadow);
    }

    #[test]
    fn a_quiet_answer_skips_and_the_ceilings_are_inclusive() {
        assert_eq!(verdict_from(&decision(0.05, 0.01), false), Gate::Skip);
        assert_eq!(verdict_from(&decision(WORTH_A_RUN_SKIP_CEILING, URGENT_SKIP_CEILING), false), Gate::Skip);
    }

    #[test]
    fn either_noul_over_its_ceiling_runs() {
        assert_eq!(verdict_from(&decision(0.26, 0.0), false), Gate::Run);
        assert_eq!(verdict_from(&decision(0.0, 0.11), false), Gate::Run);
        assert_eq!(verdict_from(&decision(0.9, 0.9), false), Gate::Run);
    }

    #[test]
    fn a_standing_outcome_skips_under_its_own_inclusive_ceilings() {
        assert_eq!(verdict_from(&decision(STANDING_SKIP_WORTH, STANDING_SKIP_URGENT), true), Gate::Skip);
        assert_eq!(verdict_from(&decision(0.51, 0.0), true), Gate::Run);
        assert_eq!(verdict_from(&decision(0.0, 0.51), true), Gate::Run);
        // Without a standing outcome the same middling answer runs, as before.
        assert_eq!(verdict_from(&decision(0.3, 0.0), false), Gate::Run);
        assert_eq!(verdict_from(&decision(0.3, 0.0), true), Gate::Skip);
    }

    #[test]
    fn the_first_live_standing_samples() {
        // voicemail-and-missed-sweep: a missing plugin read as urgent still runs.
        assert_eq!(verdict_from(&decision(0.64, 0.83), true), Gate::Run);
        // inventory-alerts, order-issue-scan, publish-queue: unchanged worlds now skip.
        assert_eq!(verdict_from(&decision(0.47, 0.36), true), Gate::Skip);
        assert_eq!(verdict_from(&decision(0.30, 0.04), true), Gate::Skip);
        assert_eq!(verdict_from(&decision(0.25, 0.18), true), Gate::Skip);
        // publish-queue ran under the ordinary bar only on urgent 0.18 > 0.10.
        assert_eq!(verdict_from(&decision(0.25, 0.18), false), Gate::Run);
    }

    #[test]
    fn a_missing_answer_runs() {
        let mut d = decision(0.0, 0.0);
        d.answers.remove("urgent");
        assert_eq!(verdict_from(&d, false), Gate::Run);
        let mut d = decision(0.0, 0.0);
        d.answers.remove("worth_a_run");
        assert_eq!(verdict_from(&d, false), Gate::Run);
    }

    #[test]
    fn every_flag_counts_as_a_change() {
        assert!(!Flags::default().changed_anything());
        let each = [
            Flags { first_run: true, ..Default::default() },
            Flags { last_run_failed: true, ..Default::default() },
            Flags { new_messages: 1, ..Default::default() },
            Flags { other_runs: 1, ..Default::default() },
            Flags { new_assignments: 1, ..Default::default() },
            Flags { settings_changed: true, ..Default::default() },
        ];
        for f in each {
            assert!(f.changed_anything(), "{f:?}");
        }
    }

    #[test]
    fn the_floor_caps_consecutive_skips_and_the_span() {
        let two_min = Some(Duration::from_secs(120));
        // Five in a row are allowed; the sixth fire runs.
        assert!(floor_allows_skip(4, Some(120), two_min, false));
        assert!(!floor_allows_skip(MAX_CONSECUTIVE_SKIPS, Some(120), two_min, false));
        // Span: a 2-minute binding may go 10 minutes (5 cadences) without a run.
        assert_eq!(max_skip_span(two_min, false), Duration::from_secs(600));
        assert!(floor_allows_skip(0, Some(600), two_min, false));
        assert!(!floor_allows_skip(0, Some(601), two_min, false));
        // A slow binding is capped at 30 minutes, whatever its cadence.
        assert_eq!(max_skip_span(Some(Duration::from_secs(3600)), false), MAX_SKIP_SPAN);
        assert_eq!(max_skip_span(None, false), MAX_SKIP_SPAN);
        assert!(!floor_allows_skip(0, Some(31 * 60), None, false));
        // No run on record: never a skip.
        assert!(!floor_allows_skip(0, None, two_min, false));
    }

    #[test]
    fn a_standing_outcome_widens_the_span_to_cadences_capped_at_a_day() {
        let half_hour = Some(Duration::from_secs(30 * 60));
        // A 30-minute binding whose last run said why there was nothing to
        // do may go 8 cadences (4 hours), not 30 minutes.
        assert_eq!(max_skip_span(half_hour, true), Duration::from_secs(4 * 3600));
        assert!(floor_allows_skip(0, Some(31 * 60), half_hour, true));
        assert!(!floor_allows_skip(0, Some(31 * 60), half_hour, false));
        assert!(floor_allows_skip(0, Some(4 * 3600), half_hour, true));
        assert!(!floor_allows_skip(0, Some(4 * 3600 + 1), half_hour, true));
        // A slow cadence is capped at a day.
        assert_eq!(max_skip_span(Some(Duration::from_secs(6 * 3600)), true), STANDING_FLOOR_CAP);
        assert_eq!(max_skip_span(Some(Duration::from_secs(7 * 86_400)), true), STANDING_FLOOR_CAP);
        // No cadence: the ordinary span.
        assert_eq!(max_skip_span(None, true), MAX_SKIP_SPAN);
        // The consecutive cap still applies, and no run on record never skips.
        assert!(!floor_allows_skip(MAX_CONSECUTIVE_SKIPS, Some(60), half_hour, true));
        assert!(!floor_allows_skip(0, None, half_hour, true));
    }

    #[tokio::test]
    async fn a_standing_outcome_is_judged_not_forced_and_flags_still_win() {
        // A standing outcome past the ordinary 30-minute span is not a
        // change: the decision is asked, and a quiet answer skips.
        let key = "test:standing";
        let mut b = quiet(key);
        b.cadence = Some(Duration::from_secs(30 * 60));
        b.since_last_run = Some(60 * 60);
        b.standing = true;
        b.last_outcome = "Nothing matched this run; the list was empty.".into();
        assert!(!b.flags.changed_anything(), "a standing outcome is not a flag");
        let client = answering_client(0.05, 0.02).await;
        assert_eq!(triage(Some(&client), Mode::On, &b, T).await, Gate::Skip);
        // The same fire without the standing outcome is past the floor and runs unasked.
        let (counting, calls) = counting_client();
        let mut ordinary = b.clone();
        ordinary.key = "test:standing-ordinary".into();
        ordinary.standing = false;
        assert_eq!(triage(Some(&counting), Mode::On, &ordinary, T).await, Gate::Run);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // Any flag still runs it without asking.
        for f in [
            Flags { new_messages: 1, ..Default::default() },
            Flags { new_assignments: 1, ..Default::default() },
            Flags { settings_changed: true, ..Default::default() },
            Flags { other_runs: 1, ..Default::default() },
            Flags { last_run_failed: true, ..Default::default() },
        ] {
            let mut flagged = b.clone();
            flagged.flags = f;
            assert_eq!(triage(Some(&counting), Mode::On, &flagged, T).await, Gate::Run);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn set_names_lists_only_the_flags_that_are_set() {
        assert_eq!(Flags::default().set_names(), "");
        let f = Flags { other_runs: 2, settings_changed: true, ..Default::default() };
        assert_eq!(f.set_names(), "other_runs:2,settings_changed");
        let all = Flags { first_run: true, last_run_failed: true, new_messages: 1, other_runs: 3, new_assignments: 4, settings_changed: true };
        assert_eq!(all.set_names(), "first_run,last_run_failed,new_messages:1,other_runs:3,new_assignments:4,settings_changed");
    }

    #[tokio::test]
    async fn the_run_line_names_the_flags_that_forced_it() {
        #[derive(Clone, Default)]
        struct Buf(Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buf = Buf::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let mut b = quiet("test:log-flags");
        b.flags = Flags { other_runs: 2, settings_changed: true, ..Default::default() };
        assert_eq!(triage(None, Mode::On, &b, T).await, Gate::Run);
        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        let line = out.lines().find(|l| l.contains("binding=test:log-flags")).expect("a triage line");
        assert!(line.contains(r#"reason="changed""#), "{line}");
        assert!(line.contains("flags=other_runs:2,settings_changed"), "{line}");
    }

    #[tokio::test]
    async fn off_never_calls() {
        let (client, calls) = counting_client();
        let b = quiet("test:off");
        assert_eq!(triage(Some(&client), Mode::Off, &b, T).await, Gate::Run);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn flags_win_and_the_decision_is_never_called() {
        let (client, calls) = counting_client();
        let mut b = quiet("test:flags");
        b.flags.new_messages = 1;
        assert_eq!(triage(Some(&client), Mode::On, &b, T).await, Gate::Run);
        b.flags = Flags { first_run: true, ..Default::default() };
        b.since_last_run = None;
        assert_eq!(triage(Some(&client), Mode::On, &b, T).await, Gate::Run);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn the_floor_runs_without_asking() {
        let (client, calls) = counting_client();
        // Span exceeded.
        let mut b = quiet("test:span");
        b.since_last_run = Some(601);
        assert_eq!(triage(Some(&client), Mode::On, &b, T).await, Gate::Run);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // Five skips on record: the sixth fire runs, and the count resets.
        let key = "test:consecutive";
        for _ in 0..MAX_CONSECUTIVE_SKIPS {
            record(key, true);
        }
        assert_eq!(tally(key).consecutive_skips, MAX_CONSECUTIVE_SKIPS);
        assert_eq!(triage(Some(&client), Mode::On, &quiet(key), T).await, Gate::Run);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(tally(key).consecutive_skips, 0);
        assert_eq!(tally(key).skipped, MAX_CONSECUTIVE_SKIPS as u64);
        assert_eq!(tally(key).ran, 1);
    }

    #[tokio::test]
    async fn no_client_an_error_or_a_timeout_runs() {
        let b = quiet("test:failopen");
        assert_eq!(triage(None, Mode::On, &b, T).await, Gate::Run);
        // Error: no bearer fails as Auth.
        let (client, calls) = counting_client();
        assert_eq!(triage(Some(&client), Mode::On, &b, T).await, Gate::Run);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Timeout: a listener that accepts and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock);
            }
        });
        let hanging = DecideClient::new(&format!("http://{addr}"), || {
            Some(ai::Bearer { token: "t".into(), bot_id: None })
        });
        let started = std::time::Instant::now();
        let gate = triage(Some(&hanging), Mode::On, &b, Duration::from_millis(200)).await;
        assert_eq!(gate, Gate::Run);
        assert!(started.elapsed() < Duration::from_secs(2), "the passed-in timeout bounds the call");
        assert_eq!(tally("test:failopen").skipped, 0);
    }

    /// A client served by a local listener that answers every decision
    /// with the given Nouls, the wire shape Janus returns.
    async fn answering_client(worth: f64, urgent: f64) -> DecideClient {
        serving(serde_json::json!({
            "worth_a_run": {"type": "noul", "noul": worth},
            "urgent": {"type": "noul", "noul": urgent},
        }))
        .await
    }

    /// A client whose every decision comes back with `answers`.
    async fn serving(answers: serde_json::Value) -> DecideClient {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = serde_json::json!({
            "model": "jev-1.13.0",
            "answers": answers,
            "usage": {"input_tokens": 400, "output_tokens": 20, "cost_micro": 400},
        })
        .to_string();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let body = body.clone();
                tokio::spawn(async move {
                    // Read the whole request (headers, then Content-Length bytes).
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        let Ok(n) = sock.read(&mut chunk).await else { return };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf);
                        if let Some(end) = text.find("\r\n\r\n") {
                            let len = text[..end]
                                .lines()
                                .find_map(|l| {
                                    let (k, v) = l.split_once(':')?;
                                    k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
                                })
                                .unwrap_or(0);
                            if buf.len() >= end + 4 + len {
                                break;
                            }
                        }
                    }
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        DecideClient::new(&format!("http://{addr}"), || Some(ai::Bearer { token: "t".into(), bot_id: None }))
    }

    #[tokio::test]
    async fn a_quiet_decision_skips_when_on_and_counts_toward_the_floor() {
        let client = answering_client(0.05, 0.02).await;
        let key = "test:on-skip";
        assert_eq!(triage(Some(&client), Mode::On, &quiet(key), T).await, Gate::Skip);
        assert_eq!(tally(key).consecutive_skips, 1);
        // A decision that says the job is due runs, and resets the count.
        let due = answering_client(0.8, 0.02).await;
        assert_eq!(triage(Some(&due), Mode::On, &quiet(key), T).await, Gate::Run);
        assert_eq!(tally(key).consecutive_skips, 0);
        // Five skips in a row, then the floor runs the sixth without asking.
        for _ in 0..MAX_CONSECUTIVE_SKIPS {
            assert_eq!(triage(Some(&client), Mode::On, &quiet(key), T).await, Gate::Skip);
        }
        let (counting, calls) = counting_client();
        assert_eq!(triage(Some(&counting), Mode::On, &quiet(key), T).await, Gate::Run);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// A duty whose last outcome said it cannot be done until something is
    /// connected.
    fn walled(key: &str) -> Binding {
        let mut b = quiet(key);
        b.standing = true;
        b.duty = Some("inbox-sweep".into());
        b.declared = vec![Declared::Capability("mail".into()), Declared::Plugin("ledgerly".into())];
        b.last_outcome = "No mail provider is available to read the inbox. The referenced plugin is not installed, so nothing was read.".into();
        b
    }

    fn need_decision(missing: f64, which: &str, confidence: f64) -> Decision {
        let mut d = decision(0.64, 0.83);
        d.answers.insert("missing_need".into(), noul(missing));
        d.answers.insert(
            "which_need".into(),
            Answer { kind: "choice".into(), choice: Some(which.into()), score: None, noul: None, confidence: Some(confidence), probabilities: BTreeMap::new() },
        );
        d
    }

    #[test]
    fn a_missing_need_holds_at_the_inclusive_threshold_and_names_only_what_is_declared() {
        let b = walled("test:need-pure");
        let clause = "No mail provider is available to read the inbox.";
        assert_eq!(
            gate_from(&need_decision(MISSING_NEED_HOLD, "mail", WHICH_NEED_CONFIDENCE), &b),
            Gate::Hold(HeldNeed { which: Some(Declared::Capability("mail".into())), clause: clause.into() })
        );
        // Under the threshold it is the ordinary verdict (here: urgent runs).
        assert_eq!(gate_from(&need_decision(0.79, "mail", 0.9), &b), Gate::Run);
        // An unsure or unlisted choice holds without naming.
        for (which, conf) in [("mail", 0.59), ("other", 0.95), ("unclear", 0.95), ("fax", 0.95)] {
            assert_eq!(gate_from(&need_decision(0.9, which, conf), &b), Gate::Hold(HeldNeed { which: None, clause: clause.into() }), "{which} {conf}");
        }
        // No missing_need answer, or no binding to hold: never a hold.
        assert_eq!(gate_from(&decision(0.64, 0.83), &b), Gate::Run);
        let mut plain = b.clone();
        plain.duty = None;
        assert_eq!(gate_from(&need_decision(0.99, "mail", 0.99), &plain), Gate::Run);
        // The need questions are asked only of a binding, with its declared options.
        assert!(questions(&plain).get("missing_need").is_none());
        let asked = serde_json::to_value(&questions(&b)["which_need"]).unwrap().to_string();
        for option in ["mail", "ledgerly", "other", "unclear"] {
            assert!(asked.contains(&format!("\"{option}\"")), "{option} in {asked}");
        }
        // A long outcome is quoted as one capped clause.
        assert_eq!(first_clause(&"x".repeat(400)).chars().count(), CLAUSE_CAP + 1);
    }

    #[tokio::test]
    async fn a_missing_need_holds_the_fire_counts_toward_the_floor_and_flags_still_win() {
        let key = "test:need-hold";
        let b = walled(key);
        let client = serving(serde_json::json!({
            "worth_a_run": {"type": "noul", "noul": 0.64},
            "urgent": {"type": "noul", "noul": 0.83},
            "missing_need": {"type": "noul", "noul": 0.93},
            "which_need": {"type": "choice", "choice": "mail", "confidence": 0.88},
        }))
        .await;
        assert!(matches!(triage(Some(&client), Mode::On, &b, T).await, Gate::Hold(HeldNeed { which: Some(Declared::Capability(ref m)), .. }) if m == "mail"));
        assert_eq!(tally(key).consecutive_skips, 1, "a hold counts as a skip");
        // Shadow logs the hold and runs.
        assert_eq!(triage(Some(&client), Mode::Shadow, &walled("test:need-shadow"), T).await, Gate::Run);
        // A plugin installed or an account connected is a flag: runs unasked.
        let (counting, calls) = counting_client();
        let mut changed = walled("test:need-flag");
        changed.flags.settings_changed = true;
        assert_eq!(triage(Some(&counting), Mode::On, &changed, T).await, Gate::Run);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // Jev silent (no answer at all): the fire runs as today.
        let (silent, _) = counting_client();
        assert_eq!(triage(Some(&silent), Mode::On, &walled("test:need-silent"), T).await, Gate::Run);
    }

    #[tokio::test]
    async fn shadow_never_skips() {
        let client = answering_client(0.05, 0.02).await;
        let key = "test:shadow";
        // The verdict is a skip; shadow logs `would_skip` and runs.
        assert_eq!(verdict_from(&decision(0.05, 0.02), false), Gate::Skip);
        for _ in 0..(MAX_CONSECUTIVE_SKIPS + 2) {
            assert_eq!(triage(Some(&client), Mode::Shadow, &quiet(key), T).await, Gate::Run);
        }
        assert_eq!(triage(None, Mode::Shadow, &quiet(key), T).await, Gate::Run);
    }

    #[tokio::test]
    async fn an_incomplete_answer_runs() {
        let client = serving(serde_json::json!({"worth_a_run": {"type": "noul", "noul": 0.01}})).await;
        let key = "test:incomplete";
        assert_eq!(triage(Some(&client), Mode::On, &quiet(key), T).await, Gate::Run);
        assert_eq!(tally(key).skipped, 0);
        assert_eq!(tally(key).ran, 1);
    }

    #[test]
    fn state_names_the_purpose_the_outcome_line_and_elapsed_words() {
        let s = state(&quiet("test:state"));
        assert_eq!(s["purpose"], "Check the status of workflow run 5b11 and report results.");
        assert_eq!(s["last_run_status"], "done");
        assert_eq!(s["last_run_outcome"], "The workflow no longer exists.");
        assert_eq!(s["time_since_last_run"], "about 2 minutes");
        assert_eq!(s["unchanged_since_last_run"].as_array().unwrap().len(), 5);
        let mut empty = quiet("test:state");
        empty.purpose = "  ".into();
        empty.last_status.clear();
        empty.last_outcome.clear();
        empty.since_last_run = None;
        let s = state(&empty);
        assert_eq!(s["purpose"], "none");
        assert_eq!(s["last_run_status"], "unknown");
        assert_eq!(s["last_run_outcome"], "none recorded");
        assert_eq!(s["time_since_last_run"], "unknown");
        let mut long = quiet("test:state");
        long.purpose = "p".repeat(PURPOSE_CAP + 100);
        assert_eq!(state(&long)["purpose"].as_str().unwrap().len(), PURPOSE_CAP);
    }

    #[test]
    fn elapsed_reads_as_words() {
        assert_eq!(elapsed_label(20), "about a minute");
        assert_eq!(elapsed_label(120), "about 2 minutes");
        assert_eq!(elapsed_label(3600), "about 1 hour");
        assert_eq!(elapsed_label(59 * 60), "about 59 minutes");
        assert_eq!(elapsed_label(2 * 3600), "about 2 hours");
        assert_eq!(elapsed_label(3 * 86_400), "about 3 days");
        assert_eq!(elapsed_label(-5), "about a minute");
    }
}
