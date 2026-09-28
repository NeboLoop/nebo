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
//! - the message that starts the turn: decided in the turn's one opening
//!   call (`opening`), with any other question about that message, from
//!   Prepare while the steps run, and read when it is needed, so it costs
//!   the turn no wait;
//! - a message typed into the running work: asked in the same call that
//!   decides its intent (`owner_intent::decide`), never a second call.
//!
//! The decision also names the memory the owner asked for: private (for the
//! employee itself, or unsaid) or local (for everyone on this Nebo). Writing
//! or saving a file or a document is not memory: in the 2026-09-27 re-run
//! (gate 36330201037) the first wording read "…write all the facts, one per
//! line, to facts.md" as a save in 3 of 3 runs (p 0.82–0.86).
//!
//! The harness, not the model, acts on it: when the turn would end (a reply
//! with no tool calls) and the owner asked, but no `remember` call has
//! succeeded since, the end check ([`SaveCheck`]) takes ONE more step. That
//! step is the harness's own ([`Correction`]): its note tells the model
//! nothing is saved yet and names the scope; nothing it writes reaches the
//! owner or the conversation, only its `remember` calls run, and each runs
//! in the scope the owner asked for, whatever the call said. The owner's
//! reply stays the one before it, and the harness adds one line of its own:
//! where the save went, read from the tool's result ([`saved_line`]), or
//! that nothing was saved ([`NOT_SAVED`]). Never the model's words: the
//! re-run's corrections narrated the note to the owner ("I'll reply with
//! nothing as instructed"), saved a "for yourself" fact to local memory, and
//! repeated a false claim. At most once per turn.
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
//! The same decision is the one sign that the owner asked to share: a
//! Confidential conversation's `remember` reaches local memory only when the
//! owner's latest ask this turn is for local memory ([`SaveWatch::shares`],
//! handed to the tool as `ToolContext::owner_shares`). The model choosing
//! `scope: "local"` on its own never takes a fact out of the conversation:
//! in the v0.16.0 proof (m08) it put one client's deposition date in local
//! memory in 2 of 3 runs. Undecided is no here too, so without a decision a
//! Confidential save stays in its conversation.
//!
//! No phrase lists: whether a message asks for a save is the decision's,
//! never a keyword match.
//!
//! Every decision is logged at info with `site="save_ask"` and the
//! probability that made it; an undecided one at warn with the reason. Every
//! correction is logged at info with `site="unsaved_memory"`.
//!
//! Switch: `NEBO_DECIDE_SAVE` — `0` turns it off (no decision is asked, so
//! a Confidential employee's saves all stay in their conversations),
//! `shadow` decides and logs `would_correct` without ever correcting; unset
//! (or anything else) is on.

use std::time::Duration;

use ai::{Decision, Question};
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

/// The memory a save goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The employee's own memory: what "for yourself" means, and what a
    /// request that names no one means.
    Private,
    /// Local memory, read by every employee on this Nebo.
    Local,
}

impl Scope {
    /// The `remember` call's `scope` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Private => "private",
            Scope::Local => "local",
        }
    }
}

/// What the owner's message says about keeping something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveAsk {
    Asked(Scope),
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
    Question::choice(
        "Whether `message` asks the employee to keep something in its memory, and for whom. Memory is what \
         the employee remembers from one conversation to the next. Writing, saving or exporting a file or a \
         document is work on that file, not memory, and changing an employee's instructions or settings is \
         work on that employee: words the owner wants written into a file, a document, a message or an \
         employee's instructions are that writing, even when they mention memory.",
        &[
            (
                "private",
                "Keep it in the employee's own memory: remember it, keep it in mind, note it or save it for later, \
                 remember it for yourself, or remember it without saying for whom.",
            ),
            (
                "local",
                "Keep it in memory shared with every employee: for everyone, for the team, for the company, in \
                 shared or company memory.",
            ),
            (
                "none",
                "Nothing to keep in memory: a question, a task, writing, saving or exporting a file or a \
                 document (a .md or .txt file, a spreadsheet, a PDF, a note in an app), or changing an \
                 employee's instructions or settings.",
            ),
        ],
    )
}

