//! The proof suite: every kind of work "Work That Keeps Its Word" says the
//! engine opens up, and every behaviour of the seven mechanisms it replaced,
//! each as one deterministic scenario. No model runs here — a turn's words
//! are given, the engine's rows are asserted: run states, waits and their
//! deadlines, ledger rows, cards, history lines.
//!
//! Every scenario carries its "must never" twin where one exists: no second
//! message to a person, no lost message, no close without a receipt, no
//! action after an opt-out, no retry of an unknown outcome, no storm after
//! downtime, no second turn beside a live one.
//!
//! Numbering follows the document (uc01–uc56); `parity` covers the seven
//! replaced mechanisms. A scenario that cannot be proven by the engine
//! alone says so in its doc comment and proves the mechanics the claim
//! rests on.

#![allow(dead_code, unused_imports)]

use super::*;
pub use db::{Enqueued, EngineEffect, EngineEvent, EngineRun, EngineWait, NewEvent, NewRun, NewWait};
pub use tools::effects::{guarded_send, SendOutcome};
pub use tools::origin::ToolContext;
pub use workflow::cases::{open_case_for, settle_turn, signal_or_open, CaseBinding, Routed};

mod concurrency;
mod finance;
mod legal;
mod marketing;
mod operations;
mod parity;
mod people;
mod platform;
mod sales;
mod service;

pub const DAY: i64 = 86_400;
pub const HOUR: i64 = 3_600;

/// A fresh store and a clock. Time only moves when a scenario moves it.
pub struct World {
    pub s: Store,
    pub t: i64,
}

pub fn idle(_: &str) -> Option<String> {
    None
}
pub fn no_steer(_: &str, _: &EngineEvent) {}

/// A fresh store on its own file.
pub fn fresh_store() -> Store {
    let path = std::env::temp_dir().join(format!("nebo-proof-{}.db", uuid::Uuid::new_v4()));
    Store::new(&path.to_string_lossy()).expect("store")
}

impl World {
    pub fn new() -> Self {
        World { s: fresh_store(), t: 1_700_000_000 }
    }

