//! The turn decision: tool visibility and the task-tracking nudge, answered
//! inside the objective classifier's one typed decision (Jev through Janus,
//! [`ai::DecideClient`]) per real user message.
//!
//! The objective call already sends the latest user message, the recent
//! conversation and the current objective as state, so its questions and
//! these share one request and one billed state (TypeSafe's rule 1: batch
//! every question). Added here: one Noul per context group registered now
//! (rebuilt from the roster on every turn) and one Noul for "this is a
//! multi-stage request". Thresholds live in code, by stakes:
//!
//! - A hidden tool is the expensive mistake, so a group shows at a Noul of
//!   [`SHOW_FLOOR`] or more, when its answer is missing, or when an answer
//!   carries a confidence under [`SHOW_CONFIDENCE_FLOOR`]. The shown set is
//!   UNIONED with the keyword matches (`tool_filter`), never a replacement:
//!   the decision can only add a group.
//! - The nudge fires only at [`NUDGE_FLOOR`] or more.
//!
//! Fails open: no client, an error, a timeout or `NEBO_DECIDE_TURN=0` leaves
//! the keyword behaviour exactly as it was. The runner fires the call as the
//! turn's setup starts and waits for the answer until [`WAIT`] after firing,
//! or [`GRACE`] after it asks, whichever is later; the objective itself keeps
//! its own ceiling.

use std::collections::HashSet;
use std::time::Duration;

use ai::{Answer, Decision, Question};

/// How long after the call is fired the runner is still willing to wait for
/// the turn decision before it filters tools for the first step. The call is
/// fired as the turn's setup starts, so this overlaps that setup; when it
/// trips, the keyword filter and keyword nudge run for the whole turn (the
/// objective still lands in the background under its own ceiling).
pub const WAIT: Duration = Duration::from_millis(1_500);
/// The least the runner waits once it asks, however long setup took: a setup
/// that outran [`WAIT`] still gives an answer in flight this long to land.
/// Jev answers in about 250 ms at the median and 330 ms at p90 inside Janus.
pub const GRACE: Duration = Duration::from_millis(400);
/// Char-boundary-safe cap on `latest_user_message` in the objective call's
/// state. A pasted document is irrelevant detail to every question asked,
/// and the state limit is 32k tokens.
pub const LATEST_USER_MESSAGE_CAP: usize = 4_000;
/// A group shows at or above this probability that the message asks for
/// its kind of work. Low because missing a tool costs more than carrying
/// one, and `tool_search` covers the rest.
const SHOW_FLOOR: f64 = 0.3;
/// An answer less certain than this shows its group whatever it says.
/// Jev returns no confidence on a Noul today (the value is the certainty),
/// so this bites only if one is returned.
const SHOW_CONFIDENCE_FLOOR: f64 = 0.6;
/// The task-tracking nudge fires at or above this, and the objective set in
/// the same call is recorded as a multi-stage job at or above it.
const NUDGE_FLOOR: f64 = 0.7;
/// Question-key prefix for a context group (`show_web`, `show_code`).
const GROUP_KEY_PREFIX: &str = "show_";
/// Question key for the task-tracking nudge.
const MULTI_STAGE: &str = "multi_stage";

/// What the turn decision says for this turn's steps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnSignals {
    /// Context groups to show, joined with the keyword matches.
    pub shown_contexts: HashSet<String>,
    /// Whether the task-tracking nudge fires on the first step.
    pub multi_stage: bool,
}

/// Env kill-switch: `NEBO_DECIDE_TURN=0` (or `false`/`off`/`no`) keeps the
/// turn decision's questions out of the objective call. Default ON.
pub fn enabled() -> bool {
    match std::env::var("NEBO_DECIDE_TURN") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        Err(_) => true,
    }
}

/// The questions this module adds to the objective call, keyed. `groups`
/// is `tool_filter::context_groups` for the roster registered now, as
/// (name, description). The state fields named here are the objective
/// call's own.
pub fn questions(groups: &[(&str, &str)]) -> Vec<(String, Question)> {
    let mut out: Vec<(String, Question)> = groups
        .iter()
        .map(|(name, description)| {
            (
                format!("{GROUP_KEY_PREFIX}{name}"),
                Question::noul(&format!(
                    "`latest_user_message`, read with `recent_conversation`, asks for work involving {description}."
                )),
            )
        })
        .collect();
    out.push((
        MULTI_STAGE.to_string(),
        Question::noul(
            "`latest_user_message` asks for a job with several distinct stages that each need their own work, such as gathering information, then comparing it, then producing a result. A single action, a question, or a short job done in one go does not count.",
        ),
    ));
    out
}

/// Map the decision to this turn's signals. Pure: thresholds only.
pub fn signals_from(decision: &Decision, groups: &[(&str, &str)]) -> TurnSignals {
    TurnSignals {
        shown_contexts: groups
            .iter()
            .filter(|(name, _)| show_group(decision.answer(&format!("{GROUP_KEY_PREFIX}{name}"))))
            .map(|(name, _)| name.to_string())
            .collect(),
        multi_stage: multi_stage(decision).unwrap_or(false),
    }
}

