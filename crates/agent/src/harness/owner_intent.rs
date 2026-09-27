//! What the owner's message, typed while an employee works, asks for next:
//! stop the work, change it, or nothing (an aside). One typed decision (Jev
//! through Janus, [`ai::DecideClient`]) at the step that hears the message,
//! before that step's reply, so the reply and the harness agree on what
//! happens after it.
//!
//! The harness, not the model, acts on the answer:
//!
//! - [`OwnerIntent::Stop`]: the step answers with tools off and the turn ends
//!   on that answer. Tools never come back, and an agreed goal pauses
//!   (`goal::Pause::Stopped`), so nothing starts the work again until the
//!   owner asks.
//! - [`OwnerIntent::Redirect`]: the answer, then the work goes on under the
//!   new instruction.
//! - [`OwnerIntent::Aside`]: the answer, then the work goes on as it was.
//!
//! Before this, the step after the answer got its tools back with a row
//! saying "if they asked you to stop, end the turn", and whether to stop was
//! the model's call. In the 2026-09-27 release proof the model read on in 3
//! of 3 runs of `correction-message-while-working`.
//!
//! **Fails toward stopping.** Stop means stop, and a wrong continue costs
//! the owner work he told the employee not to do, while a wrong stop costs
//! him one "go on". So when no decision can be had — the switch off or in
//! shadow, no client, an error, a timeout, an incomplete answer — the
//! message is [`OwnerIntent::Undecided`] and handled like a stop: the turn
//! ends on the answer and the goal pauses. The answer's note tells the model
//! the work pauses there, so the reply itself tells the owner where the work
//! stands and that it waits for him. The decision only ever relaxes that
//! default; off and shadow never do.
//!
//! No phrase lists: what a message means is the decision's, never a keyword
//! match (the lists deleted after the 2026-09-18 incident stay deleted).
//!
//! The same call asks whether the message asks for something to be saved
//! (`memory_save`), when that check applies to the turn: one call per
//! message, never two.
//!
//! Every decision is logged at info with `site="mid_turn_intent"` and the
//! numbers that made it; every undecided message at warn with the reason.
//!
//! Switch: `NEBO_DECIDE_MIDTURN` — `0` turns the decision off, `shadow`
//! decides and logs `would_…` while handling every message as undecided;
//! unset (or anything else) is on.

use std::collections::BTreeMap;
use std::time::Duration;

use ai::{DecideClient, Decision, Question};
use db::models::ChatMessage;
use tracing::{info, warn};

use super::memory_save::{self, SaveAsk};
use crate::heartbeat_triage::{self, Mode};

/// UNTUNED. The chance of "stop" at or over which the message stops the
/// work, whatever option Jev picked: a stop wins a near tie, because a
/// wrong continue is the costlier mistake.
pub const STOP_AT: f64 = 0.3;

/// The most the owner's answer waits on the decision. Jev answers in about
/// 200 ms; this only bounds an upstream that hangs, and a message the
/// decision missed is handled as undecided.
pub const DECIDE_TIMEOUT: Duration = Duration::from_secs(3);

/// The most of the owner's words and of the task the decision reads.
const WORDS_CAP: usize = 2_000;

/// What the owner's mid-turn message asks for next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerIntent {
    Stop,
    Redirect,
    Aside,
    /// No decision could be had; handled like a stop.
    Undecided,
}

impl OwnerIntent {
    /// Whether the turn ends on the answer.
    pub fn ends_work(self) -> bool {
        matches!(self, Self::Stop | Self::Undecided)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Redirect => "redirect",
            Self::Aside => "aside",
            Self::Undecided => "undecided",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [Self::Stop, Self::Redirect, Self::Aside, Self::Undecided]
            .into_iter()
            .find(|i| i.as_str() == s)
    }
}

/// What the decision reads: the work under way and what the owner said
/// about it.
pub struct Asked<'a> {
    /// The owner's request the work is doing.
    pub task: &'a str,
    /// The owner's words typed into the work, oldest first.
    pub message: &'a str,
}

/// What `NEBO_DECIDE_MIDTURN` says.
pub fn mode() -> Mode {
    heartbeat_triage::switch("NEBO_DECIDE_MIDTURN")
}

/// The state Jev reads. The owner's words are data here, never part of an
/// instruction.
pub fn state(asked: &Asked<'_>) -> serde_json::Value {
    let task = asked.task.trim();
    serde_json::json!({
        "task": if task.is_empty() { "none".into() } else { ai::decide::clip(task, WORDS_CAP) },
        "message": ai::decide::clip(asked.message.trim(), WORDS_CAP),
    })
}