    /// A case binding of one employee: `case_type` cases, `default_wait`
    /// seconds between touches when a turn names no deadline.
    pub fn binding(agent: &'static str, name: &'static str, case_type: &str, default_wait: i64) -> CaseBinding<'static> {
        CaseBinding {
            agent_id: agent,
            binding_name: name,
            case_type: case_type.into(),
            definition_json: r#"{"activities":[{"id":"run","intent":"work the case"}]}"#,
            base_inputs: serde_json::json!({}),
            default_wait_secs: default_wait,
        }
    }

    /// A message from `value` (an email, a phone, a crm id) reaches the
    /// binding: the person's open case takes it, or one opens.
    pub fn arrive(&self, b: &CaseBinding<'_>, kind: &str, value: &str, payload: serde_json::Value, idem: &str) -> Routed {
        signal_or_open(&self.s, b, kind, value, &payload, "event", idem, self.t).expect("routed")
    }

    /// A person's first message: the case it opened and the first turn,
    /// queued. Panics if the message did not open a case.
    pub fn open(&self, b: &CaseBinding<'_>, email: &str, text: &str, idem: &str) -> (String, EngineRun) {
        let routed = self.arrive(b, "email", email, serde_json::json!({"email": email, "message": text}), idem);
        let Routed::Opened { case_id } = routed else { panic!("expected a new case, got {routed:?}") };
        let turn = self.queued_turn(&case_id).expect("the first turn is queued");
        (case_id, turn)
    }

    /// The turn the engine queued for a case, if one is queued.
    pub fn queued_turn(&self, case_id: &str) -> Option<EngineRun> {
        self.s.engine_queued_runs_of_kind("workflow", 50).unwrap().into_iter().find(|t| t.parent_run_id.as_deref() == Some(case_id))
    }

    /// The runner picked the turn up.
    pub fn start(&self, turn_id: &str) -> EngineRun {
        self.s.engine_set_run_state(turn_id, "running", self.t, None).unwrap();
        self.run(turn_id)
    }

    /// The turn ended with these words (the contract is the last JSON
    /// object); the engine settles the case from them.
    pub fn finish(&self, turn: &EngineRun, words: &str) {
        settle_turn(&self.s, turn, Some(words), false, self.t).unwrap();
    }

    /// The turn failed (a crash, an exhausted budget) with this error.
    pub fn fail(&self, turn: &EngineRun, why: &str) {
        settle_turn(&self.s, turn, Some(why), true, self.t).unwrap();
    }

    /// Start the queued turn and finish it with these words, in one step.
    pub fn turn(&self, case_id: &str, words: &str) -> EngineRun {
        let queued = self.queued_turn(case_id).expect("a queued turn");
        let turn = self.start(&queued.id);
        self.finish(&turn, words);
        self.run(&turn.id)
    }

    pub fn tick(&self) -> TickReport {
        tick(&self.s, self.t, &idle, &no_steer)
    }

    /// Move the clock and tick once.
    pub fn advance(&mut self, secs: i64) -> TickReport {
        self.t += secs;
        self.tick()
    }

    pub fn run(&self, id: &str) -> EngineRun {
        self.s.engine_get_run(id).unwrap().expect("run exists")
    }

    /// The case's current wait, if it is waiting.
    pub fn wait(&self, case_id: &str) -> Option<EngineWait> {
        self.run(case_id).current_wait_id.and_then(|id| self.s.engine_get_wait(id).unwrap())
    }

    /// The case's durable history, oldest first.
    pub fn history(&self, case_id: &str) -> Vec<EngineEvent> {
        let mut h = self.s.engine_events_for("run", case_id, 200).unwrap();
        h.sort_by_key(|e| e.id);
        h
    }

    pub fn history_has(&self, case_id: &str, kind: &str, text: &str) -> bool {
        self.history(case_id).iter().any(|e| e.kind == kind && e.payload.contains(text))
    }

    /// The owner's card for a subject, if one was raised.
    pub fn card(&self, subject: &str) -> Option<String> {
        let user = self.s.ensure_local_user_id().unwrap();
        self.s.get_notification(&format!("attention:{subject}"), &user).unwrap().map(|n| n.body.unwrap_or_default())
    }

    pub fn receipts(&self, run_id: &str) -> Vec<EngineEffect> {
        self.s.engine_effects_for_run(run_id).unwrap()
    }

    /// A tool context for a turn: what the runner hands a tool inside it.
    pub fn ctx(agent: &str, run_id: &str) -> ToolContext {
        ToolContext { session_key: format!("agent:{agent}:workflow:{run_id}:run::0"), ..Default::default() }
    }

    /// A customer-facing send from inside a turn, through the ledger, with
    /// the provider answering as given.
    pub async fn send(&self, agent: &str, run_id: &str, to: &str, text: &str, outcome: SendOutcome) -> tools::ToolResult {
        let ctx = Self::ctx(agent, run_id);
        let input = serde_json::json!({"to": to, "body": text});
        guarded_send(&self.s, &ctx, "messaging", "mail-app", "mail.message.send", &input, || async { outcome }).await
    }

    /// The messages and answers still owed to someone: every non-timer
    /// event nobody has delivered. Read-only.
    pub fn undelivered(&self) -> Vec<EngineEvent> {
        self.s.engine_undelivered_signals().unwrap()
    }
}

/// A turn's contract: wait on `on` until `deadline` (relative, "3d") for `reason`.
pub fn waits(status: &str, summary: &str, on: &str, deadline: &str, reason: &str) -> String {
    format!(
        r#"{{"result":{{"status":"{status}","summary":"{summary}"}},"next":{{"action":"wait","on":"{on}","deadline":"{deadline}","reason":"{reason}"}}}}"#
    )
}

/// A turn's contract: close the case as `status`.
pub fn closes(status: &str, summary: &str) -> String {
    format!(r#"{{"result":{{"status":"{status}","summary":"{summary}"}},"next":{{"action":"close"}}}}"#)
}

/// Words with no contract at the end: an invalid turn.
pub fn prose(text: &str) -> String {
    text.to_string()
}