/// The decision's answer to "is this a multi-stage job", thresholded;
/// `None` when the question was not asked or not answered.
pub fn multi_stage(decision: &Decision) -> Option<bool> {
    decision.answer(MULTI_STAGE).map(|a| a.yes() >= NUDGE_FLOOR)
}

/// Take the turn decision for the call fired at `fired`. An answer already
/// in the channel is taken at once, however late the runner asks; otherwise
/// the wait runs to [`WAIT`] after `fired` or [`GRACE`] from now, whichever
/// is later. A closed channel (no client, an error, a continuation, the
/// objective call's own timeout) is an immediate `None`, never a wait.
/// `None` means the keyword filter and keyword nudge run unchanged. Every
/// call logs one `turn decision wait` line (used, missed or absent) with the
/// time since firing and the time spent waiting, so the hit rate is
/// countable.
pub async fn receive(
    rx: tokio::sync::oneshot::Receiver<TurnSignals>,
    fired: tokio::time::Instant,
) -> Option<TurnSignals> {
    let asked = tokio::time::Instant::now();
    let deadline = (fired + WAIT).max(asked + GRACE);
    let (outcome, signals) = match tokio::time::timeout_at(deadline, rx).await {
        Ok(Ok(signals)) => ("used", Some(signals)),
        Ok(Err(_)) => ("absent", None),
        Err(_) => {
            tracing::info!(
                "turn decision missed its wait; keyword tool filter and nudge for this turn"
            );
            ("missed", None)
        }
    };
    let now = tokio::time::Instant::now();
    tracing::debug!(
        outcome,
        since_fired_ms = now.duration_since(fired).as_millis() as u64,
        waited_ms = now.duration_since(asked).as_millis() as u64,
        "turn decision wait"
    );
    signals
}