/// What a decision says: asked when the two memory options together reach
/// [`SAVE_AT`], in the likelier of the two. Logged: the answer at info, a
/// missing one at warn.
pub fn read(decision: &Decision, agent_id: &str) -> SaveAsk {
    let Some(answer) = decision.answer(QUESTION).filter(|a| !a.probabilities.is_empty()) else {
        return undecided(agent_id, "incomplete");
    };
    let p = |option: &str| answer.probabilities.get(option).copied().unwrap_or(0.0);
    let (private, local) = (p("private"), p("local"));
    let ask = if private + local < SAVE_AT {
        SaveAsk::NotAsked
    } else if local > private {
        SaveAsk::Asked(Scope::Local)
    } else {
        SaveAsk::Asked(Scope::Private)
    };
    info!(
        site = "save_ask",
        agent = %agent_id,
        outcome = match ask {
            SaveAsk::Asked(Scope::Private) => "asked_private",
            SaveAsk::Asked(Scope::Local) => "asked_local",
            _ => "not_asked",
        },
        p_private = private,
        p_local = local,
        p_none = p("none"),
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

/// What the turn knows about the saves the owner asked for.
pub struct SaveWatch {
    mode: Mode,
    agent_id: String,
    /// The step the latest asking message was heard at, and the memory it
    /// asked for: a save must have succeeded at that step or after.
    asked_at: Option<(u32, Scope)>,
    /// The latest step whose tool round saved a memory, with the tool's
    /// words for where it went.
    saved_at: Option<(u32, String)>,
    /// The check ran its one correction (or, in shadow, logged it).
    spent: bool,
}

impl SaveWatch {
    /// A turn nothing is checked on.
    pub fn off() -> SaveWatch {
        SaveWatch { mode: Mode::Off, agent_id: String::new(), asked_at: None, saved_at: None, spent: true }
    }

    /// Start watching a turn. `applies` is whether the owner speaks in it
    /// and the employee can save (`remember` in reach, memory writes on).
    /// The opening message's decision comes from the turn's opening call
    /// ([`Self::opened`]).
    pub fn start(applies: bool, agent_id: &str) -> SaveWatch {
        let mode = mode();
        if !applies || mode == Mode::Off {
            return SaveWatch::off();
        }
        SaveWatch { mode, agent_id: agent_id.to_string(), asked_at: None, saved_at: None, spent: false }
    }

    /// Whether the opening message, and a message typed into the work, are
    /// asked about.
    pub fn applies(&self) -> bool {
        self.mode != Mode::Off
    }

    /// A message typed into the work was decided at `step`.
    pub fn heard(&mut self, step: u32, ask: SaveAsk) {
        if let SaveAsk::Asked(scope) = ask {
            self.asked_at = Some((step, scope));
        }
    }

    /// The tool round at `step` saved a memory; `result` is the tool's
    /// answer.
    pub fn saved(&mut self, step: u32, result: &str) {
        self.saved_at = Some((step, result.to_string()));
    }

    /// The tool's answer for a save made at `step` or after.
    pub fn saved_since(&self, step: u32) -> Option<&str> {
        self.saved_at.as_ref().filter(|(at, _)| *at >= step).map(|(_, result)| result.as_str())
    }

    /// The opening message's decision came in.
    pub fn opened(&mut self, ask: SaveAsk) {
        if let (true, SaveAsk::Asked(scope)) = (self.applies(), ask) {
            // The turn's first step heard it; a later ask keeps its own step.
            self.asked_at.get_or_insert((1, scope));
        }
    }

    /// Whether the owner's latest ask this turn is for local memory: what
    /// lets a Confidential conversation's save reach every employee
    /// (`ToolContext::owner_shares`). No decision, or an ask for the
    /// employee's own memory, is no.
    pub fn shares(&self) -> bool {
        matches!(self.asked_at, Some((_, Scope::Local)))
    }

    /// The turn would end: the check to run, when the owner asked for a save
    /// that no `remember` call answers yet. Once per turn.
    pub fn due(&mut self) -> Option<SaveCheck> {
        if self.spent {
            return None;
        }
        let (asked_at, scope) = self.asked_at?;
        if self.saved_since(asked_at).is_some() {
            return None;
        }
        self.spent = true;
        Some(SaveCheck { shadow: self.mode == Mode::Shadow, agent_id: self.agent_id.clone(), scope })
    }
}

/// The owner asked for a save and none has succeeded: one more step, told so.
pub struct SaveCheck {
    shadow: bool,
    agent_id: String,
    scope: Scope,
}

#[async_trait::async_trait]
impl EndCheck for SaveCheck {
    fn name(&self) -> &'static str {
        CHECK
    }

    async fn check(&self, end: &TurnEnd<'_>) -> EndVerdict {
        let outcome = if self.shadow { "would_correct" } else { "corrected" };
        info!(site = CHECK, agent = %self.agent_id, outcome, scope = self.scope.as_str(), step = end.step, "the owner asked for a save and none has succeeded");
        if self.shadow {
            return EndVerdict::Stop;
        }
        EndVerdict::Continue(TurnEvent::UnsavedMemory(self.scope))
    }
}

/// The step the check adds: the harness's own. Its text is never shown or
/// stored; only its `remember` calls run, each in `scope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Correction {
    pub scope: Scope,
    /// The step whose save answers it: the correction's own.
    pub step: u32,
}

