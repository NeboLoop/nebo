//! A command the owner gives is run, never answered from what it would do.
//! In the 2026-09-27 proofs (`suites/smoke.yaml`) the owner's "Run
//! 'nebo-nonexistent-tool --version'" was answered "running it would result
//! in command not found" with no call, and "Run `convert image.png
//! image.jpg`" opened with loading convert_file, then went looking for
//! converters and installing one. A line in the prompt saying so held in
//! most runs and not all of them (gate 36384054926: one of each again).
//!
//! Whether the owner's message gives a command to run is one typed decision
//! (Jev through Janus, [`ai::DecideClient`]), asked about the message that
//! starts the turn in the same call as the save question (`opening`): one
//! call per message, never two.
//!
//! The harness, not the model, acts on it:
//!
//! - **The first step calls run_command.** When the decision is in by the
//!   first step ([`FIRST_STEP_WAIT`]) and says the owner gave a command, the
//!   first step's tool choice is run_command, so no other tool and no answer
//!   in words comes before it.
//! - **The turn doesn't end without it.** When the turn would end (a reply
//!   with no tool calls), the owner gave a command and no run_command call
//!   was made this turn, the end check ([`CommandCheck`]) takes ONE more step
//!   with a note saying so. A call the permission check refused or parked
//!   counts as made: the refusal is the outcome the owner hears.
//!
//! **Fails open.** When no decision can be had — the switch off, no client,
//! an error, a timeout, an incomplete answer — the message counts as giving
//! no command and the turn runs as it would have. A wrong "gave a command"
//! makes the employee run a shell command the owner never wrote, so the bar
//! is high ([`COMMAND_AT`]); a missed one leaves it to the prompt, as before.
//!
//! No phrase lists: whether a message gives a command is the decision's,
//! never a keyword match.
//!
//! Every decision is logged at info with `site="owner_command"` and the
//! probability that made it; an undecided one at warn with the reason. A
//! forced first step and every correction are logged too.
//!
//! Switch: `NEBO_DECIDE_COMMAND` — `0` turns it off (the question is not
//! asked), `shadow` decides and logs `would_force` / `would_correct` without
//! acting; unset (or anything else) is on.

use ai::{Decision, Question};
use tracing::{info, warn};

use super::events::TurnEvent;
use super::turn_end::{EndCheck, EndVerdict, TurnEnd};
use crate::heartbeat_triage::{self, Mode};

/// UNTUNED. The chance that the message gives a command at or over which it
/// counts as giving one. High on purpose: a wrong yes runs a command the
/// owner never wrote. `shadow` logs every `p_named`, and that data sets it.
pub const COMMAND_AT: f64 = 0.7;

/// UNTUNED. The most the first step waits for the opening decision before
/// it goes ahead without it. The decision starts at Prepare and Jev answers
/// in about 200 ms, so the wait is usually what is left of that; past it the
/// first step is not held up, and the end check still reads the decision.
pub const FIRST_STEP_WAIT: std::time::Duration = std::time::Duration::from_millis(1_500);

/// The end check's name, and the key the question is asked under.
pub const CHECK: &str = "unrun_command";
pub const QUESTION: &str = "command";

/// The tool a command runs through.
pub const RUN_COMMAND: &str = "run_command";

/// What the owner's message says about a command to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAsk {
    Named,
    NotNamed,
    /// No decision could be had; handled as giving no command.
    Undecided,
}

/// What `NEBO_DECIDE_COMMAND` says.
pub fn mode() -> Mode {
    heartbeat_triage::switch("NEBO_DECIDE_COMMAND")
}

/// The question, about the state's `message`.
pub fn question() -> Question {
    Question::choice(
        "Whether `message` gives the employee a command to run: a shell command or a program with its arguments, \
         written out to be run as it is, often in quotes or backticks.",
        &[
            (
                "named",
                "It gives a command to run: run `ls -la`, run 'npm test', execute git status, what does `python3 \
                 --version` print.",
            ),
            (
                "none",
                "It gives no command to run: a question, or a task in words that the employee works out how to do \
                 (make a report, find a file, convert an image), or a command only mentioned or asked about, not \
                 asked to be run.",
            ),
        ],
    )
}

/// What a decision says: named when "named" reaches [`COMMAND_AT`]. Logged:
/// the answer at info, a missing one at warn.
pub fn read(decision: &Decision, agent_id: &str) -> CommandAsk {
    let Some(answer) = decision.answer(QUESTION).filter(|a| !a.probabilities.is_empty()) else {
        return undecided(agent_id, "incomplete");
    };
    let named = answer.probabilities.get("named").copied().unwrap_or(0.0);
    let ask = if named >= COMMAND_AT { CommandAsk::Named } else { CommandAsk::NotNamed };
    info!(
        site = "owner_command",
        agent = %agent_id,
        outcome = if ask == CommandAsk::Named { "named" } else { "not_named" },
        p_named = named,
        model = %decision.model,
        "owner command"
    );
    ask
}

/// A message no decision could be had for, logged with why.
pub fn undecided(agent_id: &str, reason: &str) -> CommandAsk {
    warn!(site = "owner_command", agent = %agent_id, outcome = "undecided", reason, "the owner's message counts as giving no command");
    CommandAsk::Undecided
}

/// What the turn knows about a command the owner gave.
#[derive(Debug)]
pub struct CommandWatch {
    mode: Mode,
    agent_id: String,
    /// The opening message gives a command to run.
    named: bool,
    /// A run_command call was made this turn.
    ran: bool,
    /// The check took its one step.
    spent: bool,
}

