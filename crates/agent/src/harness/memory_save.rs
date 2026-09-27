//! A save the owner asked for is a `remember` call that succeeded, never a
//! reply that says "Saved". In the 2026-09-27 memory sweep
//! (`suites/memory.yaml`) 6 of 21 first turns told the owner a fact was
//! saved with no `remember` call, and the replay of the owner's own bug (m07,
//! "save that recipe to company memory") claimed a save under an invented
//! key in 3 of 3 runs. Nothing checked the claim against the turn's calls.
//!
//! Whether the owner's message asks for something to be kept is one typed
//! decision (Jev through Janus, [`ai::DecideClient`]), made once per message:
//!
//! - the message that starts the turn: decided from Prepare, while the steps
//!   run, and read when the turn would end, so it costs the turn no wait;
//! - a message typed into the running work: asked in the same call that
//!   decides its intent (`owner_intent::decide`), never a second call.
//!
//! The harness, not the model, acts on it: when the turn would end (a reply
//! with no tool calls) and the owner asked, but no `remember` call has
//! succeeded since, the end check ([`SaveCheck`]) takes ONE more step whose
//! note tells the model nothing is saved yet. At most once per turn, so a
//! model that cannot save ends the turn saying so.
//!
//! **Fails open.** When no decision can be had — the switch off, no client,
//! an error, a timeout, an incomplete answer — the owner's message counts as
//! not asking, and the turn ends as it would have. A wrong correction costs
//! a model step and can put into memory, where local memory is read by every
//! employee on this Nebo, something the owner never asked to keep; a missed
//! one leaves the claim to the prompt, as before this check. Unlike a stop
//! (`owner_intent` fails toward stopping), staying quiet never does work the
//! owner forbade.
//!
//! No phrase lists: whether a message asks for a save is the decision's,
//! never a keyword match.
//!
//! Every decision is logged at info with `site="save_ask"` and the
//! probability that made it; an undecided one at warn with the reason. Every
//! correction is logged at info with `site="unsaved_memory"`.
//!
//! Switch: `NEBO_DECIDE_SAVE` — `0` turns it off (no decision is asked),
//! `shadow` decides and logs `would_correct` without ever correcting; unset
//! (or anything else) is on.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ai::{DecideClient, Decision, Question};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use super::events::TurnEvent;
use super::turn_end::{EndCheck, EndVerdict, TurnEnd};
use crate::heartbeat_triage::{self, Mode};

/// UNTUNED. The chance that the message asks for a save at or over which it
/// counts as asking. Set by hand before any shadow run; `shadow` logs every
/// `p_save`, and that data sets it.
pub const SAVE_AT: f64 = 0.5;

/// The most the turn's end waits on the opening message's decision. Jev
/// answers in about 200 ms and the decision started at Prepare, so a turn
/// that took a step has it already; this only bounds an upstream that hangs.
pub const DECIDE_TIMEOUT: Duration = Duration::from_secs(3);

/// The end check's name, and the key the question is asked under.
pub const CHECK: &str = "unsaved_memory";
pub const QUESTION: &str = "save";

/// The most of the owner's words the decision reads.
const WORDS_CAP: usize = 2_000;

/// What the owner's message says about keeping something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveAsk {
    Asked,
    NotAsked,
    /// No decision could be had; handled as not asking.
    Undecided,
}

/// What `NEBO_DECIDE_SAVE` says.
pub fn mode() -> Mode {
    heartbeat_triage::switch("NEBO_DECIDE_SAVE")
}

/// The question, about the state's `message`: asked alone about the message
/// that starts the turn, beside the intent about a message typed into the
/// work.
pub fn question() -> Question {
    Question::Noul {
        instructions: "`message` asks the employee to save, store or remember something (a fact, a note, a \
                       recipe, a preference, an instruction) so that it can be found again later."
            .to_string(),
    }
}

/// What a decision says. Logged: the answer at info, a missing one at warn.
pub fn read(decision: &Decision, agent_id: &str) -> SaveAsk {
    let Some(p) = decision.answer(QUESTION).and_then(|a| a.noul) else {
        return undecided(agent_id, "incomplete");
    };
    let ask = if p >= SAVE_AT { SaveAsk::Asked } else { SaveAsk::NotAsked };
    info!(
        site = "save_ask",
        agent = %agent_id,
        outcome = if ask == SaveAsk::Asked { "asked" } else { "not_asked" },
        p_save = p,
        model = %decision.model,
        input_tokens = decision.usage.input_tokens,
        cost_micro = decision.usage.cost_micro,
        "save ask"
    );
    ask
}