impl Correction {
    /// The calls of the correction's reply that run: its `remember` calls
    /// (by rule key), each with the owner's scope. Every other call is
    /// dropped, neither run nor stored.
    pub async fn calls(self, tools: &tools::Registry, calls: Vec<ai::ToolCall>) -> Vec<ai::ToolCall> {
        let mut kept = Vec::new();
        for mut call in calls {
            if !tools.target(&call.name, &call.input).await.is_some_and(|t| t.key == "remember") {
                continue;
            }
            if let Some(input) = call.input.as_object_mut() {
                input.insert("scope".into(), self.scope.as_str().into());
                kept.push(call);
            }
        }
        kept
    }
}

/// The owner's line when the correction saved nothing.
pub const NOT_SAVED: &str = "That wasn't saved to memory.";

/// The owner's line for a save, from the memory tool's own answer (never
/// the model's words): where the store says it went.
pub fn saved_line(result: &str) -> &'static str {
    use tools::memory_tools::MemoryScopeKind;
    // Where it went is said before the fact itself, whose words could name
    // any memory.
    let head = result.lines().next().unwrap_or("");
    let head = head.split_once(": [").map_or(head, |(head, _)| head);
    let went_to = |kind: MemoryScopeKind| head.contains(kind.label());
    if went_to(MemoryScopeKind::Local) && !went_to(MemoryScopeKind::Private) {
        "Saved to local memory, where every employee on this Nebo can find it."
    } else if went_to(MemoryScopeKind::Private) {
        "Saved to my private memory, where only I can see it."
    } else if went_to(MemoryScopeKind::Sealed) {
        "Saved to this conversation's sealed memory."
    } else if went_to(MemoryScopeKind::Confidential) {
        "Saved to this conversation's confidential memory, which no other conversation can see."
    } else {
        "Saved to memory."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A choice answer with these probabilities, the wire shape Janus returns.
    fn said(probabilities: &[(&str, f64)]) -> Decision {
        let picked = probabilities.iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|(k, _)| k.to_string());
        let answers = HashMap::from([(
            QUESTION.to_string(),
            ai::Answer {
                kind: "choice".into(),
                choice: picked,
                score: None,
                noul: None,
                confidence: Some(0.9),
                probabilities: probabilities.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            },
        )]);
        Decision { model: "jev-test".into(), answers, usage: Default::default() }
    }

    #[test]
    fn the_memory_options_decide_the_ask_and_its_scope() {
        assert_eq!(read(&said(&[("private", 0.9), ("local", 0.05), ("none", 0.05)]), "a"), SaveAsk::Asked(Scope::Private));
        assert_eq!(read(&said(&[("private", 0.1), ("local", 0.85), ("none", 0.05)]), "a"), SaveAsk::Asked(Scope::Local));
        assert_eq!(read(&said(&[("private", 0.3), ("local", 0.25), ("none", 0.45)]), "a"), SaveAsk::Asked(Scope::Private), "memory together outweighs none");
        assert_eq!(read(&said(&[("private", 0.1), ("local", 0.05), ("none", 0.85)]), "a"), SaveAsk::NotAsked);
        let none = Decision { model: "jev-test".into(), answers: HashMap::new(), usage: Default::default() };
        assert_eq!(read(&none, "a"), SaveAsk::Undecided);
    }

    /// The re-run's misfire (gate 36330201037, mid-turn-owner-message): the
    /// question Jev reads says a file write is not memory, and "for
    /// yourself" and an unsaid scope are private. The message itself is
    /// state, never part of the question.
    #[test]
    fn a_file_write_is_not_memory_in_the_question() {
        const TRACE: &str = "Read /tmp/nebo-eval/634283de/part1.txt and follow the instruction inside it, one file at a \
                             time, until there are no more files. Then write all the facts, one per line, to \
                             /tmp/nebo-eval/634283de/facts.md.";
        let Question::Choice { instructions, criteria } = question() else { panic!("a choice") };
        assert!(instructions.contains("`message`") && instructions.contains("file or a document is work on that file, not memory"), "{instructions}");
        assert!(criteria["none"].contains("writing, saving or exporting a file"), "{:?}", criteria["none"]);
        assert!(criteria["private"].contains("for yourself") && criteria["private"].contains("without saying for whom"));
        assert!(criteria["local"].contains("for everyone"));
        let asked = serde_json::to_string(&(instructions, criteria)).unwrap();
        assert!(!asked.contains("facts.md") && !asked.contains(TRACE), "the owner's words are state");
        // Jev reading the trace as no memory: nothing is asked, nothing corrected.
        assert_eq!(read(&said(&[("private", 0.06), ("local", 0.04), ("none", 0.9)]), "a"), SaveAsk::NotAsked);
    }

    /// The v0.16.0 proof's misfire (correction-agent-update-instructions, 3
    /// of 3 runs, p_private 0.42–0.52): new instructions for an employee
    /// that mention local memory were read as a save, and the correction's
    /// "That wasn't saved to memory." ended a reply that never spoke of
    /// memory. The question Jev reads says changing an employee's
    /// instructions is not memory, even when they mention it; the message is
    /// state.
    #[test]
    fn an_instruction_edit_is_not_memory_in_the_question() {
        const TRACE: &str = "Update the front-desk-c3ebffbf employee's instructions to exactly this: You answer inbound \
                             calls for NeboAI. Use only what is in local memory; never search the web.";
        let Question::Choice { instructions, criteria } = question() else { panic!("a choice") };
        assert!(instructions.contains("changing an employee's instructions or settings is work on that employee"), "{instructions}");
        assert!(instructions.contains("even when they mention memory"), "{instructions}");
        assert!(criteria["none"].contains("changing an employee's instructions or settings"), "{:?}", criteria["none"]);
        let asked = serde_json::to_string(&(instructions, criteria)).unwrap();
        assert!(!asked.contains("front-desk") && !asked.contains(TRACE), "the owner's words are state");
    }

    /// The owner's ask to share is the latest ask's scope, once the opening
    /// decision is in: local is yes, the employee's own memory or no ask is
    /// no, and a later ask for the employee's own memory takes it back.
    #[tokio::test]
    async fn sharing_is_the_latest_ask_for_local_memory() {
        assert!(watch(Some(LOCAL)).shares());
        assert!(!watch(Some(PRIVATE)).shares());
        assert!(!watch(Some(SaveAsk::NotAsked)).shares());
        assert!(!watch(Some(SaveAsk::Undecided)).shares());
        assert!(!SaveWatch::off().shares());

        let mut w = watch(Some(SaveAsk::NotAsked));
        w.heard(3, LOCAL);
        assert!(w.shares(), "a message typed into the work asked");
        w.heard(5, PRIVATE);
        assert!(!w.shares(), "and a later one kept it to the employee");

        let mut w = watch(Some(LOCAL));
        assert!(w.shares());
        assert_eq!(w.due().map(|c| c.scope), Some(Scope::Local), "the end check still reads the same ask");
    }

    fn watch(opening: Option<SaveAsk>) -> SaveWatch {
        let mut w = SaveWatch { mode: Mode::On, agent_id: "a".into(), asked_at: None, saved_at: None, spent: false };
        if let Some(ask) = opening {
            w.opened(ask);
        }
        w
    }

    const LOCAL: SaveAsk = SaveAsk::Asked(Scope::Local);
    const PRIVATE: SaveAsk = SaveAsk::Asked(Scope::Private);

    /// Fails open: an undecided or not-asking message is never corrected.
    #[tokio::test]
    async fn only_an_asked_save_with_none_done_is_checked() {
        assert!(watch(Some(SaveAsk::Undecided)).due().is_none(), "undecided counts as not asking");
        assert!(watch(Some(SaveAsk::NotAsked)).due().is_none());
        assert!(watch(None).due().is_none());

        let mut w = watch(Some(LOCAL));
        w.saved(2, "Saved to local memory (every employee on this Nebo can find it): [project] k = v");
        assert!(w.due().is_none(), "a save that succeeded answers the ask");

        let mut w = watch(Some(PRIVATE));
        let check = w.due().expect("asked and nothing saved");
        assert_eq!(check.scope, Scope::Private, "the correction carries the scope asked for");
        assert!(w.due().is_none(), "one correction a turn");
    }

    /// A message typed into the work asks from its own step, in its own
    /// scope: a save before it answered an earlier ask, not this one.
    #[tokio::test]
    async fn a_mid_turn_ask_needs_a_save_after_it() {
        let mut w = watch(Some(SaveAsk::NotAsked));
        w.saved(2, "Saved to local memory");
        w.heard(4, LOCAL);
        assert_eq!(w.due().map(|c| c.scope), Some(Scope::Local));

        let mut w = watch(Some(PRIVATE));
        w.heard(4, LOCAL);
        w.saved(5, "Saved to local memory");
        assert!(w.due().is_none());

        let mut w = watch(None);
        w.heard(4, SaveAsk::Undecided);
        assert!(w.due().is_none(), "fails open");
    }

    #[tokio::test]
    async fn shadow_logs_and_never_continues() {
        let transcript: Vec<ai::Message> = Vec::new();
        let end = TurnEnd { transcript: &transcript, step: 1, checks_this_turn: 0 };
        let check = SaveCheck { shadow: true, agent_id: "a".into(), scope: Scope::Private };
        assert!(matches!(check.check(&end).await, EndVerdict::Stop));
        let check = SaveCheck { shadow: false, agent_id: "a".into(), scope: Scope::Local };
        assert!(matches!(check.check(&end).await, EndVerdict::Continue(TurnEvent::UnsavedMemory(Scope::Local))));
    }

    /// The owner's line is the store's word for where the save went, never
    /// the fact's words.
    #[test]
    fn the_owners_line_comes_from_the_tools_answer() {
        assert_eq!(
            saved_line("Saved to local memory (every employee on this Nebo can find it): [project] recipes/bites = 380F"),
            "Saved to local memory, where every employee on this Nebo can find it."
        );
        assert_eq!(
            saved_line("Saved to your private memory (only you can see it): [tacit/general] owner/replies = keep it out of local memory"),
            "Saved to my private memory, where only I can see it.",
            "the fact's own words never decide"
        );
        assert_eq!(
            saved_line("Saved to this conversation's confidential memory (no other conversation can see it): [project] case/terms = 410,000"),
            "Saved to this conversation's confidential memory, which no other conversation can see."
        );
        assert_eq!(saved_line("Saved."), "Saved to memory.");
    }

    #[test]
    fn a_watch_that_does_not_apply_asks_nothing() {
        let w = SaveWatch::off();
        assert!(!w.applies());
    }
}