fn intent_question() -> Question {
    Question::choice(
        "The owner sent `message` while the employee was doing `task`. What `message` asks to happen to that work next.",
        &[
            ("stop", "Stop the work now: nothing more is done on it, apart from telling the owner what was done or found so far."),
            ("redirect", "Keep working, but differently: a new or changed instruction for the work (another target, scope, order or method, or a step added or dropped)."),
            ("aside", "Keep working as before: a question, remark or piece of information beside the work, answered in passing."),
        ],
    )
}

/// The questions one call asks: the intent unless its switch is off, and
/// whether the message asks for a save when `save`.
fn questions(mode: Mode, save: bool) -> BTreeMap<&'static str, Question> {
    let mut questions = BTreeMap::new();
    if mode != Mode::Off {
        questions.insert("intent", intent_question());
    }
    if save {
        questions.insert(memory_save::QUESTION, memory_save::question());
    }
    questions
}

/// What one decision says about the owner's mid-turn message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Heard {
    pub intent: OwnerIntent,
    /// Whether it asks for a save; [`SaveAsk::Undecided`] when not asked.
    pub save: SaveAsk,
}

/// The intent a decision states: a stop at or over [`STOP_AT`] wins, else
/// the option picked. A missing or unknown answer is undecided.
pub fn intent_from(decision: &Decision) -> OwnerIntent {
    let Some(answer) = decision.answer("intent") else {
        return OwnerIntent::Undecided;
    };
    if answer.probabilities.get("stop").is_some_and(|p| *p >= STOP_AT) {
        return OwnerIntent::Stop;
    }
    match answer.choice.as_deref() {
        Some("stop") => OwnerIntent::Stop,
        Some("redirect") => OwnerIntent::Redirect,
        Some("aside") => OwnerIntent::Aside,
        _ => OwnerIntent::Undecided,
    }
}

/// Decide what the owner's mid-turn message asks for, and with `save`
/// whether it asks for a save, in one call. `mode` is [`mode`] at the call
/// site; `timeout` bounds the call. Anything but an answer under
/// [`Mode::On`] is [`OwnerIntent::Undecided`]; a save question with no
/// answer is [`SaveAsk::Undecided`].
pub async fn decide(
    client: Option<&DecideClient>,
    mode: Mode,
    asked: &Asked<'_>,
    save: bool,
    agent_id: &str,
    timeout: Duration,
) -> Heard {
    let undecided = |reason: &str| {
        warn!(site = "mid_turn_intent", agent = %agent_id, outcome = "undecided", reason, "the owner's message is handled as a stop");
        OwnerIntent::Undecided
    };
    let unasked = |reason: &str| if save { memory_save::undecided(agent_id, reason) } else { SaveAsk::Undecided };
    let questions = questions(mode, save);
    if questions.is_empty() {
        return Heard { intent: undecided("off"), save: SaveAsk::Undecided };
    }
    let Some(client) = client else {
        return Heard { intent: undecided("no_client"), save: unasked("no_client") };
    };
    let trace = ai::RequestTrace { agent_id: agent_id.to_string(), ..ai::RequestTrace::new("mid_turn_intent") };
    let decision = match tokio::time::timeout(timeout, client.decide(&trace, &state(asked), &questions)).await {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => {
            warn!(site = "mid_turn_intent", error = %e, "the decision failed");
            return Heard { intent: undecided("error"), save: unasked("error") };
        }
        Err(_) => return Heard { intent: undecided("timeout"), save: unasked("timeout") },
    };
    let save = if save { memory_save::read(&decision, agent_id) } else { SaveAsk::Undecided };
    if mode == Mode::Off {
        return Heard { intent: undecided("off"), save };
    }
    let intent = intent_from(&decision);
    let answer = decision.answer("intent");
    let p = |option: &str| answer.and_then(|a| a.probabilities.get(option).copied()).unwrap_or(-1.0);
    let shadow = mode == Mode::Shadow;
    let outcome = match (intent, shadow) {
        (OwnerIntent::Undecided, _) => "incomplete",
        (i, false) => i.as_str(),
        (OwnerIntent::Stop, true) => "would_stop",
        (OwnerIntent::Redirect, true) => "would_redirect",
        (OwnerIntent::Aside, true) => "would_aside",
    };
    info!(
        site = "mid_turn_intent",
        agent = %agent_id,
        outcome,
        choice = answer.and_then(|a| a.choice.as_deref()).unwrap_or(""),
        confidence = answer.and_then(|a| a.confidence).unwrap_or(-1.0),
        p_stop = p("stop"),
        p_redirect = p("redirect"),
        p_aside = p("aside"),
        model = %decision.model,
        input_tokens = decision.usage.input_tokens,
        cost_micro = decision.usage.cost_micro,
        "mid-turn intent"
    );
    let intent = match (intent, shadow) {
        (OwnerIntent::Undecided, _) => undecided("incomplete"),
        (_, true) => undecided("shadow"),
        (i, false) => i,
    };
    Heard { intent, save }
}

