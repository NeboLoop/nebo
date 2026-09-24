//! The turn decision: the task-tracking nudge, answered inside the
//! objective classifier's one typed decision (Jev through Janus,
//! [`ai::DecideClient`]) per real user message.
//!
//! The objective call already sends the latest user message, the recent
//! conversation and the current objective as state, so this question and
//! those share one request and one billed state (TypeSafe's rule 1: batch
//! every question). The nudge fires only at [`NUDGE_FLOOR`] or more.
//!
//! Fails open: no client, an error, a timeout or `NEBO_DECIDE_TURN=0` leaves
//! the keyword nudge exactly as it was. The runner fires the call as the
//! turn's setup starts and waits for the answer until [`WAIT`] after firing,
//! or [`GRACE`] after it asks, whichever is later; the objective itself keeps
//! its own ceiling.

use std::time::Duration;

use ai::{Decision, Question};

/// How long after the call is fired the runner is still willing to wait for
/// the turn decision before its first step. The call is fired as the turn's
/// setup starts, so this overlaps that setup; when it trips, the keyword
/// nudge runs for the whole turn (the objective still lands in the
/// background under its own ceiling).
pub const WAIT: Duration = Duration::from_millis(1_500);
/// The least the runner waits once it asks, however long setup took: a setup
/// that outran [`WAIT`] still gives an answer in flight this long to land.
/// Jev answers in about 250 ms at the median and 330 ms at p90 inside Janus.
pub const GRACE: Duration = Duration::from_millis(400);
/// Char-boundary-safe cap on `latest_user_message` in the objective call's
/// state. A pasted document is irrelevant detail to every question asked,
/// and the state limit is 32k tokens.
pub const LATEST_USER_MESSAGE_CAP: usize = 4_000;
/// The task-tracking nudge fires at or above this.
const NUDGE_FLOOR: f64 = 0.7;
/// Question key for the task-tracking nudge.
const MULTI_STAGE: &str = "multi_stage";

/// What the turn decision says for this turn's steps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnSignals {
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

/// The question this module adds to the objective call, keyed. The state
/// fields named here are the objective call's own.
pub fn questions() -> Vec<(String, Question)> {
    vec![(
        MULTI_STAGE.to_string(),
        Question::noul(
            "`latest_user_message` asks for a job with several distinct stages that each need their own work, such as gathering information, then comparing it, then producing a result. A single action, a question, or a short job done in one go does not count.",
        ),
    )]
}

/// Map the decision to this turn's signals. Pure: thresholds only.
pub fn signals_from(decision: &Decision) -> TurnSignals {
    TurnSignals {
        multi_stage: decision
            .answer(MULTI_STAGE)
            .is_some_and(|a| a.yes() >= NUDGE_FLOOR),
    }
}

/// Take the turn decision for the call fired at `fired`. An answer already
/// in the channel is taken at once, however late the runner asks; otherwise
/// the wait runs to [`WAIT`] after `fired` or [`GRACE`] from now, whichever
/// is later. A closed channel (no client, an error, a continuation, the
/// objective call's own timeout) is an immediate `None`, never a wait.
/// `None` means the keyword nudge runs unchanged. Every
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
                "turn decision missed its wait; keyword nudge for this turn"
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

#[cfg(test)]
mod tests {
    use super::*;
    use ai::Answer;
    use std::collections::{BTreeMap, HashMap};

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
    fn the_one_question_is_the_nudge() {
        let q = questions();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].0, "multi_stage");
        let Question::Noul { instructions } = &q[0].1 else {
            panic!("the turn question is a Noul");
        };
        assert!(instructions.contains("`latest_user_message`"));
    }

    #[test]
    fn the_nudge_fires_only_at_point_seven() {
        let at = |p| signals_from(&decision(&[("multi_stage", noul(p))])).multi_stage;
        assert!(at(0.7));
        assert!(at(0.99));
        // 0.5 is as-likely-as-not, not "half a nudge".
        assert!(!at(0.69));
        assert!(!at(0.5));
        // A missing answer never nudges.
        assert!(!signals_from(&decision(&[])).multi_stage);
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
        let signals = signals_from(&decision(&[("multi_stage", noul(0.9))]));
        tx.send(signals.clone()).unwrap();
        let got = receive(rx, tokio::time::Instant::now()).await;
        assert_eq!(got, Some(signals));
    }

    /// The runner's order: the call is fired as setup starts, setup runs
    /// (history, recall, compaction), then the first step asks. `setup` and
    /// `answer` are measured from firing; returns what the filter got and
    /// how long it waited once it asked.
    async fn turn(setup: Duration, answer: Duration) -> (Option<TurnSignals>, Duration) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let fired = tokio::time::Instant::now();
        tokio::spawn(async move {
            tokio::time::sleep(answer).await;
            let _ = tx.send(TurnSignals { multi_stage: true });
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