/// A message no decision could be had for, logged with why.
pub fn undecided(agent_id: &str, reason: &str) -> SaveAsk {
    warn!(site = "save_ask", agent = %agent_id, outcome = "undecided", reason, "the owner's message counts as not asking for a save");
    SaveAsk::Undecided
}

/// Decide whether the message that starts the turn asks for a save: one
/// call, one question. Anything but an answer is [`SaveAsk::Undecided`].
pub async fn decide(client: Option<&DecideClient>, message: &str, agent_id: &str, timeout: Duration) -> SaveAsk {
    let Some(client) = client else {
        return undecided(agent_id, "no_client");
    };
    let state = serde_json::json!({ "message": ai::decide::clip(message.trim(), WORDS_CAP) });
    let questions = BTreeMap::from([(QUESTION, question())]);
    let trace = ai::RequestTrace { agent_id: agent_id.to_string(), ..ai::RequestTrace::new("save_ask") };
    match tokio::time::timeout(timeout, client.decide(&trace, &state, &questions)).await {
        Ok(Ok(decision)) => read(&decision, agent_id),
        Ok(Err(e)) => {
            warn!(site = "save_ask", error = %e, "the decision failed");
            undecided(agent_id, "error")
        }
        Err(_) => undecided(agent_id, "timeout"),
    }
}

/// What the turn knows about the saves the owner asked for.
pub struct SaveWatch {
    mode: Mode,
    agent_id: String,
    /// The decision on the message that starts the turn, running since
    /// Prepare; read once, when the turn would first end.
    opening: Option<JoinHandle<SaveAsk>>,
    /// The step the latest asking message was heard at: a save must have
    /// succeeded at it or after.
    asked_at: Option<u32>,
    /// The latest step whose tool round saved a memory.
    saved_at: Option<u32>,
    /// The check ran its one correction (or, in shadow, logged it).
    spent: bool,
}

impl SaveWatch {
    /// A turn nothing is checked on.
    pub fn off() -> SaveWatch {
        SaveWatch { mode: Mode::Off, agent_id: String::new(), opening: None, asked_at: None, saved_at: None, spent: true }
    }

    /// Start watching a turn. `applies` is whether the owner speaks in it
    /// and the employee can save (`remember` in reach, memory writes on);
    /// `message` is the owner's words that start it, if any.
    pub fn start(client: Option<Arc<DecideClient>>, message: Option<&str>, applies: bool, agent_id: &str) -> SaveWatch {
        let mode = mode();
        if !applies || mode == Mode::Off {
            return SaveWatch::off();
        }
        let opening = message.map(str::trim).filter(|m| !m.is_empty()).map(|message| {
            let message = message.to_string();
            let agent = agent_id.to_string();
            tokio::spawn(async move { decide(client.as_deref(), &message, &agent, DECIDE_TIMEOUT).await })
        });
        SaveWatch { mode, agent_id: agent_id.to_string(), opening, asked_at: None, saved_at: None, spent: false }
    }

    /// Whether a message typed into the work is asked about too.
    pub fn applies(&self) -> bool {
        self.mode != Mode::Off
    }

    /// A message typed into the work was decided at `step`.
    pub fn heard(&mut self, step: u32, ask: SaveAsk) {
        if ask == SaveAsk::Asked {
            self.asked_at = Some(step);
        }
    }

    /// The tool round at `step` saved a memory.
    pub fn saved(&mut self, step: u32) {
        self.saved_at = Some(step);
    }

    /// The turn would end: the check to run, when the owner asked for a save
    /// that no `remember` call answers yet. Once per turn.
    pub async fn due(&mut self) -> Option<SaveCheck> {
        if self.spent {
            return None;
        }
        if let Some(opening) = self.opening.take()
            && opening.await.unwrap_or(SaveAsk::Undecided) == SaveAsk::Asked
        {
            // The turn's first step heard it; a later ask keeps its own step.
            self.asked_at.get_or_insert(1);
        }
        let asked_at = self.asked_at?;
        if self.saved_at.is_some_and(|saved| saved >= asked_at) {
            return None;
        }
        self.spent = true;
        Some(SaveCheck { shadow: self.mode == Mode::Shadow, agent_id: self.agent_id.clone() })
    }
}

impl Drop for SaveWatch {
    /// A turn that ends before its decision came back stops it.
    fn drop(&mut self) {
        if let Some(opening) = self.opening.take() {
            opening.abort();
        }
    }
}