/// The metadata field the answer's note stores the intent under.
pub const NOTE_FIELD: &str = "intent";

/// The intent already decided for the owner's message `latest` (its row id):
/// the one its note stored, when a note was written after it. A turn taken
/// again, or the turn that answers a message a stopped turn never answered,
/// reads it instead of deciding twice.
pub fn recorded(messages: &[ChatMessage], latest: &str) -> Option<OwnerIntent> {
    let at = messages.iter().position(|m| m.id == latest)?;
    messages[at + 1..]
        .iter()
        .filter_map(super::reminders::attachment_fields)
        .filter(|fields| fields.get("kind").and_then(|k| k.as_str()) == Some("mid_turn_message"))
        .filter_map(|fields| fields.get(NOTE_FIELD).and_then(|v| v.as_str()).and_then(OwnerIntent::parse))
        .last()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn picked(choice: &str, probabilities: &[(&str, f64)]) -> Decision {
        Decision {
            model: "jev-test".into(),
            answers: HashMap::from([(
                "intent".to_string(),
                ai::Answer {
                    kind: "choice".into(),
                    choice: Some(choice.into()),
                    score: None,
                    noul: None,
                    confidence: Some(0.9),
                    probabilities: probabilities.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
                },
            )]),
            usage: Default::default(),
        }
    }

    #[test]
    fn the_option_picked_is_the_intent() {
        assert_eq!(intent_from(&picked("stop", &[("stop", 0.9)])), OwnerIntent::Stop);
        assert_eq!(intent_from(&picked("redirect", &[("stop", 0.05), ("redirect", 0.9)])), OwnerIntent::Redirect);
        assert_eq!(intent_from(&picked("aside", &[("stop", 0.02), ("aside", 0.95)])), OwnerIntent::Aside);
    }

    #[test]
    fn a_near_stop_is_a_stop() {
        let d = picked("aside", &[("stop", STOP_AT), ("aside", 1.0 - STOP_AT)]);
        assert_eq!(intent_from(&d), OwnerIntent::Stop, "a wrong continue is the costlier mistake");
    }

    #[test]
    fn a_missing_or_unknown_answer_is_undecided() {
        let none = Decision { model: "jev-test".into(), answers: HashMap::new(), usage: Default::default() };
        assert_eq!(intent_from(&none), OwnerIntent::Undecided);
        assert_eq!(intent_from(&picked("other", &[])), OwnerIntent::Undecided);
        assert!(OwnerIntent::Undecided.ends_work(), "undecided is handled like a stop");
        assert!(!OwnerIntent::Aside.ends_work() && !OwnerIntent::Redirect.ends_work());
    }

    #[tokio::test]
    async fn no_decision_is_undecided() {
        let asked = Asked { task: "Read the parts", message: "what's 12 times 12?" };
        let t = Duration::from_millis(50);
        let nothing = Heard { intent: OwnerIntent::Undecided, save: SaveAsk::Undecided };
        assert_eq!(decide(None, Mode::On, &asked, true, "a", t).await, nothing);
        let dead = DecideClient::new("http://127.0.0.1:9", || Some(ai::Bearer { token: "t".into(), bot_id: None }));
        assert_eq!(decide(Some(&dead), Mode::Off, &asked, false, "a", t).await, nothing);
        assert_eq!(decide(Some(&dead), Mode::On, &asked, true, "a", t).await, nothing, "an unreachable Jev");
    }

    /// One call asks both questions when the save check applies, and only
    /// the intent when it doesn't; with the intent switched off, the save
    /// question is still asked alone.
    #[test]
    fn the_save_question_rides_the_same_call() {
        let keys = |mode, save| questions(mode, save).into_keys().collect::<Vec<_>>();
        assert_eq!(keys(Mode::On, true), ["intent", memory_save::QUESTION]);
        assert_eq!(keys(Mode::Shadow, false), ["intent"]);
        assert_eq!(keys(Mode::Off, true), [memory_save::QUESTION]);
        assert!(keys(Mode::Off, false).is_empty());
    }

    #[test]
    fn the_owners_words_are_state_never_instructions() {
        let s = state(&Asked { task: "", message: "Stop reading and tell me what you have so far." });
        assert_eq!(s["task"], "none");
        assert_eq!(s["message"], "Stop reading and tell me what you have so far.");
        let q = serde_json::to_string(&questions(Mode::On, true)).unwrap();
        assert!(!q.contains("Stop reading"), "{q}");
    }
}