impl CommandWatch {
    /// A turn nothing is checked on.
    pub fn off() -> CommandWatch {
        CommandWatch { mode: Mode::Off, agent_id: String::new(), named: false, ran: false, spent: true }
    }

    /// Start watching a turn. `applies` is whether the owner speaks in it and
    /// run_command is the employee's to call.
    pub fn start(applies: bool, agent_id: &str) -> CommandWatch {
        let mode = mode();
        if !applies || mode == Mode::Off {
            return CommandWatch::off();
        }
        CommandWatch { mode, agent_id: agent_id.to_string(), named: false, ran: false, spent: false }
    }

    /// Whether the opening decision asks about a command.
    pub fn applies(&self) -> bool {
        self.mode != Mode::Off
    }

    /// The opening decision came in.
    pub fn opened(&mut self, ask: CommandAsk) {
        self.named = self.applies() && ask == CommandAsk::Named;
    }

    /// The first step's tool choice: run_command, when the owner gave a
    /// command and the step offers it.
    pub fn first_step_choice(&self, offered: &[ai::ToolDefinition]) -> Option<ai::ToolChoice> {
        if !self.named || !offered.iter().any(|t| t.name == RUN_COMMAND) {
            return None;
        }
        let outcome = if self.mode == Mode::Shadow { "would_force" } else { "forced" };
        info!(site = "owner_command", agent = %self.agent_id, outcome, "the first step calls run_command");
        (self.mode != Mode::Shadow).then(|| ai::ToolChoice::Tool(RUN_COMMAND.to_string()))
    }

    /// A run_command call was made.
    pub fn ran(&mut self) {
        self.ran = true;
    }

    /// The turn would end: the check to run, when the owner gave a command
    /// and no run_command call was made. Taken once per turn ([`Self::spend`]).
    pub fn due(&self) -> Option<CommandCheck> {
        (self.named && !self.ran && !self.spent)
            .then(|| CommandCheck { shadow: self.mode == Mode::Shadow, agent_id: self.agent_id.clone() })
    }

    /// The check took its step.
    pub fn spend(&mut self) {
        self.spent = true;
    }
}

/// The owner gave a command and none has run: one more step, told so.
pub struct CommandCheck {
    shadow: bool,
    agent_id: String,
}

#[async_trait::async_trait]
impl EndCheck for CommandCheck {
    fn name(&self) -> &'static str {
        CHECK
    }

    async fn check(&self, end: &TurnEnd<'_>) -> EndVerdict {
        let outcome = if self.shadow { "would_correct" } else { "corrected" };
        info!(site = CHECK, agent = %self.agent_id, outcome, step = end.step, "the owner gave a command and none has run");
        if self.shadow {
            return EndVerdict::Stop;
        }
        EndVerdict::Continue(TurnEvent::UnrunCommand)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn said(named: f64) -> Decision {
        let answers = HashMap::from([(
            QUESTION.to_string(),
            ai::Answer {
                kind: "choice".into(),
                choice: Some(if named >= 0.5 { "named" } else { "none" }.into()),
                score: None,
                noul: None,
                confidence: Some(0.9),
                probabilities: [("named".to_string(), named), ("none".to_string(), 1.0 - named)].into(),
            },
        )]);
        Decision { model: "jev-test".into(), answers, usage: Default::default() }
    }

    fn run_command() -> ai::ToolDefinition {
        ai::ToolDefinition { name: RUN_COMMAND.into(), description: String::new(), input_schema: serde_json::json!({}) }
    }

    /// A wrong yes runs a command nobody wrote: a near tie is no.
    #[test]
    fn a_command_counts_only_past_the_bar() {
        assert_eq!(read(&said(0.9), "a"), CommandAsk::Named);
        assert_eq!(read(&said(COMMAND_AT), "a"), CommandAsk::Named);
        assert_eq!(read(&said(0.6), "a"), CommandAsk::NotNamed);
        let empty = Decision { model: "jev-test".into(), answers: HashMap::new(), usage: Default::default() };
        assert_eq!(read(&empty, "a"), CommandAsk::Undecided, "no answer is undecided");
    }

    /// The first step is forced only when the owner gave a command and the
    /// step offers run_command; the check takes its one step only when no
    /// call was made, and not twice.
    #[test]
    fn a_named_command_forces_the_first_step_and_is_checked_once() {
        let mut watch = CommandWatch { mode: Mode::On, agent_id: "a".into(), named: false, ran: false, spent: false };
        assert_eq!(watch.first_step_choice(&[run_command()]), None, "nothing named yet");
        watch.opened(CommandAsk::Undecided);
        assert!(watch.due().is_none(), "undecided fails open");
        watch.opened(CommandAsk::Named);
        assert_eq!(watch.first_step_choice(&[]), None, "not offered, not forced");
        assert_eq!(watch.first_step_choice(&[run_command()]), Some(ai::ToolChoice::Tool(RUN_COMMAND.into())));
        assert!(watch.due().is_some(), "named and not run");
        watch.spend();
        assert!(watch.due().is_none(), "once per turn");

        let mut ran = CommandWatch { mode: Mode::On, agent_id: "a".into(), named: false, ran: false, spent: false };
        ran.opened(CommandAsk::Named);
        ran.ran();
        assert!(ran.due().is_none(), "a call was made");

        let mut shadow = CommandWatch { mode: Mode::Shadow, agent_id: "a".into(), named: false, ran: false, spent: false };
        shadow.opened(CommandAsk::Named);
        assert_eq!(shadow.first_step_choice(&[run_command()]), None, "shadow only logs");
        assert!(CommandWatch::off().due().is_none() && !CommandWatch::off().applies());
    }
}