/// The owner asked for a save and none has succeeded: one more step, told so.
pub struct SaveCheck {
    shadow: bool,
    agent_id: String,
}

#[async_trait::async_trait]
impl EndCheck for SaveCheck {
    fn name(&self) -> &'static str {
        CHECK
    }

    async fn check(&self, end: &TurnEnd<'_>) -> EndVerdict {
        let outcome = if self.shadow { "would_correct" } else { "corrected" };
        info!(site = CHECK, agent = %self.agent_id, outcome, step = end.step, "the owner asked for a save and none has succeeded");
        if self.shadow {
            return EndVerdict::Stop;
        }
        EndVerdict::Continue(TurnEvent::UnsavedMemory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn said(p: Option<f64>) -> Decision {
        let answers = p
            .map(|p| {
                HashMap::from([(
                    QUESTION.to_string(),
                    ai::Answer { kind: "noul".into(), choice: None, score: None, noul: Some(p), confidence: None, probabilities: Default::default() },
                )])
            })
            .unwrap_or_default();
        Decision { model: "jev-test".into(), answers, usage: Default::default() }
    }

    #[test]
    fn the_probability_decides_and_a_missing_answer_is_undecided() {
        assert_eq!(read(&said(Some(0.93)), "a"), SaveAsk::Asked);
        assert_eq!(read(&said(Some(SAVE_AT)), "a"), SaveAsk::Asked);
        assert_eq!(read(&said(Some(0.1)), "a"), SaveAsk::NotAsked);
        assert_eq!(read(&said(None), "a"), SaveAsk::Undecided);
    }

    #[tokio::test]
    async fn no_decision_is_undecided() {
        let t = Duration::from_millis(50);
        assert_eq!(decide(None, "save this recipe", "a", t).await, SaveAsk::Undecided);
        let dead = DecideClient::new("http://127.0.0.1:9", || Some(ai::Bearer { token: "t".into(), bot_id: None }));
        assert_eq!(decide(Some(&dead), "save this recipe", "a", t).await, SaveAsk::Undecided, "an unreachable Jev");
    }

    fn watch(opening: Option<SaveAsk>) -> SaveWatch {
        SaveWatch {
            mode: Mode::On,
            agent_id: "a".into(),
            opening: opening.map(|ask| tokio::spawn(async move { ask })),
            asked_at: None,
            saved_at: None,
            spent: false,
        }
    }

    /// Fails open: an undecided or not-asking message is never corrected.
    #[tokio::test]
    async fn only_an_asked_save_with_none_done_is_checked() {
        assert!(watch(Some(SaveAsk::Undecided)).due().await.is_none(), "undecided counts as not asking");
        assert!(watch(Some(SaveAsk::NotAsked)).due().await.is_none());
        assert!(watch(None).due().await.is_none());

        let mut w = watch(Some(SaveAsk::Asked));
        w.saved(2);
        assert!(w.due().await.is_none(), "a save that succeeded answers the ask");

        let mut w = watch(Some(SaveAsk::Asked));
        assert!(w.due().await.is_some(), "asked and nothing saved");
        assert!(w.due().await.is_none(), "one correction a turn");
    }

    /// A message typed into the work asks from its own step: a save before
    /// it answered an earlier ask, not this one.
    #[tokio::test]
    async fn a_mid_turn_ask_needs_a_save_after_it() {
        let mut w = watch(Some(SaveAsk::NotAsked));
        w.saved(2);
        w.heard(4, SaveAsk::Asked);
        assert!(w.due().await.is_some());

        let mut w = watch(None);
        w.heard(4, SaveAsk::Asked);
        w.saved(5);
        assert!(w.due().await.is_none());

        let mut w = watch(None);
        w.heard(4, SaveAsk::Undecided);
        assert!(w.due().await.is_none(), "fails open");
    }

    #[tokio::test]
    async fn shadow_logs_and_never_continues() {
        let transcript: Vec<ai::Message> = Vec::new();
        let end = TurnEnd { transcript: &transcript, step: 1, checks_this_turn: 0 };
        let check = SaveCheck { shadow: true, agent_id: "a".into() };
        assert!(matches!(check.check(&end).await, EndVerdict::Stop));
        let check = SaveCheck { shadow: false, agent_id: "a".into() };
        assert!(matches!(check.check(&end).await, EndVerdict::Continue(TurnEvent::UnsavedMemory)));
    }

    #[test]
    fn a_watch_that_does_not_apply_asks_nothing() {
        let w = SaveWatch::off();
        assert!(!w.applies());
    }
}
