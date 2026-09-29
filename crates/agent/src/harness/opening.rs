//! The one decision about the owner's message that starts a turn: whether
//! it asks for something to be kept in memory (`memory_save`) and whether it
//! gives a command to run (`owner_command`). Each question is asked only when
//! its check applies to the turn, and the ones that apply go in ONE call
//! (Jev through Janus, [`ai::DecideClient`]): one call per message, never
//! two. A message typed into the running work is decided with its intent
//! (`owner_intent`), not here.
//!
//! The call starts at Prepare and runs while the steps do. The first step
//! waits for it only when the command question is asked (its tool choice
//! reads the answer, `owner_command::FIRST_STEP_WAIT`); the save check reads
//! it when it needs it: a local memory write, or the turn's end.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ai::DecideClient;
use tokio::task::JoinHandle;
use tracing::warn;

use super::memory_save::{self, SaveAsk};
use super::owner_command::{self, CommandAsk};

/// The most of the owner's words the decision reads.
const WORDS_CAP: usize = 2_000;

/// Which questions the turn asks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Asks {
    pub save: bool,
    pub command: bool,
}

/// What the decision says about the opening message. A question that
/// wasn't asked reads as not asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Heard {
    pub save: SaveAsk,
    pub command: CommandAsk,
}

impl Heard {
    const NOTHING: Heard = Heard { save: SaveAsk::NotAsked, command: CommandAsk::NotNamed };
}

/// The opening decision, running since Prepare; read once.
pub struct Opening {
    task: Option<JoinHandle<Heard>>,
}

impl Opening {
    /// A turn with nothing to decide.
    pub fn off() -> Opening {
        Opening { task: None }
    }

    /// Start deciding `message` (the owner's words that start the turn) on
    /// the questions `asks` names. Nothing is asked when none applies or
    /// there are no words.
    pub fn start(client: Option<Arc<DecideClient>>, message: Option<&str>, asks: Asks, agent_id: &str) -> Opening {
        let Some(message) = message.map(str::trim).filter(|m| !m.is_empty()) else {
            return Opening::off();
        };
        if asks == Asks::default() {
            return Opening::off();
        }
        let message = message.to_string();
        let agent = agent_id.to_string();
        let task = tokio::spawn(async move {
            decide(client.as_deref(), &message, asks, &agent, memory_save::DECIDE_TIMEOUT).await
        });
        Opening { task: Some(task) }
    }

    /// The decision, when it is in within `wait`. `None` when nothing was
    /// asked, it was read already, or it isn't in yet: then a later call can
    /// still read it.
    pub async fn heard(&mut self, wait: Duration) -> Option<Heard> {
        let task = self.task.as_mut()?;
        let heard = match tokio::time::timeout(wait, task).await {
            Err(_) => return None,
            Ok(Ok(heard)) => heard,
            Ok(Err(e)) => {
                warn!(site = "opening", error = %e, "the opening decision did not finish");
                Heard::NOTHING
            }
        };
        self.task = None;
        Some(heard)
    }
}

impl Drop for Opening {
    /// A turn that ends before its decision came back stops it.
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Ask the questions `asks` names about `message`, in one call. Anything but
/// an answer is undecided for each question asked.
pub async fn decide(client: Option<&DecideClient>, message: &str, asks: Asks, agent_id: &str, timeout: Duration) -> Heard {
    let undecided = |reason: &str| Heard {
        save: if asks.save { memory_save::undecided(agent_id, reason) } else { SaveAsk::NotAsked },
        command: if asks.command { owner_command::undecided(agent_id, reason) } else { CommandAsk::NotNamed },
    };
    let Some(client) = client else {
        return undecided("no_client");
    };
    let mut questions = BTreeMap::new();
    if asks.save {
        questions.insert(memory_save::QUESTION, memory_save::question());
    }
    if asks.command {
        questions.insert(owner_command::QUESTION, owner_command::question());
    }
    let state = serde_json::json!({ "message": ai::decide::clip(message.trim(), WORDS_CAP) });
    let trace = ai::RequestTrace { agent_id: agent_id.to_string(), ..ai::RequestTrace::new("opening_ask") };
    match tokio::time::timeout(timeout, client.decide(&trace, &state, &questions)).await {
        Ok(Ok(decision)) => Heard {
            save: if asks.save { memory_save::read(&decision, agent_id) } else { SaveAsk::NotAsked },
            command: if asks.command { owner_command::read(&decision, agent_id) } else { CommandAsk::NotNamed },
        },
        Ok(Err(e)) => {
            warn!(site = "opening", error = %e, "the decision failed");
            undecided("error")
        }
        Err(_) => undecided("timeout"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails open: no client, or a Jev that can't be reached, is undecided
    /// on each question asked, and a question not asked reads as not asking.
    #[tokio::test]
    async fn no_decision_is_undecided_on_what_was_asked() {
        let t = Duration::from_millis(50);
        let both = Asks { save: true, command: true };
        let undecided = Heard { save: SaveAsk::Undecided, command: CommandAsk::Undecided };
        assert_eq!(decide(None, "run ls and save the list", both, "a", t).await, undecided);
        let dead = DecideClient::new("http://127.0.0.1:9", || Some(ai::Bearer { token: "t".into(), bot_id: None }));
        assert_eq!(decide(Some(&dead), "run ls", both, "a", t).await, undecided, "an unreachable Jev");
        let save_only = Asks { save: true, command: false };
        let heard = decide(None, "run ls", save_only, "a", t).await;
        assert_eq!(heard, Heard { save: SaveAsk::Undecided, command: CommandAsk::NotNamed });
    }

    /// Nothing to ask, or no words: no call, and nothing to read.
    #[tokio::test]
    async fn nothing_asked_starts_nothing() {
        let wait = Duration::from_millis(10);
        assert!(Opening::start(None, Some("run ls"), Asks::default(), "a").heard(wait).await.is_none());
        assert!(Opening::start(None, Some("  "), Asks { save: true, command: true }, "a").heard(wait).await.is_none());
        let mut opening = Opening::start(None, Some("run ls"), Asks { save: false, command: true }, "a");
        assert_eq!(opening.heard(Duration::from_secs(1)).await.map(|h| h.command), Some(CommandAsk::Undecided));
        assert!(opening.heard(wait).await.is_none(), "read once");
    }
}