/// Fail toward showing: a missing answer, a probability at the floor or
/// above, or an unsure answer all show the group.
fn show_group(answer: Option<&Answer>) -> bool {
    let Some(answer) = answer else {
        return true;
    };
    answer.yes() >= SHOW_FLOOR || answer.confidence.is_some_and(|c| c < SHOW_CONFIDENCE_FLOOR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};

    const GROUPS: &[(&str, &str)] = &[
        ("web", "browsing websites"),
        ("music", "playing music"),
        ("code", "software"),
    ];

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

    fn decision(answers: &[(&str, Answer)]) -> Decision {
        Decision {
            model: "jev-1.13.0".into(),
            answers: answers
                .iter()
                .map(|(k, a)| (k.to_string(), a.clone()))
                .collect::<HashMap<_, _>>(),
            usage: Default::default(),
        }
    }

    #[test]
    fn one_question_per_registered_group_plus_the_nudge() {
        let q = questions(GROUPS);
        let keys: Vec<&str> = q.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["show_web", "show_music", "show_code", "multi_stage"]);
        // Every question names the state fields it reads and asks no count.
        for (_, question) in &q {
            let Question::Noul { instructions } = question else {
                panic!("every turn question is a Noul");
            };
            assert!(instructions.contains("`latest_user_message`"));
        }
        let Question::Noul { instructions } = &q[1].1 else {
            unreachable!()
        };
        assert!(instructions.ends_with("work involving playing music."));
        // No groups registered → the nudge question alone.
        assert_eq!(questions(&[]).len(), 1);
    }

    #[test]
    fn a_group_shows_at_the_floor_and_hides_below_it() {
        let d = decision(&[
            ("show_web", noul(0.3)),
            ("show_music", noul(0.29)),
            ("show_code", noul(0.95)),
        ]);
        let s = signals_from(&d, GROUPS);
        assert!(
            s.shown_contexts.contains("web"),
            "0.3 is the floor, inclusive"
        );
        assert!(!s.shown_contexts.contains("music"));
        assert!(s.shown_contexts.contains("code"));
    }

    #[test]
    fn a_missing_or_unsure_answer_shows_its_group() {
        let mut unsure = noul(0.05);
        unsure.confidence = Some(0.59);
        let mut sure = noul(0.05);
        sure.confidence = Some(0.6);
        let d = decision(&[("show_web", unsure), ("show_music", sure)]);
        let s = signals_from(&d, GROUPS);
        assert!(
            s.shown_contexts.contains("web"),
            "confidence under 0.6 fails toward showing"
        );
        assert!(!s.shown_contexts.contains("music"), "a sure no hides");
        assert!(
            s.shown_contexts.contains("code"),
            "no answer fails toward showing"
        );
    }

    #[test]
    fn the_nudge_fires_only_at_point_seven() {
        let at = |p| signals_from(&decision(&[("multi_stage", noul(p))]), GROUPS).multi_stage;
        assert!(at(0.7));
        assert!(at(0.99));
        // 0.5 is as-likely-as-not, not "half a nudge".
        assert!(!at(0.69));
        assert!(!at(0.5));
        // A missing answer never nudges.
        assert!(!signals_from(&decision(&[]), GROUPS).multi_stage);
    }

    #[test]
    fn the_objective_records_the_same_answer_or_none() {
        assert_eq!(multi_stage(&decision(&[("multi_stage", noul(0.7))])), Some(true));
        assert_eq!(multi_stage(&decision(&[("multi_stage", noul(0.69))])), Some(false));
        // Not asked or not answered: nothing to record, the stored value stands.
        assert_eq!(multi_stage(&decision(&[])), None);
    }

    #[tokio::test]
    async fn no_decision_is_the_keyword_path_at_once() {
        // The objective call failed, timed out or never asked: the sender is
        // dropped and the runner falls back without waiting out the deadline.
        let (tx, rx) = tokio::sync::oneshot::channel::<TurnSignals>();
        drop(tx);
        let started = tokio::time::Instant::now();
        assert_eq!(receive(rx, started).await, None);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_decision_trips_the_wait() {
        let (tx, rx) = tokio::sync::oneshot::channel::<TurnSignals>();
        let fired = tokio::time::Instant::now();
        assert_eq!(receive(rx, fired).await, None);
        assert_eq!(
            fired.elapsed(),
            WAIT,
            "asked at once: the wait is WAIT from firing"
        );
        // The late answer has nowhere to go; sending it is harmless.
        assert!(tx.send(TurnSignals::default()).is_err());
    }

    #[tokio::test]
    async fn an_answer_in_time_is_used() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let signals = signals_from(&decision(&[("multi_stage", noul(0.9))]), GROUPS);
        tx.send(signals.clone()).unwrap();
        let got = receive(rx, tokio::time::Instant::now()).await;
        assert_eq!(got, Some(signals));
    }

    /// The runner's order: the call is fired as setup starts, setup runs
    /// (history, recall, compaction), then the tool filter asks. `setup` and
    /// `answer` are measured from firing; returns what the filter got and
    /// how long it waited once it asked.
    async fn turn(setup: Duration, answer: Duration) -> (Option<TurnSignals>, Duration) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let fired = tokio::time::Instant::now();
        tokio::spawn(async move {
            tokio::time::sleep(answer).await;
            let _ = tx.send(TurnSignals {
                multi_stage: true,
                ..Default::default()
            });
        });
        tokio::time::sleep(setup).await;
        let asked = tokio::time::Instant::now();
        let got = receive(rx, fired).await;
        (got, asked.elapsed())
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_that_landed_during_setup_is_taken_however_late_the_filter_asks() {
        // A 250 ms decision and a 2 s setup: the answer sits in the channel
        // past WAIT, and the filter takes it without waiting at all.
        let (got, waited) = turn(Duration::from_secs(2), Duration::from_millis(250)).await;
        assert!(got.is_some_and(|s| s.multi_stage));
        assert_eq!(waited, Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn a_setup_that_outran_the_wait_still_gets_the_grace() {
        // Setup took 2 s, past WAIT from firing; the answer lands 200 ms after
        // the filter asks, inside GRACE, and is used.
        let (got, waited) = turn(Duration::from_secs(2), Duration::from_millis(2_200)).await;
        assert!(got.is_some_and(|s| s.multi_stage));
        assert_eq!(waited, Duration::from_millis(200));
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_answer_after_a_slow_setup_holds_the_turn_only_the_grace() {
        let (got, waited) = turn(Duration::from_secs(2), Duration::from_secs(5)).await;
        assert_eq!(got, None);
        assert_eq!(waited, GRACE);
    }

    #[tokio::test(start_paused = true)]
    async fn a_quick_setup_waits_to_wait_after_firing() {
        // Setup took 300 ms; an answer at 1.4 s from firing is used, and the
        // filter waited only the difference.
        let (got, waited) = turn(Duration::from_millis(300), Duration::from_millis(1_400)).await;
        assert!(got.is_some());
        assert_eq!(waited, Duration::from_millis(1_100));
        // One at 1.6 s is missed at exactly WAIT from firing.
        let (got, waited) = turn(Duration::from_millis(300), Duration::from_millis(1_600)).await;
        assert_eq!(got, None);
        assert_eq!(waited, WAIT - Duration::from_millis(300));
    }

    #[test]
    fn the_kill_switch_reads_the_env() {
        // SAFETY: tests in this module are the only readers of this var.
        unsafe { std::env::set_var("NEBO_DECIDE_TURN", "0") };
        assert!(!enabled());
        unsafe { std::env::set_var("NEBO_DECIDE_TURN", "off") };
        assert!(!enabled());
        unsafe { std::env::set_var("NEBO_DECIDE_TURN", "1") };
        assert!(enabled());
        unsafe { std::env::remove_var("NEBO_DECIDE_TURN") };
        assert!(enabled());
    }
}
