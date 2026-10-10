//! The ask: a call parked on the owner (Technical Design §2.12.5).
//!
//! Only that step waits. The ask is written and the model hears at once
//! that the call is waiting, so it carries on with everything else. One
//! card goes out everywhere (the Inbox, the phone, the open chat), and the
//! first answer anywhere wins.
//!
//! Every ask is a durable wait in the engine (owner, 09-25): an engine run
//! of kind `ask` waits on `answer` for `ask:<id>`, and the owner's answer is
//! the signal that wakes it. The engine then hands the run to
//! [`Asks::resume`], which applies the answer once: it re-runs the stored
//! call with the stored seat through the one check and tells the employee,
//! or releases the workflow step parked on the ask. An unanswered ask never
//! expires and never counts as a No; the wait's timer brings it back to the
//! owner as a reminder instead, at widening intervals, for as long as it is
//! open. Nothing here ever approves on its own.

use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use tools::ResolvedCall;
use types::permissions::{
    AskCase, Door, Effect, Grant, MoneyLimit, OwnerReach, Rule, RuleField, RuleKey, RuleSource, Scope, Target, Writer,
};

use super::{CheckCx, RuleSet};
use crate::harness::delegation::notify::{AskOutcome, render_ask_outcome};

/// How long an open ask waits before it comes back to the owner as a
/// reminder, by how many reminders it has had: a day, then two, then four,
/// then every week for as long as it is open.
pub fn reminder_after(reminded: usize) -> i64 {
    const DAY: i64 = 24 * 3600;
    match reminded {
        0 => DAY,
        1 => 2 * DAY,
        2 => 4 * DAY,
        _ => 7 * DAY,
    }
}

/// The event kind the owner's answer is signalled as, on the ask's wait key
/// ([`db::ask_wait_key`]).
pub const ANSWER_SIGNAL: &str = "answer";

/// The owner's answer. A permission ask takes Allow always, This once or
/// No; a held send's ask ([`AskKind::SendCheck`]) takes whether it went out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    AllowAlways,
    ThisOnce,
    No,
    /// The held send went out.
    Sent,
    /// The held send did not go out.
    NotSent,
}

/// What an ask asks: the owner's OK for a call, or whether a send whose
/// outcome never came back went out. The card shows each with its own
/// question and answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskKind {
    Permission,
    SendCheck,
}

impl AskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AskKind::Permission => "permission",
            AskKind::SendCheck => "send_check",
        }
    }

    /// Whether `answer` is one this kind of ask takes.
    pub fn takes(self, answer: Answer) -> bool {
        match self {
            AskKind::Permission => matches!(answer, Answer::AllowAlways | Answer::ThisOnce | Answer::No),
            AskKind::SendCheck => matches!(answer, Answer::Sent | Answer::NotSent),
        }
    }
}

impl Answer {
    pub fn as_str(self) -> &'static str {
        match self {
            Answer::AllowAlways => "allow_always",
            Answer::ThisOnce => "this_once",
            Answer::No => "no",
            Answer::Sent => "sent",
            Answer::NotSent => "not_sent",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow_always" => Some(Answer::AllowAlways),
            "this_once" => Some(Answer::ThisOnce),
            "no" => Some(Answer::No),
            "sent" => Some(Answer::Sent),
            "not_sent" => Some(Answer::NotSent),
            _ => None,
        }
    }
}

/// Where the owner answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnsweredVia {
    Chat,
    Inbox,
    Mobile,
    /// A button on the phone's notification: the lock screen, or the Watch
    /// it mirrors to.
    Notification,
    /// Out loud, on the owner's own call.
    Voice,
}

impl AnsweredVia {
    pub fn as_str(self) -> &'static str {
        match self {
            AnsweredVia::Chat => "chat",
            AnsweredVia::Inbox => "inbox",
            AnsweredVia::Mobile => "mobile",
            AnsweredVia::Notification => "notification",
            AnsweredVia::Voice => "voice",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "chat" => Some(AnsweredVia::Chat),
            "inbox" => Some(AnsweredVia::Inbox),
            "mobile" => Some(AnsweredVia::Mobile),
            "notification" => Some(AnsweredVia::Notification),
            "voice" => Some(AnsweredVia::Voice),
            _ => None,
        }
    }
}

/// Where an ask stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AskStatus {
    Open,
    Answered { answer: Answer, via: Option<AnsweredVia> },
    /// The work that waited on it ended without it (a parked workflow run
    /// that is gone): nothing waits for an answer, so the card is cleared.
    /// Never a No, and nothing ran.
    Withdrawn,
}

/// The seat a parked call ran under, kept so the answer runs it exactly as
/// it would have run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeatSnapshot {
    pub grant: Grant,
    pub origin: tools::Origin,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub untrusted_input: bool,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub handoff_depth: u8,
}

/// The call as it was asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredCall {
    pub name: String,
    pub input: serde_json::Value,
}

/// One ask, as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub id: String,
    pub agent_id: String,
    pub session_key: String,
    pub door: Door,
    pub case: AskCase,
    /// The tool's activity line for the call ("sending a text to …").
    pub sentence: String,
    pub target: Target,
    pub call: StoredCall,
    pub seat: SeatSnapshot,
    pub status: AskStatus,
    /// The workflow run parked on this ask.
    pub run_id: Option<String>,
    /// The owner's conversation whose own flow raised the ask, the one chat
    /// its card shows in. None for everything else (a schedule, a workflow,
    /// a heartbeat, another employee's run, a helper): those reach the
    /// owner through the Inbox and a push, never a chat.
    pub chat_id: Option<String>,
    pub created_at: i64,
}

impl Ask {
    fn from_row(row: db::PermissionAskRow) -> Option<Ask> {
        let status = match row.status.as_str() {
            "open" => AskStatus::Open,
            "withdrawn" => AskStatus::Withdrawn,
            _ => AskStatus::Answered {
                answer: Answer::parse(row.answer.as_deref()?)?,
                via: row.answered_via.as_deref().and_then(AnsweredVia::parse),
            },
        };
        Some(Ask {
            door: serde_json::from_str(&row.door).ok()?,
            case: serde_json::from_str(&row.ask_case).ok()?,
            target: serde_json::from_str(&row.target).ok()?,
            call: serde_json::from_str(&row.call).ok()?,
            seat: serde_json::from_str(&row.seat).ok()?,
            id: row.id,
            agent_id: row.agent_id,
            session_key: row.session_key,
            sentence: row.sentence,
            status,
            run_id: row.run_id,
            chat_id: row.chat_id.filter(|c| !c.is_empty()),
            created_at: row.created_at,
        })
    }

    /// Whether "Allow always" can be offered: a locked must-ask can't be
    /// loosened by an answer, a deny is never loosened by one, giving an
    /// employee more room is answered each time, the company's day figures
    /// change only in the company layer, and a command that can't
    /// be read has no rule to save (no rule could be sure to cover it).
    pub fn allow_always_offered(&self, store: &db::Store) -> bool {
        let loosenable = match &self.case {
            AskCase::AskRule { rule_id } => store
                .get_permission_rule(rule_id)
                .ok()
                .flatten()
                .is_some_and(|r| !r.locked && r.effect == Effect::Ask),
            AskCase::Widens
            | AskCase::RemovesEmployee
            | AskCase::CompanyMoney { .. }
            | AskCase::UnconfirmedSend { .. } => false,
            _ => true,
        };
        loosenable && allow_always_rules(store, self).is_some()
    }

    /// Whether "This once" can be offered: an employee's extra needs are
    /// part of its job, so they are granted for good or not at all.
    pub fn this_once_offered(&self) -> bool {
        // A step's tool is added to the step for good or not at all: one
        // allow.
        !matches!(self.case, AskCase::CreatedExtras { .. } | AskCase::UnconfirmedSend { .. } | AskCase::StepTool { .. })
    }

    /// Why it asked, for the card: a step's missing tool names the step and
    /// the tool.
    pub fn reason_text(&self) -> String {
        match &self.case {
            AskCase::StepTool { step, tool } => format!(
                "The workflow step \u{201c}{step}\u{201d} wasn't given {tool}. Allowing it adds {tool} to that step and the run continues."
            ),
            _ => self.reason().to_string(),
        }
    }

    /// What this ask asks.
    pub fn kind(&self) -> AskKind {
        match self.case {
            AskCase::UnconfirmedSend { .. } => AskKind::SendCheck,
            _ => AskKind::Permission,
        }
    }

    /// Why it asked, in plain words for the card.
    pub fn reason(&self) -> &'static str {
        reason_of(&self.case)
    }
}

/// Why a call asks, in plain words.
pub fn reason_of(case: &AskCase) -> &'static str {
    match case {
        AskCase::Money { .. } => "It's over this employee's money limit.",
        AskCase::CompanyMoney { .. } => {
            "It's over what the company may spend unattended today."
        }
        AskCase::NewCounterparty { .. } => "It's the first time it would contact them.",
        AskCase::Irreversible { .. } => "It can't be undone.",
        AskCase::OutsideJob { .. } => "It's outside this employee's job.",
        AskCase::UntrustedInput { .. } => "It acts on something that came from outside.",
        AskCase::AskRule { .. } => "This needs your OK every time.",
        AskCase::AskMode => "This employee asks before it changes anything.",
        AskCase::Widens => "Only you can give an employee more room.",
        AskCase::RemovesEmployee => "Deleting an employee needs your OK every time.",
        AskCase::ReachesOwner { reach } => match reach {
            OwnerReach::Screen => "It would see what's on your screen.",
            OwnerReach::Microphone => "It would hear what your microphone picks up.",
            OwnerReach::Camera => "It would see what your camera sees.",
            OwnerReach::App { .. } | OwnerReach::Input => "It would act in your apps as you.",
        },
        AskCase::CreatedExtras { .. } => "It was made by another employee and needs more than that employee has.",
        AskCase::UnconfirmedSend { .. } => {
            "Nebo couldn't confirm it went out. Check the sent items, then say whether it did."
        }
        AskCase::StepTool { .. } => "It isn't one of the tools this workflow step was given.",
    }
}

/// Where the card goes and where answers are delivered. The server
/// implements it: the Inbox and the phone, the open chat, the wake rail,
/// the workflow engine.
pub trait AskSurfaces: Send + Sync {
    /// Show the card everywhere the owner might answer it.
    fn card(&self, ask: &Ask);
    /// The ask is still open after a while: bring its card back to the
    /// owner, in the Inbox and on the phone, as unread.
    fn remind(&self, ask: &Ask);
    /// The ask is settled: clear the card everywhere.
    fn resolved(&self, ask: &Ask);
    /// Deliver a notification row to `session_key`: a busy session hears it
    /// at its next step, an idle one starts a turn with it.
    fn notify(&self, session_key: &str, text: &str);
    /// A workflow run parked on an ask: release it, allowed or not.
    fn release_run(&self, run_id: &str, allowed: bool);
}

/// Why an answer wasn't taken.
#[derive(Debug, thiserror::Error)]
pub enum AskError {
    #[error("no such ask")]
    NotFound,
    /// Someone already answered it, somewhere else, or it was withdrawn.
    #[error("this was already answered")]
    Settled(Box<Ask>),
    /// The answer is not one this ask takes (a permission answer to a held
    /// send's question, or the other way round).
    #[error("that answer doesn't fit this question")]
    NotOffered,
    #[error("{0}")]
    Store(String),
}

/// The asks: parking, the card, answers, and what the engine does when an
/// ask's wait wakes.
pub struct Asks {
    store: Arc<db::Store>,
    surfaces: OnceLock<Arc<dyn AskSurfaces>>,
}

impl Asks {
    pub fn new(store: Arc<db::Store>) -> Self {
        Self { store, surfaces: OnceLock::new() }
    }

    /// Connect the card and the delivery. Once, when the server's hub is up;
    /// asks parked before that are shown by the Inbox's own list.
    pub fn attach(&self, surfaces: Arc<dyn AskSurfaces>) {
        if self.surfaces.set(surfaces).is_err() {
            tracing::warn!("ask surfaces already attached");
        }
    }

    fn surfaces(&self) -> Option<&Arc<dyn AskSurfaces>> {
        self.surfaces.get()
    }

    pub fn store(&self) -> &Arc<db::Store> {
        &self.store
    }

    /// Write the ask for `call`, send its card, and return its id.
    pub(super) fn park(&self, cx: &CheckCx<'_>, call: &ResolvedCall<'_>, case: &AskCase) -> String {
        let now = chrono::Utc::now().timestamp();
        let ask = Ask {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: cx.grant.agent_id.clone(),
            session_key: cx.ctx.session_key.clone(),
            door: cx.ctx.door.clone(),
            case: case.clone(),
            sentence: call.tool.activity(call.input),
            target: call.target.clone(),
            call: StoredCall { name: call.name().to_string(), input: call.input.clone() },
            seat: SeatSnapshot {
                grant: cx.grant.clone(),
                origin: cx.ctx.origin,
                user_id: cx.ctx.user_id.clone(),
                session_id: cx.ctx.session_id.clone(),
                untrusted_input: cx.ctx.untrusted_input,
                cwd: cx.ctx.cwd.clone(),
                handoff_depth: cx.ctx.handoff_depth,
            },
            status: AskStatus::Open,
            run_id: None,
            chat_id: originating_chat(&cx.ctx.door, &cx.ctx.session_key),
            created_at: now,
        };
        if let Err(e) = self.raise(&ask) {
            tracing::warn!(tool = %call.name(), error = %e, "ask not written");
        }
        ask.id
    }

    /// Write `ask` with its wait in the engine, and send its card.
    fn raise(&self, ask: &Ask) -> Result<(), types::NeboError> {
        self.store.insert_permission_ask(&db::PermissionAskRow {
            id: ask.id.clone(),
            agent_id: ask.agent_id.clone(),
            session_key: ask.session_key.clone(),
            chat_id: ask.chat_id.clone(),
            door: json(&ask.door),
            ask_case: json(&ask.case),
            sentence: ask.sentence.clone(),
            target: json(&ask.target),
            call: json(&ask.call),
            seat: json(&ask.seat),
            status: "open".to_string(),
            created_at: ask.created_at,
            ..Default::default()
        }, ask.created_at + reminder_after(0))?;
        if let Some(s) = self.surfaces() {
            s.card(ask);
        }
        Ok(())
    }

    /// The one card for an employee made by an employee that needs more
    /// than its creator holds (§2.12.7). `sentence` is the consent line for
    /// the extras; `asker` the grant of the run that made it. Its answer is
    /// [`super::consent::answer_extras`]; it has no call to run.
    pub fn raise_extras(
        &self,
        asker: &Grant,
        agent_id: &str,
        capabilities: Vec<String>,
        sentence: String,
        session_key: &str,
    ) -> Result<String, types::NeboError> {
        let now = chrono::Utc::now().timestamp();
        let ask = Ask {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            session_key: session_key.to_string(),
            door: Door::Chat,
            case: AskCase::CreatedExtras { capabilities },
            sentence,
            target: Target {
                tool: String::new(),
                key: String::new(),
                operation: None,
                capability: None,
                field: None,
                subject: None,
                read_only: false,
                effects: types::permissions::CallEffects { widens: true, ..Default::default() },
            },
            call: StoredCall { name: String::new(), input: serde_json::Value::Null },
            seat: SeatSnapshot {
                grant: asker.clone(),
                origin: tools::Origin::System,
                user_id: String::new(),
                session_id: String::new(),
                untrusted_input: false,
                cwd: None,
                handoff_depth: 0,
            },
            status: AskStatus::Open,
            run_id: None,
            chat_id: None,
            created_at: now,
        };
        self.raise(&ask)?;
        Ok(ask.id)
    }

    /// The ask the owner said no to for this same call in this session, if
    /// any: it is refused without a card.
    pub(super) fn declined_before(&self, session_key: &str, t: &Target, input: &serde_json::Value) -> Option<String> {
        let rows = self.store.declined_permission_asks(session_key).ok()?;
        rows.into_iter().filter_map(Ask::from_row).find_map(|a| {
            (a.target.key == t.key && a.target.field == t.field && a.call.input == *input).then_some(a.id)
        })
    }

    /// The open ask the same employee already raised about this same new
    /// person on the same key: a second card would ask the owner the same
    /// thing again. Live 2026-10-01: the same first email to the same new
    /// customer was asked twice, from two sessions.
    pub(super) fn waiting_already(&self, agent_id: &str, t: &Target, case: &AskCase) -> Option<String> {
        let AskCase::NewCounterparty { who } = case else { return None };
        let rows = self.store.open_permission_asks(None).ok()?;
        rows.into_iter().filter_map(Ask::from_row).find_map(|a| {
            let same_person = matches!(&a.case, AskCase::NewCounterparty { who: was } if was.eq_ignore_ascii_case(who));
            (a.agent_id == agent_id && a.target.key == t.key && same_person).then_some(a.id)
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<Ask>, AskError> {
        let row = self.store.get_permission_ask(id).map_err(|e| AskError::Store(e.to_string()))?;
        Ok(row.and_then(Ask::from_row))
    }

    /// The ask a workflow run is (or was last) parked on.
    pub fn for_run(&self, run_id: &str) -> Result<Option<Ask>, AskError> {
        let row = self
            .store
            .permission_ask_for_run(run_id)
            .map_err(|e| AskError::Store(e.to_string()))?;
        Ok(row.and_then(Ask::from_row))
    }

    /// The asks waiting on the owner, oldest first; `session_key` narrows
    /// them to one session (the open chat). One whose parked workflow run
    /// has ended is no longer needed: it is withdrawn here, the moment any
    /// surface looks, not left standing until its next reminder a day on.
    pub fn open(&self, session_key: Option<&str>) -> Result<Vec<Ask>, AskError> {
        let rows = self.store.open_permission_asks(session_key).map_err(|e| AskError::Store(e.to_string()))?;
        let now = chrono::Utc::now().timestamp();
        Ok(rows
            .into_iter()
            .filter_map(Ask::from_row)
            .filter(|a| match &a.run_id {
                // Seen ended, not merely unfound: a run still being written
                // is left to the reminder's own check.
                Some(run)
                    if self
                        .store
                        .engine_get_run(run)
                        .ok()
                        .flatten()
                        .is_some_and(|r| matches!(r.state.as_str(), "done" | "failed" | "cancelled")) =>
                {
                    if let Err(e) = self.withdraw(a.clone(), now) {
                        tracing::warn!(ask = %a.id, error = %e, "ask whose run ended not withdrawn");
                    }
                    false
                }
                _ => true,
            })
            .collect())
    }

    /// The owner's answer. The first answer anywhere wins; a later one
    /// gets [`AskError::Settled`]. The answer is recorded, the card cleared
    /// everywhere, and the answer signalled to the ask's wait; the engine
    /// wakes the run and [`Asks::resume`] applies it.
    pub fn answer(&self, id: &str, answer: Answer, via: AnsweredVia) -> Result<Ask, AskError> {
        let mut ask = self.get(id)?.ok_or(AskError::NotFound)?;
        if !ask.kind().takes(answer) {
            return Err(AskError::NotOffered);
        }
        let now = chrono::Utc::now().timestamp();
        let won = self
            .store
            .settle_permission_ask(id, "answered", Some(answer.as_str()), Some(via.as_str()), now)
            .map_err(|e| AskError::Store(e.to_string()))?;
        if !won {
            return Err(AskError::Settled(Box::new(self.get(id)?.ok_or(AskError::NotFound)?)));
        }
        ask.status = AskStatus::Answered { answer, via: Some(via) };
        if let Some(s) = self.surfaces() {
            s.resolved(&ask);
        }
        let key = db::ask_wait_key(id);
        self.store
            .engine_enqueue_event(&db::NewEvent {
                kind: ANSWER_SIGNAL,
                target_type: "run",
                target_id: &key,
                payload: answer.as_str(),
                channel: via.as_str(),
                idem_key: &format!("{key}:answer"),
                durable: true,
                ..Default::default()
            })
            .map_err(|e| AskError::Store(e.to_string()))?;
        Ok(ask)
    }

    /// The engine woke the ask's run: by the owner's answer, or by the
    /// wait's timer. An answered ask is applied, once. An open one comes
    /// back to the owner as a reminder and waits again until the next one;
    /// an open one whose parked workflow run is gone is withdrawn. Returns
    /// the task running an allowed call, when one runs here.
    pub fn resume(&self, registry: &Arc<tools::Registry>, id: &str, now: i64) -> Result<Option<tokio::task::JoinHandle<()>>, AskError> {
        let store_err = |e: types::NeboError| AskError::Store(e.to_string());
        let Some(ask) = self.get(id)? else {
            self.store.engine_close_run(id, "failed", now).map_err(store_err)?;
            return Ok(None);
        };
        match ask.status {
            AskStatus::Open => {
                if let Some(run) = &ask.run_id
                    && self.parked_run_is_gone(run)
                {
                    return self.withdraw(ask, now).map(|_| None);
                }
                if let Some(s) = self.surfaces() {
                    s.remind(&ask);
                }
                let reminded = self.store.engine_waits_for_run(id).map_err(store_err)?.len();
                let key = db::ask_wait_key(id);
                self.store
                    .engine_declare_wait(
                        id,
                        &db::NewWait {
                            action: "resume",
                            on_kind: ANSWER_SIGNAL,
                            key: &key,
                            deadline: Some(now + reminder_after(reminded)),
                            parked: None,
                            reason: &ask.sentence,
                        },
                        now,
                    )
                    .map_err(store_err)?;
                // Answered while this reminder went out: its signal may have
                // found no wait to wake. The answer is on the row; apply it.
                match self.get(id)? {
                    Some(answered) if answered.status != AskStatus::Open => self.apply(registry, answered, now),
                    _ => Ok(None),
                }
            }
            AskStatus::Answered { .. } | AskStatus::Withdrawn => self.apply(registry, ask, now),
        }
    }

    /// Close the ask's run and act on how it was settled. Closing is the
    /// claim: an ask whose run is already closed was applied before, so a
    /// second wake does nothing.
    fn apply(&self, registry: &Arc<tools::Registry>, ask: Ask, now: i64) -> Result<Option<tokio::task::JoinHandle<()>>, AskError> {
        if !self.store.engine_finish_run(&ask.id, now).map_err(|e| AskError::Store(e.to_string()))? {
            return Ok(None);
        }
        let AskStatus::Answered { answer, .. } = ask.status else {
            return Ok(None);
        };
        if let AskCase::UnconfirmedSend { effect_id } = ask.case {
            self.settle_send(&ask, effect_id, answer == Answer::Sent, now);
            return Ok(None);
        }
        // An answer the card didn't offer counts as the one it did.
        let answer = match answer {
            Answer::AllowAlways if !ask.allow_always_offered(&self.store) => Answer::ThisOnce,
            other => other,
        };
        if let AskCase::CreatedExtras { capabilities } = &ask.case {
            let allow = answer != Answer::No;
            if let Err(e) = super::consent::answer_extras(&self.store, &ask.id, allow) {
                tracing::warn!(ask = %ask.id, error = %e, "extras not granted");
            }
            let granted = format!("Granted: {}.", capabilities.join(", "));
            let outcome = if allow {
                AskOutcome::Ran { always: true, result: &granted, is_error: false }
            } else {
                AskOutcome::Declined
            };
            self.settle_without_running(&ask, outcome);
            return Ok(None);
        }
        // A step's missing tool: the one allow adds it to the step's
        // declaration (the run's own definition and the employee's
        // workflow), then the run continues at the call. No rule is written.
        if let AskCase::StepTool { tool, .. } = &ask.case {
            if answer == Answer::No {
                self.settle_without_running(&ask, AskOutcome::Declined);
                return Ok(None);
            }
            if let Some(run) = &ask.run_id
                && let Err(e) = self.store.add_step_tool(run, tool)
            {
                tracing::warn!(ask = %ask.id, error = %e, "the step's tool was not added; it runs this once");
            }
            return Ok(self.run(registry, ask, true));
        }
        let mut always = false;
        if answer == Answer::AllowAlways {
            always = true;
            for rule in allow_always_rules(&self.store, &ask).unwrap_or_default() {
                // A locked must-ask can't be loosened: it runs this once.
                if let Err(e) = self.store.write_permission_rule(&rule, &Writer::Owner) {
                    tracing::warn!(ask = %ask.id, error = %e, "allow-always rule not written; runs once");
                    always = false;
                }
            }
        }
        Ok(match answer {
            Answer::No => {
                self.settle_without_running(&ask, AskOutcome::Declined);
                None
            }
            Answer::AllowAlways | Answer::ThisOnce => self.run(registry, ask, always),
            // Not a permission's answer: `answer` refused it.
            Answer::Sent | Answer::NotSent => None,
        })
    }

    /// The owner's question for a send that was attempted and whose outcome
    /// never came back (the ledger row `effect` is held): did it go out?
    /// One card per row, in the conversation that sent it (the Inbox when
    /// that is no chat); it waits and comes back as a reminder like any ask,
    /// until he answers. Returns the ask's id when this call raised it,
    /// `None` when the row already has one.
    pub fn raise_send_check(
        &self,
        effect: &db::EngineEffect,
        agent_id: &str,
        session_key: &str,
        sentence: String,
    ) -> Result<Option<String>, types::NeboError> {
        let id = send_check_id(effect.id);
        if self.store.get_permission_ask(&id)?.is_some() {
            return Ok(None);
        }
        let door = match tools::origin::workflow_run_id(session_key) {
            Some(_) => Door::Workflow,
            None => Door::Chat,
        };
        let chat_id = originating_chat(&door, session_key);
        let ask = Ask {
            id: id.clone(),
            agent_id: agent_id.to_string(),
            session_key: session_key.to_string(),
            door,
            case: AskCase::UnconfirmedSend { effect_id: effect.id },
            sentence,
            target: Target {
                tool: String::new(),
                key: String::new(),
                operation: None,
                capability: None,
                field: None,
                subject: None,
                read_only: false,
                effects: types::permissions::CallEffects::default(),
            },
            call: StoredCall { name: String::new(), input: serde_json::Value::Null },
            seat: SeatSnapshot {
                grant: Grant::new(agent_id, types::permissions::Mode::default()),
                origin: tools::Origin::System,
                user_id: String::new(),
                session_id: String::new(),
                untrusted_input: false,
                cwd: None,
                handoff_depth: 0,
            },
            status: AskStatus::Open,
            run_id: None,
            chat_id,
            created_at: chrono::Utc::now().timestamp(),
        };
        self.raise(&ask)?;
        Ok(Some(id))
    }

    /// The owner said whether a held send went out: its ledger row takes
    /// that as its outcome, once. It went: completed, and the people it
    /// went to are people the employee works with. It didn't: failed, so it
    /// may be sent again.
    fn settle_send(&self, ask: &Ask, effect_id: i64, sent: bool, now: i64) {
        let effect = match self.store.engine_get_effect(effect_id) {
            Ok(Some(e)) if e.state == "pending" => e,
            Ok(_) => return,
            Err(e) => {
                tracing::warn!(ask = %ask.id, effect = effect_id, error = %e, "held send not settled: its row is unreadable");
                return;
            }
        };
        let written = if sent {
            self.store.engine_effect_completed(effect_id, None, Some("The owner confirmed it went out."), now).map(|_| {
                for who in effect.counterparty.as_deref().unwrap_or("").split(',').filter(|w| !w.is_empty()) {
                    if let Err(e) = self.store.add_employee_counterparty(&ask.agent_id, who, "sent") {
                        tracing::warn!(error = %e, "counterparty not recorded");
                    }
                }
            })
        } else {
            self.store.engine_effect_failed(effect_id, "The owner confirmed it did not go out.", now).map(|_| ())
        };
        match written {
            Ok(()) => tracing::info!(effect = effect_id, ask = %ask.id, sent, "held send settled by the owner"),
            Err(e) => tracing::warn!(effect = effect_id, ask = %ask.id, error = %e, "held send's answer not written"),
        }
    }

    /// Whether the workflow run parked on an ask has ended, so nothing
    /// waits for the answer any more.
    fn parked_run_is_gone(&self, run_id: &str) -> bool {
        match self.store.engine_get_run(run_id) {
            Ok(Some(run)) => matches!(run.state.as_str(), "done" | "failed" | "cancelled"),
            Ok(None) => true,
            Err(_) => false,
        }
    }

    /// Nothing waits for this ask any more: settle it as withdrawn, clear
    /// its card everywhere and close its run. Nothing runs, and it is not a
    /// No.
    fn withdraw(&self, mut ask: Ask, now: i64) -> Result<(), AskError> {
        let store_err = |e: types::NeboError| AskError::Store(e.to_string());
        if self.store.settle_permission_ask(&ask.id, "withdrawn", None, None, now).map_err(store_err)? {
            ask.status = AskStatus::Withdrawn;
            if let Some(s) = self.surfaces() {
                s.resolved(&ask);
            }
        }
        self.store.engine_finish_run(&ask.id, now).map_err(store_err)?;
        Ok(())
    }

    /// A No: the parked workflow run ends refused, or the employee is told.
    fn settle_without_running(&self, ask: &Ask, outcome: AskOutcome<'_>) {
        let Some(s) = self.surfaces() else { return };
        match &ask.run_id {
            Some(run) => s.release_run(run, false),
            None => s.notify(&ask.session_key, &render_ask_outcome(&ask.id, &ask.sentence, &outcome)),
        }
    }

    /// Run the allowed call. A parked workflow run is released and resumes
    /// at the call itself; any other run gets the call's result as a
    /// notification. The ask's run was closed before this runs, so the call
    /// runs at most once, even across a restart.
    fn run(&self, registry: &Arc<tools::Registry>, ask: Ask, always: bool) -> Option<tokio::task::JoinHandle<()>> {
        let surfaces = self.surfaces().cloned();
        if let Some(run) = &ask.run_id {
            if let Some(s) = &surfaces {
                s.release_run(run, true);
            }
            return None;
        }
        let registry = registry.clone();
        Some(tokio::spawn(async move {
            let seat = ask.seat.clone();
            let mut ctx = tools::ToolContext::new(seat.origin).with_session(ask.session_key.clone(), seat.session_id);
            ctx.user_id = seat.user_id;
            ctx.grant = Some(Arc::new(seat.grant));
            ctx.door = ask.door.clone();
            ctx.untrusted_input = seat.untrusted_input;
            ctx.cwd = seat.cwd;
            ctx.handoff_depth = seat.handoff_depth;
            // The owner allowed exactly this call; the check still applies
            // the hard limits, the ceiling and deny rules.
            ctx.answered_ask = Some(ask.id.clone());
            let result = registry.execute(&ctx, &ask.call.name, ask.call.input.clone()).await;
            let text = render_ask_outcome(
                &ask.id,
                &ask.sentence,
                &AskOutcome::Ran { always, result: &result.content, is_error: result.is_error },
            );
            if let Some(s) = surfaces {
                s.notify(&ask.session_key, &text);
            }
        }))
    }
}

/// The owner's conversation an ask raised through `door` on `session_key`
/// belongs to: the chat thread itself, when the owner's own conversation
/// (typed or spoken) raised it. Read from the thread's own key, never
/// guessed: a schedule, a workflow, a heartbeat, a coworker's request or a
/// helper has none, and its ask goes to the Inbox.
pub fn originating_chat(door: &Door, session_key: &str) -> Option<String> {
    if !matches!(door, Door::Chat | Door::Voice) || types::keyparser::is_subagent_key(session_key) {
        return None;
    }
    let chat = types::keyparser::chat_id_from_thread_key(session_key)?;
    (!chat.is_empty() && !chat.contains(':')).then(|| chat.to_string())
}

/// The id of the one ask a held send's ledger row gets.
pub fn send_check_id(effect_id: i64) -> String {
    format!("send-check-{effect_id}")
}

/// Most rules one "Allow always" on a compound command saves, so one answer
/// can't quietly open a long list of commands.
const MAX_COMMAND_RULES: usize = 5;

/// The standing allows "Allow always" writes, for this employee: the rule
/// the ask's case names (§2.12.4). A shell command gets one rule per command
/// it runs that needed the answer, so the same command matches again next
/// time; `None` when one of them can't be read (no rule could cover
/// it).
pub fn allow_always_rules(store: &db::Store, ask: &Ask) -> Option<Vec<Rule>> {
    let t = &ask.target;
    if let AskCase::ReachesOwner { .. } = ask.case {
        return reach_rules(ask);
    }
    let per_command = !matches!(
        ask.case,
        AskCase::OutsideJob { .. }
            | AskCase::Money { .. }
            | AskCase::CompanyMoney { .. }
            | AskCase::NewCounterparty { .. }
            | AskCase::Widens
            | AskCase::RemovesEmployee
            | AskCase::CreatedExtras { .. }
            | AskCase::UnconfirmedSend { .. }
            | AskCase::StepTool { .. }
    );
    if per_command && matches!(t.field, Some(RuleField::CommandPrefix(_))) {
        return command_rules(ask);
    }
    Some(vec![allow_always_rule(store, ask)])
}

/// One allow per command of a shell call that needed the answer: the ask
/// rule it met, else the command's own prefix.
fn command_rules(ask: &Ask) -> Option<Vec<Rule>> {
    let t = &ask.target;
    let rules = RuleSet::of(&ask.seat.grant);
    let mut out: Vec<Rule> = Vec::new();
    for piece in super::rules::pieces(t) {
        let answered = match &ask.case {
            AskCase::AskRule { .. } => !matches!(rules.decide_piece(t, piece.as_ref()), Some((_, Effect::Ask | Effect::Deny))),
            AskCase::AskMode => rules.piece_allowed_by(t, piece.as_ref(), |r| !matches!(r.key, RuleKey::Capability(_))),
            _ => rules.piece_allowed_by(t, piece.as_ref(), |r| matches!(r.source, RuleSource::AllowAlways { .. })),
        };
        if answered {
            continue;
        }
        let (key, field) = match rules.decide_piece(t, piece.as_ref()) {
            Some((rule, Effect::Ask)) if rule.effect == Effect::Ask => (rule.key.clone(), rule.field.clone()),
            _ => (RuleKey::Tool(t.key.clone()), Some(RuleField::CommandPrefix(piece.as_ref()?.rule_prefix()?))),
        };
        if !out.iter().any(|r| r.key == key && r.field == field) {
            out.push(standing_allow(ask, key, field, None));
        }
    }
    out.truncate(MAX_COMMAND_RULES);
    Some(out)
}

/// One allow per command of the call that watches the owner or drives his
/// apps and isn't allowed yet: the program (`screencapture`, `osascript`,
/// `open -a`), so the next one is covered whatever its file or script.
/// `None` when one of them can't be named. A call that is not a command
/// (the `os` tool on his calendar, the `message` tool on his Messages) gets
/// one allow for its own key (`calendar_event_list`): that call, for this
/// employee, and nothing wider.
fn reach_rules(ask: &Ask) -> Option<Vec<Rule>> {
    let t = &ask.target;
    if !matches!(t.field, Some(RuleField::CommandPrefix(_))) {
        return Some(vec![standing_allow(ask, RuleKey::Tool(t.key.clone()), None, None)]);
    }
    let rules = RuleSet::of(&ask.seat.grant);
    let mut out: Vec<Rule> = Vec::new();
    let mut reaching = false;
    for piece in super::rules::pieces(t).into_iter().flatten() {
        if tools::policy::reach_of(&piece).is_none() {
            continue;
        }
        reaching = true;
        if rules.reach_piece_allowed(t, &piece) {
            continue;
        }
        let field = Some(RuleField::CommandPrefix(tools::policy::reach_prefix(&piece)?));
        let key = RuleKey::Tool(t.key.clone());
        if !out.iter().any(|r| r.key == key && r.field == field) {
            out.push(standing_allow(ask, key, field, None));
        }
    }
    reaching.then_some(out)
}

fn standing_allow(ask: &Ask, key: RuleKey, field: Option<RuleField>, money: Option<MoneyLimit>) -> Rule {
    Rule {
        id: uuid::Uuid::new_v4().to_string(),
        scope: Scope::Employee(ask.agent_id.clone()),
        key,
        field,
        effect: Effect::Allow,
        money,
        source: RuleSource::AllowAlways { ask_id: ask.id.clone() },
        locked: false,
        created_at: chrono::Utc::now().timestamp(),
    }
}

/// The one standing allow for any call but a shell command's.
fn allow_always_rule(store: &db::Store, ask: &Ask) -> Rule {
    let t = &ask.target;
    let call_key = || match &t.operation {
        Some(op) => RuleKey::Operation(op.clone()),
        None => RuleKey::Tool(t.key.clone()),
    };
    let who = |fallback: Option<&str>| match &t.field {
        Some(f @ (RuleField::Recipient(_) | RuleField::Domain(_))) => Some(f.clone()),
        _ => t.effects.recipients.first().map(String::as_str).or(fallback).map(|r| RuleField::Recipient(r.to_string())),
    };
    let (key, field, money) = match &ask.case {
        // Outside the job: the capability joins the job for good.
        AskCase::OutsideJob { .. } => {
            let allowed = RuleSet::of(&ask.seat.grant).decide(t).is_some_and(|(_, e)| e == Effect::Allow);
            match &t.capability {
                // The capability is in the job; the file is outside its
                // folders.
                Some(c) if allowed => (RuleKey::Capability(c.clone()), t.field.clone(), None),
                Some(c) => (RuleKey::Capability(c.clone()), None, None),
                None => (call_key(), t.field.clone(), None),
            }
        }
        AskCase::NewCounterparty { who: to } => (call_key(), who(Some(to)), None),
        AskCase::UntrustedInput { .. } => (call_key(), who(None), None),
        AskCase::Money { cents, .. } => money_cover(store, ask, *cents),
        // An ask rule the owner can loosen: the allow takes its place.
        AskCase::AskRule { rule_id } => match store.get_permission_rule(rule_id).ok().flatten() {
            Some(r) => (r.key, r.field, None),
            None => (call_key(), t.field.clone(), None),
        },
        AskCase::Irreversible { .. }
        | AskCase::AskMode
        | AskCase::ReachesOwner { .. }
        | AskCase::Widens
        | AskCase::RemovesEmployee
        | AskCase::CreatedExtras { .. }
        | AskCase::UnconfirmedSend { .. }
        | AskCase::StepTool { .. }
        | AskCase::CompanyMoney { .. } => (call_key(), t.field.clone(), None),
    };
    standing_allow(ask, key, field, money)
}

/// The money case: the standing allow that decided the call, with every
/// limit this amount went over raised just enough to cover it.
fn money_cover(store: &db::Store, ask: &Ask, cents: i64) -> (RuleKey, Option<RuleField>, Option<MoneyLimit>) {
    let t = &ask.target;
    let rules = RuleSet::of(&ask.seat.grant);
    let Some((rule, _)) = rules.decide(t) else {
        return (RuleKey::Tool(t.key.clone()), t.field.clone(), None);
    };
    let mut limit = rule.money.clone().unwrap_or_default();
    let counterparty = t.effects.counterparty.clone().unwrap_or_default();
    let spent = store
        .permission_spend(&ask.agent_id, &super::today(), rule.key.value(), &counterparty)
        .unwrap_or_default();
    let raise = |l: &mut Option<i64>, need: i64| {
        if let Some(v) = l {
            *v = (*v).max(need);
        }
    };
    raise(&mut limit.per_action_cents, cents);
    raise(&mut limit.per_day_cents, spent.cents + cents);
    raise(&mut limit.per_day_count, spent.count + 1);
    if !counterparty.is_empty() {
        raise(&mut limit.per_counterparty_day_cents, spent.counterparty_cents + cents);
    }
    (rule.key.clone(), rule.field.clone(), Some(limit))
}

/// What the model hears for a parked call: what waits, why, and where the
/// owner answers it, so the model never has to guess (live 2026-09-28: told
/// nothing, it sent the owner on his phone to "the desktop app", then to
/// support).
pub fn parked_text(sentence: &str, case: &AskCase, door: &Door, origin: tools::Origin) -> String {
    let why = match case {
        AskCase::Money { .. } => " It is over this employee's money limit.",
        AskCase::CompanyMoney { .. } => " It is over what the company may spend unattended today.",
        AskCase::OutsideJob { .. } => " It is outside this employee's job.",
        AskCase::UntrustedInput { .. } => " It acts on words that came from outside.",
        AskCase::StepTool { .. } => " It isn't one of the tools this workflow step was given; the owner can add it.",
        _ => "",
    };
    format!("Waiting for the owner to allow: {sentence}.{why} {}", where_to_answer(door, origin))
}

/// Where the owner answers an ask raised through `door` by a run of
/// `origin`. Every ask's card is in the owner's Inbox, in the Nebo app on
/// each of his devices; the conversation that asked shows it too, and on
/// his own call his spoken answer is the answer. Someone else's words (a
/// phone caller, a stranger's message) never answer one.
fn where_to_answer(door: &Door, origin: tools::Origin) -> &'static str {
    if !origin.is_trusted() {
        return "Only the owner can allow it, in their Nebo app; the person you are talking with can't, so \
                don't ask them to. Carry on with anything else; the answer arrives as a notification. Don't \
                retry this action.";
    }
    match door {
        Door::Voice => {
            "You are on a call with the owner: ask them now, in one short spoken question (for example \
             \"Want me to send it?\"). Their spoken yes or no on this call is the answer; they can also tap \
             the card in this conversation or in their Inbox, in the Nebo app on any of their devices. \
             Don't retry this action."
        }
        Door::Chat => {
            "The owner answers on the card in this conversation or in their Inbox, in the Nebo app on any of \
             their devices, phone or computer. Carry on with anything else; the answer arrives as a \
             notification. Don't retry this action."
        }
        _ => {
            "The owner answers in their Inbox, in the Nebo app on any of their devices, phone or computer. \
             Carry on with anything else; the answer arrives as a notification. Don't retry this action."
        }
    }
}

/// Why a call that needed the owner's OK didn't run in a run nothing can
/// wait in (a scheduled command). The owner reads it in the job's failure.
pub fn cannot_wait_text(sentence: &str, case: &AskCase, door: &Door) -> String {
    format!(
        "Didn't run: {sentence}. It needs the owner's OK, and {} can't wait for one. {}",
        Door::unattended_words(door.label()),
        reason_of(case)
    )
}

/// What the model hears for a call the owner is already being asked about.
pub fn already_waiting_text(sentence: &str) -> String {
    format!(
        "An earlier ask for this is still waiting for the owner's OK, so this ({sentence}) wasn't \
         asked again. You'll be told when he answers; then do it if it's still needed. Don't ask \
         again now; carry on with other work."
    )
}

/// What the model hears for a call the owner already said no to.
pub fn declined_text(sentence: &str) -> String {
    format!(
        "The owner already said no to this ({sentence}), so it didn't run. Don't ask again or try \
         another way to do it; plan around it."
    )
}

/// A stored field. These types always serialize.
fn json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;
    use tools::registry::DynTool;
    use tools::{Origin, Registry, ToolContext, ToolResult};
    use types::permissions::{Ceiling, Mode};

    use super::super::Check;
    use super::*;

    /// What reached the owner and the employee.
    #[derive(Default)]
    struct Seen {
        cards: Mutex<Vec<String>>,
        reminded: Mutex<Vec<String>>,
        resolved: Mutex<Vec<(String, AskStatus)>>,
        notes: Mutex<Vec<(String, String)>>,
        released: Mutex<Vec<(String, bool)>>,
    }

    impl AskSurfaces for Seen {
        fn card(&self, ask: &Ask) {
            self.cards.lock().unwrap().push(ask.id.clone());
        }
        fn remind(&self, ask: &Ask) {
            self.reminded.lock().unwrap().push(ask.id.clone());
        }
        fn resolved(&self, ask: &Ask) {
            self.resolved.lock().unwrap().push((ask.id.clone(), ask.status));
        }
        fn notify(&self, session_key: &str, text: &str) {
            self.notes.lock().unwrap().push((session_key.to_string(), text.to_string()));
        }
        fn release_run(&self, run_id: &str, allowed: bool) {
            self.released.lock().unwrap().push((run_id.to_string(), allowed));
        }
    }

    impl Seen {
        fn cards(&self) -> usize {
            self.cards.lock().unwrap().len()
        }
        fn notes(&self) -> Vec<(String, String)> {
            self.notes.lock().unwrap().clone()
        }
    }

    /// A tool that counts the calls that ran.
    struct Probe {
        name: &'static str,
        capability: Option<&'static str>,
        ran: Arc<AtomicUsize>,
    }

    impl DynTool for Probe {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> String {
            String::new()
        }
        fn schema(&self) -> serde_json::Value {
            json!({ "type": "object" })
        }
        fn capability(&self, _input: &serde_json::Value) -> Option<&'static str> {
            self.capability
        }
        fn activity(&self, input: &serde_json::Value) -> String {
            format!("{} {}", self.name, input["to"].as_str().unwrap_or("the books"))
        }
        fn effects(&self, input: &serde_json::Value) -> types::permissions::CallEffects {
            let mut e = types::permissions::CallEffects::unknown();
            if let Some(to) = input["to"].as_str() {
                e.recipients = vec![to.to_string()];
            }
            e
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a ToolContext,
            _input: serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
            self.ran.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { ToolResult::ok("DONE") })
        }
    }

    struct Rig {
        _dir: tempfile::TempDir,
        store: Arc<db::Store>,
        reg: Arc<Registry>,
        asks: Arc<Asks>,
        seen: Arc<Seen>,
        /// read_books (basic work), text (capability sms), tally (basic).
        ran: [Arc<AtomicUsize>; 3],
    }

    const KEY: &str = "agent:emp:heartbeat";

    async fn rig() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("a.db").to_string_lossy()).unwrap());
        // People the employees already text: these asks are about the job
        // (case 4), never a first message (case 2).
        for agent in ["emp", ""] {
            for to in ["+15550142", "+15550177"] {
                store.add_employee_counterparty(agent, to, "sent").unwrap();
            }
        }
        let check = Arc::new(Check::new(store.clone()));
        let asks = check.asks();
        let seen = Arc::new(Seen::default());
        asks.attach(seen.clone());
        let reg = Arc::new(Registry::new(check));
        let ran = [Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0))];
        for (name, capability, ran) in [("read_books", None, &ran[0]), ("text", Some("sms"), &ran[1]), ("tally", None, &ran[2])] {
            reg.register(Box::new(Probe { name, capability, ran: ran.clone() })).await;
        }
        Rig { _dir: dir, store, reg, asks, seen, ran }
    }

    fn ctx(key: &str, door: Door) -> ToolContext {
        let mut c = ToolContext::new(Origin::System).with_session(key, "s1");
        c.door = door;
        c
    }

    fn text(to: &str) -> serde_json::Value {
        json!({ "to": to })
    }

    impl Rig {
        fn ran(&self, i: usize) -> usize {
            self.ran[i].load(Ordering::SeqCst)
        }

        /// Park a text to `to` on `key` and return the ask id.
        async fn park(&self, key: &str, door: Door, to: &str) -> String {
            let r = self.reg.execute(&ctx(key, door), "text", text(to)).await;
            assert!(r.content.starts_with("Waiting for the owner to allow: text"), "{}", r.content);
            r.parked_ask.expect("parked")
        }

        /// The owner answers, and the engine wakes the ask's run on the
        /// answer: the asks apply it.
        async fn answer(&self, id: &str, a: Answer, via: AnsweredVia) -> Result<Ask, AskError> {
            let ask = self.asks.answer(id, a, via)?;
            if let Some(h) = self.asks.resume(&self.reg, id, chrono::Utc::now().timestamp())? {
                h.await.unwrap();
            }
            Ok(ask)
        }
    }

    /// D11: the owner approves the plan on the one ask card: the employee
    /// leaves plan mode (to the company's mode) and hears the approved plan
    /// as the answer. A No keeps it in plan mode.
    #[tokio::test]
    async fn approving_the_plan_card_leaves_plan_mode() {
        let r = rig().await;
        r.reg.register(Box::new(tools::file_tools::ExitPlanModeTool::new(r.store.clone()))).await;
        let emp = types::permissions::Scope::Employee("emp".into());
        r.store.set_permission_mode(&emp, Mode::Plan).unwrap();
        let doc = r._dir.path().join("rivera-plan.md");
        std::fs::write(&doc, "# Rivera listing\n\n- [ ] 1. Update the price\n  verify: `true`\n").unwrap();
        let mut c = ctx(KEY, Door::Chat);
        c.grant = Some(Arc::new(crate::harness::permissions::resolve_grant(&r.store, "emp", None)));
        let input = json!({ "path": doc.to_string_lossy() });

        // A No: still planning.
        let parked = r.reg.execute(&c, "exit_plan_mode", input.clone()).await;
        let id = parked.parked_ask.clone().expect("parked on the owner");
        assert_eq!(r.seen.cards(), 1, "one card");
        r.answer(&id, Answer::No, AnsweredVia::Chat).await.unwrap();
        assert_eq!(r.store.permission_mode(&emp).unwrap(), Some(Mode::Plan));

        // The same plan again is refused without a card; a revised plan
        // is a new card.
        let same = r.reg.execute(&c, "exit_plan_mode", input.clone()).await;
        assert!(same.parked_ask.is_none() && same.content.contains("already said no"), "{}", same.content);
        std::fs::write(&doc, "# Rivera listing\n\n- [ ] 1. Update the price and the photos\n  verify: `true`\n").unwrap();

        // Approved: out of plan mode, and the plan comes back as the answer.
        let again = r.reg.execute(&c, "exit_plan_mode", input).await;
        assert!(again.parked_ask.is_some(), "the revised plan goes to the owner: {}", again.content);
        r.answer(again.parked_ask.as_deref().unwrap(), Answer::ThisOnce, AnsweredVia::Mobile).await.unwrap();
        assert_eq!(r.store.permission_mode(&emp).unwrap(), Some(Mode::Automatic), "the company's mode");
        let (_, note) = r.seen.notes().pop().unwrap();
        assert!(note.contains("The owner approved your plan"), "{note}");
        assert!(note.contains("Update the price and the photos"), "the approved plan comes back: {note}");
    }

    /// Three steps of one unattended run: the one that asks waits, the
    /// others run, and one card goes out.
    #[tokio::test]
    async fn ask_parks_only_that_step_and_the_turn_continues() {
        let r = rig().await;
        let c = ctx(KEY, Door::Heartbeat);
        let (a, b, t) = tokio::join!(
            r.reg.execute(&c, "read_books", json!({})),
            r.reg.execute(&c, "text", text("+15550142")),
            r.reg.execute(&c, "tally", json!({})),
        );
        assert_eq!((a.content.as_str(), t.content.as_str()), ("DONE", "DONE"));
        assert!(b.parked_ask.is_some() && b.content.contains("Carry on with anything else"), "{}", b.content);
        assert_eq!((r.ran(0), r.ran(1), r.ran(2)), (1, 0, 1));
        let open = r.asks.open(Some(KEY)).unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].door, Door::Heartbeat);
        assert_eq!(open[0].case, AskCase::OutsideJob { capability: "sms".into() });
        assert_eq!(r.seen.cards(), 1, "one card");
    }

    /// The card goes out once, with what the owner needs to answer it.
    #[tokio::test]
    async fn card_reaches_inbox_mobile_and_open_chat() {
        let r = rig().await;
        let id = r.park(KEY, Door::Chat, "+15550142").await;
        assert_eq!(*r.seen.cards.lock().unwrap(), vec![id.clone()]);
        let ask = r.asks.get(&id).unwrap().unwrap();
        assert_eq!(ask.sentence, "text +15550142");
        assert_eq!(ask.reason(), "It's outside this employee's job.");
        assert!(ask.allow_always_offered(&r.store));
        // The ask is a wait in the engine: its run waits on the answer, and
        // the wait's timer is the first reminder, a day out. No expiry.
        let run = r.store.engine_get_run(&id).unwrap().expect("the ask's run");
        assert_eq!((run.kind.as_str(), run.state.as_str()), ("ask", "waiting"));
        let wait = r.store.engine_get_wait(run.current_wait_id.unwrap()).unwrap().unwrap();
        assert_eq!((wait.on_kind.as_str(), wait.key.as_str()), (ANSWER_SIGNAL, format!("ask:{id}").as_str()));
        assert_eq!(wait.deadline, Some(ask.created_at + 24 * 3600));
    }

    #[tokio::test]
    async fn first_answer_wins_everywhere() {
        let r = rig().await;
        let id = r.park(KEY, Door::Chat, "+15550142").await;
        let first = r.answer(&id, Answer::ThisOnce, AnsweredVia::Mobile).await.unwrap();
        assert_eq!(first.status, AskStatus::Answered { answer: Answer::ThisOnce, via: Some(AnsweredVia::Mobile) });
        match r.answer(&id, Answer::No, AnsweredVia::Chat).await {
            Err(AskError::Settled(ask)) => assert_eq!(ask.status, first.status),
            other => panic!("a second answer was taken: {:?}", other.map(|a| a.status)),
        }
        assert_eq!(r.seen.resolved.lock().unwrap().len(), 1, "cleared once, everywhere");
        assert_eq!(r.ran(1), 1, "ran once");
        assert!(matches!(r.answer("nope", Answer::No, AnsweredVia::Inbox).await, Err(AskError::NotFound)));
    }

    /// Outside the job: "Allow always" grows the job, the step runs, and the
    /// same thing never asks again.
    #[tokio::test]
    async fn allow_always_writes_the_case_rule_and_never_asks_again() {
        let r = rig().await;
        let id = r.park(KEY, Door::Chat, "+15550142").await;
        r.answer(&id, Answer::AllowAlways, AnsweredVia::Inbox).await.unwrap();
        assert_eq!(r.ran(1), 1);
        let rules = r.store.permission_rules("emp").unwrap();
        let grown = rules.iter().find(|x| x.key == RuleKey::Capability("sms".into())).expect("the job grew");
        assert_eq!((grown.effect, grown.scope.clone()), (Effect::Allow, Scope::Employee("emp".into())));
        assert_eq!(grown.source, RuleSource::AllowAlways { ask_id: id });
        let again = r.reg.execute(&ctx(KEY, Door::Chat), "text", text("+15550142")).await;
        assert_eq!(again.content, "DONE", "never asks twice");
        assert_eq!((r.ran(1), r.seen.cards()), (2, 1));
    }

    /// An ask rule: "Allow always" takes its place.
    #[tokio::test]
    async fn allow_always_replaces_an_ask_rule() {
        let r = rig().await;
        let ask_rule = r
            .store
            .write_permission_rule(
                &Rule {
                    id: String::new(),
                    scope: Scope::Employee("emp".into()),
                    key: RuleKey::Tool("tally".into()),
                    field: None,
                    effect: Effect::Ask,
                    money: None,
                    source: RuleSource::Owner,
                    locked: false,
                    created_at: 0,
                },
                &Writer::Owner,
            )
            .unwrap();
        let parked = r.reg.execute(&ctx(KEY, Door::Chat), "tally", json!({})).await;
        let id = parked.parked_ask.expect("the ask rule asks");
        assert_eq!(r.asks.get(&id).unwrap().unwrap().case, AskCase::AskRule { rule_id: ask_rule.id });
        r.answer(&id, Answer::AllowAlways, AnsweredVia::Chat).await.unwrap();
        assert_eq!(r.reg.execute(&ctx(KEY, Door::Chat), "tally", json!({})).await.content, "DONE");
        assert_eq!(r.ran(2), 2);
    }

    /// A new recipient: "Allow always" names that recipient only.
    #[test]
    fn allow_always_for_a_new_recipient_names_only_that_recipient() {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(&dir.path().join("r.db").to_string_lossy()).unwrap();
        let target = |to: &str| Target {
            tool: "text".into(),
            key: "sms_message_send".into(),
            operation: None,
            capability: Some("sms".into()),
            field: None,
            subject: None,
            read_only: false,
            effects: types::permissions::CallEffects { recipients: vec![to.into()], ..Default::default() },
        };
        let ask = Ask {
            id: "a1".into(),
            agent_id: "emp".into(),
            session_key: KEY.into(),
            door: Door::Chat,
            case: AskCase::NewCounterparty { who: "+15550142".into() },
            sentence: "texting +15550142".into(),
            target: target("+15550142"),
            call: StoredCall { name: "text".into(), input: json!({}) },
            seat: SeatSnapshot {
                grant: Grant::new("emp", Mode::Automatic),
                origin: Origin::User,
                user_id: String::new(),
                session_id: String::new(),
                untrusted_input: false,
                cwd: None,
                handoff_depth: 0,
            },
            status: AskStatus::Open,
            run_id: None,
            chat_id: None,
            created_at: 0,
        };
        let rule = allow_always_rules(&store, &ask).unwrap().remove(0);
        assert_eq!(rule.field, Some(RuleField::Recipient("+15550142".into())));
        assert!(super::super::rules::matches(&rule, &target("+15550142")));
        assert!(!super::super::rules::matches(&rule, &target("+15550177")), "not texting in general");
    }

    /// An ask about `cmd` (a shell command) under `rules`, for `case`.
    fn shell_ask(case: AskCase, cmd: &str, rules: Vec<Rule>) -> Ask {
        let mut grant = Grant::new("emp", Mode::Ask);
        grant.rules = rules;
        Ask {
            id: "a1".into(),
            agent_id: "emp".into(),
            session_key: KEY.into(),
            door: Door::Chat,
            case,
            sentence: format!("running {cmd}"),
            target: Target {
                tool: "run_command".into(),
                key: "run_command".into(),
                operation: None,
                capability: Some("shell".into()),
                field: Some(RuleField::CommandPrefix(cmd.into())),
                subject: None,
                read_only: false,
                effects: types::permissions::CallEffects::unknown(),
            },
            call: StoredCall { name: "run_command".into(), input: json!({ "command": cmd }) },
            seat: SeatSnapshot {
                grant,
                origin: Origin::User,
                user_id: String::new(),
                session_id: String::new(),
                untrusted_input: false,
                cwd: None,
                handoff_depth: 0,
            },
            status: AskStatus::Open,
            run_id: None,
            chat_id: None,
            created_at: 0,
        }
    }

    fn command_rule(effect: Effect, prefix: &str) -> Rule {
        Rule {
            id: uuid::Uuid::new_v4().to_string(),
            scope: Scope::Employee("emp".into()),
            key: RuleKey::Tool("run_command".into()),
            field: Some(RuleField::CommandPrefix(prefix.into())),
            effect,
            money: None,
            source: RuleSource::Owner,
            locked: false,
            created_at: 0,
        }
    }

    fn prefixes(rules: &[Rule]) -> Vec<String> {
        rules
            .iter()
            .map(|r| match &r.field {
                Some(RuleField::CommandPrefix(p)) => p.clone(),
                other => panic!("not a command rule: {other:?}"),
            })
            .collect()
    }

    /// "Allow always" on a compound command saves one rule per command that
    /// needed the answer, and the same command never asks again. The whole compound text, saved as one
    /// rule, never matched again.
    #[test]
    fn allow_always_on_a_compound_command_saves_one_rule_per_command() {
        let (_d, store) = {
            let dir = tempfile::tempdir().unwrap();
            let store = db::Store::new(&dir.path().join("c.db").to_string_lossy()).unwrap();
            (dir, store)
        };
        let cmd = "cd src && git push origin main | tee log.txt";
        let owner_ls = command_rule(Effect::Allow, "tee log.txt");
        let ask = shell_ask(AskCase::AskMode, cmd, vec![owner_ls.clone()]);
        assert!(ask.allow_always_offered(&store));
        let saved = allow_always_rules(&store, &ask).unwrap();
        assert_eq!(prefixes(&saved), ["cd src", "git push"], "only the commands that needed the answer");
        let mut grant = ask.seat.grant.clone();
        grant.rules.extend(saved);
        assert!(RuleSet::of(&grant).owner_allowed(&ask.target), "the same command runs without asking");

        // An ask rule the command met is the rule the allow replaces.
        let ask_rule = command_rule(Effect::Ask, "git push");
        let ask = shell_ask(AskCase::AskRule { rule_id: ask_rule.id.clone() }, "ls && git push origin", vec![ask_rule.clone()]);
        let saved = allow_always_rules(&store, &ask).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!((&saved[0].key, &saved[0].field), (&ask_rule.key, &ask_rule.field));

        // At most five, the leftmost.
        let many = "a1 x; a2 x; a3 x; a4 x; a5 x; a6 x; a7 x";
        let saved = allow_always_rules(&store, &shell_ask(AskCase::AskMode, many, vec![])).unwrap();
        assert_eq!(prefixes(&saved), ["a1 x", "a2 x", "a3 x", "a4 x", "a5 x"]);
    }

    /// A command that can't be read has no rule to save: the card offers
    /// "This once" only: no saved rule could be sure to cover it.
    #[test]
    fn an_unreadable_command_offers_no_allow_always() {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(&dir.path().join("u.db").to_string_lossy()).unwrap();
        for cmd in ["ls && $CMD -rf x", "git $SUB origin", "ls &&", "FOO=1 make"] {
            let ask = shell_ask(AskCase::AskMode, cmd, vec![]);
            assert!(allow_always_rules(&store, &ask).is_none(), "{cmd:?}");
            assert!(!ask.allow_always_offered(&store), "{cmd:?}");
        }
        // Asked because a deny could name it: the deny is never loosened.
        let deny = command_rule(Effect::Deny, "rm");
        let ask = shell_ask(AskCase::AskRule { rule_id: deny.id.clone() }, "$CMD -rf x", vec![deny]);
        assert!(!ask.allow_always_offered(&store));
    }

    /// An ask belongs to the chat whose own flow raised it, read from the
    /// thread's key, never guessed. Asks from a schedule, a heartbeat, a
    /// workflow, a coworker's request or a helper have no chat: they live in
    /// the Inbox (live 2026-10-01: a scheduled run's 18 asks filled a chat).
    #[tokio::test]
    async fn an_ask_names_only_the_chat_that_raised_it() {
        let r = rig().await;
        let chat_of = |id: &str| r.asks.get(id).unwrap().unwrap().chat_id;
        let here = r.park("agent:emp:thread:c-1", Door::Chat, "+15550142").await;
        assert_eq!(chat_of(&here).as_deref(), Some("c-1"));
        let spoken = r.park("agent:emp:thread:c-2", Door::Voice, "+15550142").await;
        assert_eq!(chat_of(&spoken).as_deref(), Some("c-2"));
        for (key, door) in [
            (KEY, Door::Heartbeat),
            ("agent:emp:cron:morning", Door::Schedule),
            ("workflow:wf-1", Door::Workflow),
            ("agent:emp:thread:c-1", Door::Coworker { from: "gm".into() }),
            ("subagent:agent:emp:thread:c-1:h-1", Door::Helper),
            ("subagent:agent:emp:thread:c-1:h-2", Door::Chat),
            ("agent:emp:web", Door::Chat),
        ] {
            let id = r.park(key, door.clone(), "+15550177").await;
            assert_eq!(chat_of(&id), None, "{key} via {door:?}");
        }
    }

    #[tokio::test]
    async fn this_once_runs_once() {
        let r = rig().await;
        let id = r.park(KEY, Door::Chat, "+15550142").await;
        r.answer(&id, Answer::ThisOnce, AnsweredVia::Chat).await.unwrap();
        assert_eq!(r.ran(1), 1);
        assert!(r.store.permission_rules("emp").unwrap().is_empty(), "nothing standing");
        let next = r.park(KEY, Door::Chat, "+15550142").await;
        assert_ne!(next, id, "the next time asks again");
        assert_eq!(r.ran(1), 1);
    }

    #[tokio::test]
    async fn no_is_never_retried_in_the_session() {
        let r = rig().await;
        let id = r.park(KEY, Door::Chat, "+15550142").await;
        r.answer(&id, Answer::No, AnsweredVia::Chat).await.unwrap();
        assert_eq!(r.ran(1), 0);
        let told = r.seen.notes();
        assert_eq!(told.len(), 1);
        assert_eq!(told[0].0, KEY);
        assert!(told[0].1.contains(": declined\nIt did not run. Don't ask again"), "{}", told[0].1);
        // The same call in the same session: refused without a card.
        let again = r.reg.execute(&ctx(KEY, Door::Chat), "text", text("+15550142")).await;
        assert!(again.is_error && again.parked_ask.is_none(), "{}", again.content);
        assert!(again.content.starts_with("The owner already said no to this (text +15550142)"), "{}", again.content);
        assert_eq!((r.seen.cards(), r.ran(1)), (1, 0));
        // Another recipient, or another session, is a new question.
        r.park(KEY, Door::Chat, "+15550177").await;
        r.park("agent:emp:web", Door::Chat, "+15550142").await;
        assert_eq!(r.seen.cards(), 3);
    }

    /// The answer reaches the session that parked, as a notification with
    /// the call's result; the wake rail hears it next step or starts a turn.
    #[tokio::test]
    async fn the_answer_reaches_the_parked_session_as_a_notification() {
        let r = rig().await;
        let id = r.park(KEY, Door::Heartbeat, "+15550142").await;
        r.answer(&id, Answer::ThisOnce, AnsweredVia::Mobile).await.unwrap();
        let told = r.seen.notes();
        assert_eq!(told.len(), 1);
        assert_eq!(told[0].0, KEY);
        assert!(told[0].1.starts_with("<system-reminder>\n[Notification: not a message from the owner]"));
        assert!(told[0].1.contains(&format!("ask {id} \"text +15550142\": allowed, this once\nIt ran:\nDONE")), "{}", told[0].1);
    }

    /// A helper's ask parks only its step and resumes under the helper's
    /// own seat: the parent's ceiling still holds. The parent supervises in
    /// Ask mode, so a change the job covers still asks.
    #[tokio::test]
    async fn helper_ask_parks_and_resumes() {
        let r = rig().await;
        let mut parent = Grant::new("emp", Mode::Ask);
        parent.rules = vec![Rule {
            id: "sms".into(),
            scope: Scope::Employee("emp".into()),
            key: RuleKey::Capability("sms".into()),
            field: None,
            effect: Effect::Allow,
            money: None,
            source: RuleSource::Owner,
            locked: false,
            created_at: 0,
        }];
        let mut helper = parent.clone();
        helper.ceiling = Some(Ceiling::Parent { grant: Box::new(parent) });
        let key = "subagent:agent:emp:web:h-1";
        let mut c = ctx(key, Door::Helper);
        c.grant = Some(Arc::new(helper.clone()));
        let parked = r.reg.execute(&c, "text", text("+15550142")).await;
        let id = parked.parked_ask.expect("the helper's step parks");
        let ask = r.asks.get(&id).unwrap().unwrap();
        assert_eq!((ask.door.clone(), ask.seat.grant.clone()), (Door::Helper, helper));
        r.answer(&id, Answer::ThisOnce, AnsweredVia::Mobile).await.unwrap();
        assert_eq!(r.ran(1), 1);
        assert_eq!(r.seen.notes()[0].0, key, "the helper hears it");
    }

    /// A workflow step parks on the same ask: the answer releases the run,
    /// which resumes at the call itself.
    #[tokio::test]
    async fn workflow_activity_parks_on_the_same_ask() {
        let r = rig().await;
        let id = r.park("workflow:wf-1", Door::Workflow, "+15550142").await;
        r.store.link_permission_ask_run(&id, "run-1").unwrap();
        r.asks.answer(&id, Answer::ThisOnce, AnsweredVia::Inbox).unwrap();
        let resumed = r.asks.resume(&r.reg, &id, chrono::Utc::now().timestamp()).unwrap();
        assert!(resumed.is_none(), "the run resumes the call, not the answer");
        assert_eq!(*r.seen.released.lock().unwrap(), vec![("run-1".to_string(), true)]);
        assert!(r.seen.notes().is_empty());
        assert_eq!(r.ran(1), 0);
        let no = r.park("workflow:wf-2", Door::Workflow, "+15550177").await;
        r.store.link_permission_ask_run(&no, "run-2").unwrap();
        r.answer(&no, Answer::No, AnsweredVia::Inbox).await.unwrap();
        assert_eq!(r.seen.released.lock().unwrap()[1], ("run-2".to_string(), false));
    }

    /// An employee made by an employee asks for more than its creator
    /// holds: one card, no call to run; Allow grants the extras for good.
    #[tokio::test]
    async fn extras_card_is_the_one_card_and_its_answer_grants_the_job() {
        let r = rig().await;
        let creator = Grant::new("office", Mode::Automatic);
        let id = r
            .asks
            .raise_extras(&creator, "researcher", vec!["web".into(), "mail".into()], "Lead Researcher will search the web and send email".into(), "agent:office:web")
            .unwrap();
        assert_eq!(r.seen.cards(), 1, "the one card");
        let ask = r.asks.get(&id).unwrap().expect("a readable ask");
        assert!(ask.allow_always_offered(&r.store) && !ask.this_once_offered());
        assert_eq!(ask.reason(), "It was made by another employee and needs more than that employee has.");
        r.asks.answer(&id, Answer::AllowAlways, AnsweredVia::Mobile).unwrap();
        let resumed = r.asks.resume(&r.reg, &id, chrono::Utc::now().timestamp()).unwrap();
        assert!(resumed.is_none(), "nothing to run");
        let job: Vec<_> = r.store.permission_rules("researcher").unwrap().into_iter().map(|x| x.key).collect();
        assert!(job.contains(&RuleKey::Capability("web".into())) && job.contains(&RuleKey::Capability("mail".into())), "{job:?}");
        assert!(r.seen.notes()[0].1.contains("Granted: web, mail."), "{}", r.seen.notes()[0].1);
        // No grants nothing.
        let no = r.asks.raise_extras(&creator, "scribe", vec!["web".into()], "Scribe will search the web".into(), "agent:office:web").unwrap();
        r.answer(&no, Answer::No, AnsweredVia::Chat).await.unwrap();
        assert!(r.store.permission_rules("scribe").unwrap().iter().all(|x| x.scope != Scope::Employee("scribe".into())));
    }

    /// A "never" rule refuses shell in Full Access, and the refusal offers
    /// the way back instead of "stop": the owner says yes, the employee
    /// calls request_permission, which is the Widens ask on the owner's one
    /// card; his answer (here, out loud) lifts the rule, and shell runs. A
    /// No, or a rule a law fixed, leaves it off.
    #[tokio::test]
    async fn a_refused_permission_comes_back_on_the_owners_card() {
        use types::permissions::{Effect, Rule, RuleKey, RuleSource, Scope, Writer};
        let r = rig().await;
        let shell_ran = Arc::new(AtomicUsize::new(0));
        r.reg.register(Box::new(Probe { name: "run_command", capability: Some("shell"), ran: shell_ran.clone() })).await;
        r.reg.register(Box::new(tools::permission_request_tool::RequestPermissionTool::new(r.store.clone()))).await;
        r.store.set_permission_mode(&Scope::Company, Mode::FullAccess).unwrap();
        let deny = |cap: &str, locked: bool| Rule {
            id: uuid::Uuid::new_v4().to_string(),
            scope: Scope::Company,
            key: RuleKey::Capability(cap.into()),
            field: None,
            effect: Effect::Deny,
            money: None,
            source: if locked { RuleSource::Law { pack: "company".into() } } else { RuleSource::Owner },
            locked,
            created_at: 0,
        };
        r.store.write_permission_rule(&deny("shell", false), &Writer::Owner).unwrap();
        let seat_in = |key: &str| {
            let mut c = ctx(key, Door::Chat);
            c.grant = Some(Arc::new(crate::harness::permissions::resolve_grant(&r.store, "emp", None)));
            c
        };
        let seat = || seat_in(KEY);

        let refused = r.reg.execute(&seat(), "run_command", json!({ "command": "shopify version" })).await;
        assert!(refused.is_error && refused.parked_ask.is_none(), "{}", refused.content);
        assert!(refused.content.contains("request_permission(permission: \"shell\")"), "{}", refused.content);
        assert!(!refused.content.contains("then stop"), "{}", refused.content);
        assert_eq!(shell_ran.load(Ordering::SeqCst), 0);

        // Called without the owner's card, it only asks.
        let request = json!({ "permission": "shell" });
        let parked = r.reg.execute(&seat(), "request_permission", request.clone()).await;
        let id = parked.parked_ask.clone().expect("the owner's card");
        let ask = r.asks.get(&id).unwrap().unwrap();
        assert_eq!(ask.case, AskCase::Widens);
        assert!(!ask.allow_always_offered(&r.store), "answered each time");
        assert!(ask.sentence.contains("Shell Commands"), "{}", ask.sentence);

        // A No leaves it off.
        r.answer(&id, Answer::No, AnsweredVia::Chat).await.unwrap();
        assert!(r.reg.execute(&seat(), "run_command", json!({ "command": "ls" })).await.is_error);

        // Later, in another conversation, a yes out loud: the rule is
        // lifted and shell runs.
        let again = r.reg.execute(&seat_in("agent:emp:web"), "request_permission", request).await;
        let id = again.parked_ask.clone().expect("asked again");
        r.answer(&id, Answer::ThisOnce, AnsweredVia::Voice).await.unwrap();
        assert!(
            r.store.all_permission_rules().unwrap().iter().all(|x| !(x.key == RuleKey::Capability("shell".into()) && x.effect == Effect::Deny)),
            "the never is gone"
        );
        assert!(r.store.permission_rules_in(&Scope::Employee("emp".into())).unwrap().iter().any(|x| x.key == RuleKey::Capability("shell".into()) && x.effect == Effect::Allow));
        let ran = r.reg.execute(&seat(), "run_command", json!({ "command": "shopify version" })).await;
        assert!(!ran.is_error, "{}", ran.content);
        assert_eq!(shell_ran.load(Ordering::SeqCst), 1);

        // A law's never is not lifted by a card.
        r.store.write_permission_rule(&deny("media", true), &Writer::Package { package: "company".into() }).unwrap();
        let fixed = r.reg.execute(&seat(), "request_permission", json!({ "permission": "media" })).await;
        r.answer(fixed.parked_ask.as_deref().expect("asked"), Answer::ThisOnce, AnsweredVia::Mobile).await.unwrap();
        assert!(r.store.all_permission_rules().unwrap().iter().any(|x| x.key == RuleKey::Capability("media".into()) && x.effect == Effect::Deny));
    }

    /// Giving an employee more room is answered each time: no "Allow
    /// always", and an answer the card didn't offer counts as "This once".
    #[tokio::test]
    async fn widening_is_answered_each_time() {
        let r = rig().await;
        let mut ask = r.asks.get(&r.park(KEY, Door::Chat, "+15550142").await).unwrap().unwrap();
        ask.case = AskCase::Widens;
        assert!(!ask.allow_always_offered(&r.store) && ask.this_once_offered());
        assert_eq!(ask.reason(), "Only you can give an employee more room.");
    }

    /// An ask nobody answers never expires and never counts as a No: each
    /// time its wait's timer wakes it, the card comes back to the owner and
    /// the ask waits again, a day, then two, then four days out. Four days
    /// on it is still open, nothing ran and the employee was told nothing;
    /// the answer then runs the call once.
    #[tokio::test]
    async fn an_unanswered_ask_stays_open_and_is_reminded_until_answered() {
        let r = rig().await;
        let id = r.park(KEY, Door::Heartbeat, "+15550142").await;
        let created = r.asks.get(&id).unwrap().unwrap().created_at;
        let deadline = |r: &Rig| {
            let run = r.store.engine_get_run(&id).unwrap().unwrap();
            r.store.engine_get_wait(run.current_wait_id.unwrap()).unwrap().unwrap().deadline.unwrap()
        };
        let day = 24 * 3600;
        assert_eq!(deadline(&r), created + day);
        for (at, next) in [(created + day, created + 3 * day), (created + 3 * day, created + 7 * day)] {
            assert!(r.asks.resume(&r.reg, &id, at).unwrap().is_none());
            assert_eq!(deadline(&r), next, "the next reminder");
        }
        assert_eq!(*r.seen.reminded.lock().unwrap(), vec![id.clone(), id.clone()], "reminded twice");
        let four_days_on = r.asks.get(&id).unwrap().unwrap();
        assert_eq!(four_days_on.status, AskStatus::Open, "four days on, still open");
        assert_eq!(r.asks.open(Some(KEY)).unwrap().len(), 1);
        assert_eq!(r.store.engine_get_run(&id).unwrap().unwrap().state, "waiting");
        assert_eq!((r.ran(1), r.seen.notes().len()), (0, 0), "nothing ran, nobody was told no");

        // The answer arrives: the call runs once and the employee hears it.
        r.answer(&id, Answer::ThisOnce, AnsweredVia::Mobile).await.unwrap();
        assert_eq!(r.ran(1), 1);
        assert!(r.seen.notes()[0].1.contains("allowed, this once\nIt ran:\nDONE"), "{}", r.seen.notes()[0].1);
        assert_eq!(r.store.engine_get_run(&id).unwrap().unwrap().state, "done");
        // A second wake of the closed run applies nothing twice.
        assert!(r.asks.resume(&r.reg, &id, created + 5 * day).unwrap().is_none());
        assert_eq!((r.ran(1), r.seen.notes().len(), r.seen.reminded.lock().unwrap().len()), (1, 1, 2));
    }

    /// Live 2026-10-01: the same first email to the same new customer was
    /// asked twice while the first ask still waited. Another message to the
    /// same new person from the same employee is not a second card: the
    /// model hears it is already waiting.
    #[tokio::test]
    async fn a_new_person_waiting_is_asked_about_once() {
        let r = rig().await;
        let c = ctx(KEY, Door::Chat);
        let first = r.reg.execute(&c, "text", json!({ "to": "+15550199", "body": "Your card expires" })).await;
        assert!(first.parked_ask.is_some(), "{}", first.content);
        assert!(matches!(
            r.asks.get(first.parked_ask.as_deref().unwrap()).unwrap().unwrap().case,
            AskCase::NewCounterparty { .. }
        ));
        let second = r.reg.execute(&c, "text", json!({ "to": "+15550199", "body": "A reminder" })).await;
        assert!(second.parked_ask.is_none() && second.content.starts_with("An earlier ask"), "{}", second.content);
        assert_eq!(r.seen.cards(), 1);
        assert_eq!(r.asks.open(None).unwrap().len(), 1);
        // Another new person is a question of its own.
        let other = r.reg.execute(&c, "text", json!({ "to": "+15550188", "body": "Your card expires" })).await;
        assert!(other.parked_ask.is_some(), "{}", other.content);
        assert_eq!(r.seen.cards(), 2);
    }

    /// An ask whose parked workflow run has ended is no longer needed the
    /// moment anything lists the open asks: withdrawn (not a No), its card
    /// cleared everywhere, nothing run, nothing released.
    #[tokio::test]
    async fn an_ask_whose_run_ended_leaves_the_open_list_at_once() {
        let r = rig().await;
        let id = r.park("workflow:wf-7", Door::Workflow, "+15550142").await;
        r.store
            .engine_create_run(&db::NewRun { id: "run-7", kind: "workflow", session_key: "workflow:wf-7", agent_id: "emp", lane: "main", ..Default::default() })
            .unwrap();
        r.store.link_permission_ask_run(&id, "run-7").unwrap();
        assert_eq!(r.asks.open(None).unwrap().len(), 1, "its run still waits on it");
        r.store.engine_set_run_state("run-7", "cancelled", 1, None).unwrap();
        assert!(r.asks.open(None).unwrap().is_empty(), "no longer needed");
        assert_eq!(r.asks.get(&id).unwrap().unwrap().status, AskStatus::Withdrawn);
        assert_eq!(*r.seen.resolved.lock().unwrap(), vec![(id.clone(), AskStatus::Withdrawn)]);
        assert!(r.seen.released.lock().unwrap().is_empty());
        assert_eq!(r.ran(1), 0);
        // Listed again: nothing more happens.
        assert!(r.asks.open(None).unwrap().is_empty());
        assert_eq!(r.seen.resolved.lock().unwrap().len(), 1);
    }

    /// An ask whose parked workflow run is gone is withdrawn at its next
    /// wake: the card clears, nothing runs, nothing is released, and it is
    /// not a No.
    #[tokio::test]
    async fn an_ask_whose_parked_run_ended_is_withdrawn_not_declined() {
        let r = rig().await;
        let id = r.park("workflow:wf-9", Door::Workflow, "+15550142").await;
        r.store
            .engine_create_run(&db::NewRun { id: "run-9", kind: "workflow", session_key: "workflow:wf-9", agent_id: "emp", lane: "main", ..Default::default() })
            .unwrap();
        r.store.link_permission_ask_run(&id, "run-9").unwrap();
        r.store.engine_set_run_state("run-9", "cancelled", 1, None).unwrap();
        assert!(r.asks.resume(&r.reg, &id, chrono::Utc::now().timestamp()).unwrap().is_none());
        let row = r.store.get_permission_ask(&id).unwrap().unwrap();
        assert_eq!((row.status.as_str(), row.answer.as_deref()), ("withdrawn", None));
        assert_eq!(r.asks.get(&id).unwrap().unwrap().status, AskStatus::Withdrawn);
        assert_eq!(*r.seen.resolved.lock().unwrap(), vec![(id.clone(), AskStatus::Withdrawn)]);
        assert!(r.seen.reminded.lock().unwrap().is_empty() && r.seen.released.lock().unwrap().is_empty());
        assert_eq!(r.store.engine_get_run(&id).unwrap().unwrap().state, "done");
        assert!(matches!(r.asks.answer(&id, Answer::ThisOnce, AnsweredVia::Inbox), Err(AskError::Settled(_))));
        assert_eq!(r.ran(1), 0);
    }

    /// The model always hears where the owner answers, by door: on his own
    /// call he is asked aloud and his spoken answer is the answer; in a chat
    /// the card is in the conversation; otherwise his Inbox. A stranger (a
    /// phone caller, an outside message) is never asked to approve. No text
    /// names a desktop app or support (live 2026-09-28, both invented).
    #[test]
    fn the_parked_text_says_where_the_owner_answers() {
        let case = AskCase::NewCounterparty { who: "pat@example.com".into() };
        let text = |door: Door, origin: Origin| parked_text("sending an email to pat@example.com", &case, &door, origin);
        let call = text(Door::Voice, Origin::User);
        assert!(call.starts_with("Waiting for the owner to allow: sending an email to pat@example.com. "), "{call}");
        assert!(call.contains("ask them now, in one short spoken question"), "{call}");
        assert!(call.contains("Their spoken yes or no on this call is the answer"), "{call}");
        let stranger = text(Door::Voice, Origin::Caller);
        assert!(stranger.contains("the person you are talking with can't"), "{stranger}");
        assert!(!stranger.contains("spoken"), "a caller is never asked: {stranger}");
        assert!(text(Door::Chat, Origin::Comm).contains("the person you are talking with can't"));
        let chat = text(Door::Chat, Origin::User);
        assert!(chat.contains("on the card in this conversation or in their Inbox"), "{chat}");
        let unattended = text(Door::Heartbeat, Origin::System);
        assert!(unattended.contains("in their Inbox, in the Nebo app on any of their devices"), "{unattended}");
        for door in [Door::Voice, Door::Chat, Door::Heartbeat, Door::Workflow, Door::Helper] {
            for origin in [Origin::User, Origin::System, Origin::Caller] {
                let t = text(door.clone(), origin).to_lowercase();
                assert!(!t.contains("desktop") && !t.contains("support"), "{t}");
                assert!(t.contains("don't retry this action"), "{t}");
            }
        }
    }

    #[test]
    fn a_spoken_answer_is_recorded_as_voice() {
        assert_eq!(AnsweredVia::parse(AnsweredVia::Voice.as_str()), Some(AnsweredVia::Voice));
    }

    /// A button on the phone's notification (the lock screen, or the Watch
    /// it mirrors to) is a door of its own: the answer is taken, runs the
    /// step, and is kept as answered from the notification.
    #[tokio::test]
    async fn an_answer_from_the_notification_is_kept_as_one() {
        let r = rig().await;
        let id = r.park(KEY, Door::Chat, "+15550142").await;
        let via = AnsweredVia::parse("notification").expect("notification is a door");
        r.answer(&id, Answer::ThisOnce, via).await.unwrap();
        assert_eq!(r.ran(1), 1, "the step ran once");
        let kept = r.asks.get(&id).unwrap().expect("the ask");
        assert_eq!(
            kept.status,
            AskStatus::Answered { answer: Answer::ThisOnce, via: Some(AnsweredVia::Notification) }
        );
    }

    /// Answered while a reminder was going out: the answer's signal found no
    /// wait to wake, and the reminder's resume applies it, once.
    #[tokio::test]
    async fn an_answer_that_lands_during_a_reminder_is_applied_once() {
        let r = rig().await;
        let id = r.park(KEY, Door::Heartbeat, "+15550142").await;
        // The reminder timer woke the run: it is queued, not waiting.
        let wait = r.store.engine_get_run(&id).unwrap().unwrap().current_wait_id.unwrap();
        r.store.engine_resume_from_wait(wait, 0, 1).unwrap();
        r.asks.answer(&id, Answer::ThisOnce, AnsweredVia::Chat).unwrap();
        if let Some(h) = r.asks.resume(&r.reg, &id, chrono::Utc::now().timestamp()).unwrap() {
            h.await.unwrap();
        }
        assert_eq!(r.ran(1), 1, "applied");
        assert!(r.seen.reminded.lock().unwrap().is_empty(), "an answered ask is not a reminder");
        assert!(r.asks.resume(&r.reg, &id, chrono::Utc::now().timestamp()).unwrap().is_none());
        assert_eq!(r.ran(1), 1, "once");
    }

    /// `run_command` as the shell tool resolves it (its rule field, effects
    /// and activity), counting the commands instead of running them: no
    /// screen is captured and no app is driven here.
    struct ShellSpec {
        spec: tools::command_tools::RunCommandTool,
        ran: Arc<Mutex<Vec<String>>>,
    }

    impl DynTool for ShellSpec {
        fn name(&self) -> &str {
            "run_command"
        }
        fn description(&self) -> String {
            self.spec.description()
        }
        fn schema(&self) -> serde_json::Value {
            self.spec.schema()
        }
        fn read_only(&self, input: &serde_json::Value) -> bool {
            self.spec.read_only(input)
        }
        fn rule_field(&self, input: &serde_json::Value) -> Option<RuleField> {
            self.spec.rule_field(input)
        }
        fn capability(&self, input: &serde_json::Value) -> Option<&'static str> {
            self.spec.capability(input)
        }
        fn effects(&self, input: &serde_json::Value) -> types::permissions::CallEffects {
            self.spec.effects(input)
        }
        fn activity(&self, input: &serde_json::Value) -> String {
            self.spec.activity(input)
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a ToolContext,
            input: serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
            self.ran.lock().unwrap().push(input["command"].as_str().unwrap_or("").to_string());
            Box::pin(async { ToolResult::ok("RAN") })
        }
    }

    impl Rig {
        /// The shell, as `run_command` resolves its calls; what it ran.
        async fn shell(&self) -> Arc<Mutex<Vec<String>>> {
            let ran = Arc::new(Mutex::new(Vec::new()));
            let machine = Arc::new(tools::file_tools::Machine::new(Arc::new(tools::ProcessRegistry::new()), None));
            self.reg.register(Box::new(ShellSpec { spec: tools::command_tools::RunCommandTool(machine), ran: ran.clone() })).await;
            ran
        }

        fn mode(&self, agent: &str, mode: Mode) {
            self.store.set_permission_mode(&Scope::Employee(agent.into()), mode).unwrap();
        }
    }

    fn command(c: &str) -> serde_json::Value {
        json!({ "command": c, "description": "Look at the page" })
    }

    /// 2026-10-04: an employee with no recording tool ran `screencapture -x`
    /// 90 times on the owner's display, with nothing asking. Capturing his
    /// screen asks in every mode, Full Access included, in Nebo's own words
    /// whatever the description says. "Allow always" keeps it for that
    /// employee, so the next capture runs unasked, a scheduled one too;
    /// another employee is still asked.
    #[tokio::test]
    async fn capturing_the_screen_asks_and_allow_always_keeps_it_for_the_employee() {
        let r = rig().await;
        let ran = r.shell().await;
        for (agent, mode) in [("auto", Mode::Automatic), ("asker", Mode::Ask), ("emp", Mode::FullAccess)] {
            r.mode(agent, mode);
            let parked = r.reg.execute(&ctx(&format!("agent:{agent}:web"), Door::Chat), "run_command", command("screencapture -x /tmp/a.png")).await;
            let id = parked.parked_ask.clone().unwrap_or_else(|| panic!("{mode:?}: {}", parked.content));
            assert!(parked.content.starts_with("Waiting for the owner to allow: capturing your screen."), "{}", parked.content);
            let ask = r.asks.get(&id).unwrap().unwrap();
            assert_eq!(ask.case, AskCase::ReachesOwner { reach: OwnerReach::Screen });
            assert_eq!((ask.sentence.as_str(), ask.reason()), ("capturing your screen", "It would see what's on your screen."));
            assert!(ask.allow_always_offered(&r.store) && ask.this_once_offered(), "{mode:?}: all three answers");
        }
        assert!(ran.lock().unwrap().is_empty(), "nothing ran before the owner answered");

        let id = r.asks.open(None).unwrap().into_iter().find(|a| a.agent_id == "emp").unwrap().id;
        r.answer(&id, Answer::AllowAlways, AnsweredVia::Chat).await.unwrap();
        assert_eq!(*ran.lock().unwrap(), vec!["screencapture -x /tmp/a.png"]);
        let saved: Vec<Rule> = r.store.permission_rules("emp").unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            (&saved[0].scope, &saved[0].key, &saved[0].field, saved[0].effect),
            (
                &Scope::Employee("emp".into()),
                &RuleKey::Tool("run_command".into()),
                &Some(RuleField::CommandPrefix("screencapture".into())),
                Effect::Allow
            )
        );
        assert_eq!(saved[0].source, RuleSource::AllowAlways { ask_id: id });

        // The next capture, another file, runs unasked; so does a scheduled
        // one, which can't wait for anyone.
        let next = r.reg.execute(&ctx("agent:emp:web", Door::Chat), "run_command", command("screencapture -x /tmp/b.png")).await;
        assert_eq!((next.content.as_str(), next.parked_ask.is_none()), ("RAN", true));
        let mut scheduled = ctx("agent:emp:cron:shots", Door::Schedule);
        scheduled.cannot_wait = true;
        let cron = r.reg.execute(&scheduled, "run_command", command("screencapture -x /tmp/c.png")).await;
        assert_eq!(cron.content, "RAN");
        assert_eq!(ran.lock().unwrap().len(), 3);
        // Another employee is still asked.
        r.mode("other", Mode::FullAccess);
        let other = r.reg.execute(&ctx("agent:other:web", Door::Chat), "run_command", command("screencapture -x /tmp/d.png")).await;
        assert!(other.parked_ask.is_some(), "{}", other.content);
    }

    /// No: it doesn't run, the employee is told plainly, and the same call
    /// isn't asked again. A run that can't wait for the owner is refused.
    #[tokio::test]
    async fn no_blocks_capturing_the_screen_with_a_plain_message() {
        let r = rig().await;
        let ran = r.shell().await;
        r.mode("emp", Mode::FullAccess);
        let shot = command("screencapture -x /tmp/a.png");
        let parked = r.reg.execute(&ctx(KEY, Door::Chat), "run_command", shot.clone()).await;
        r.answer(&parked.parked_ask.unwrap(), Answer::No, AnsweredVia::Chat).await.unwrap();
        assert!(ran.lock().unwrap().is_empty());
        let told = r.seen.notes();
        assert!(told[0].1.contains("\"capturing your screen\": declined\nIt did not run."), "{}", told[0].1);
        let again = r.reg.execute(&ctx(KEY, Door::Chat), "run_command", shot.clone()).await;
        assert!(again.is_error && again.parked_ask.is_none(), "{}", again.content);
        assert!(again.content.starts_with("The owner already said no to this (capturing your screen)"), "{}", again.content);
        let mut scheduled = ctx("agent:emp:cron:shots", Door::Schedule);
        scheduled.cannot_wait = true;
        let cron = r.reg.execute(&scheduled, "run_command", shot).await;
        assert_eq!(
            cron.content,
            "Didn't run: capturing your screen. It needs the owner's OK, and a scheduled command can't wait for one. \
             It would see what's on your screen."
        );
        assert_eq!((ran.lock().unwrap().len(), r.seen.cards()), (0, 1));
    }

    /// Driving another app asks, named; a command that only mentions one
    /// of these programs, or opens a file in its own app, doesn't.
    #[tokio::test]
    async fn driving_an_app_asks_and_ordinary_commands_do_not() {
        let r = rig().await;
        let ran = r.shell().await;
        r.mode("emp", Mode::FullAccess);
        for c in [
            r#"osascript -e 'tell application "Safari" to do JavaScript "location.href = 1" in document 1'"#,
            "open -a Safari https://example.com",
        ] {
            let parked = r.reg.execute(&ctx(KEY, Door::Chat), "run_command", command(c)).await;
            let id = parked.parked_ask.clone().unwrap_or_else(|| panic!("{c}: {}", parked.content));
            let ask = r.asks.get(&id).unwrap().unwrap();
            assert_eq!((ask.sentence.as_str(), ask.reason()), ("controlling Safari", "It would act in your apps as you."), "{c}");
        }
        for c in ["grep screencapture notes.txt", "echo osascript", "open file.pdf"] {
            let t = r.reg.target("run_command", &command(c)).await.unwrap();
            assert_eq!(t.effects.reaches_owner, None, "{c}");
            let r2 = r.reg.execute(&ctx(KEY, Door::Chat), "run_command", command(c)).await;
            assert_eq!((r2.content.as_str(), r2.parked_ask.is_none()), ("RAN", true), "{c}");
        }
        assert_eq!(*ran.lock().unwrap(), vec!["grep screencapture notes.txt", "echo osascript", "open file.pdf"]);
        assert_eq!(r.seen.cards(), 2);
    }

    /// The owner's own apps as Nebo's tools reach them (`os` on his mail,
    /// calendar, reminders and contacts; `message` on his Messages), counting
    /// the calls instead of making them: nothing of his is read here.
    struct AppSpec {
        spec: Box<dyn DynTool>,
        ran: Arc<Mutex<Vec<String>>>,
    }

    impl DynTool for AppSpec {
        fn name(&self) -> &str {
            self.spec.name()
        }
        fn description(&self) -> String {
            self.spec.description()
        }
        fn schema(&self) -> serde_json::Value {
            self.spec.schema()
        }
        fn read_only(&self, input: &serde_json::Value) -> bool {
            self.spec.read_only(input)
        }
        fn rule_key(&self, input: &serde_json::Value) -> String {
            self.spec.rule_key(input)
        }
        fn rule_field(&self, input: &serde_json::Value) -> Option<RuleField> {
            self.spec.rule_field(input)
        }
        fn capability(&self, input: &serde_json::Value) -> Option<&'static str> {
            self.spec.capability(input)
        }
        fn effects(&self, input: &serde_json::Value) -> types::permissions::CallEffects {
            self.spec.effects(input)
        }
        fn activity(&self, input: &serde_json::Value) -> String {
            self.spec.activity(input)
        }
        fn validates_input(&self) -> bool {
            self.spec.validates_input()
        }
        fn normalize_input(&self, input: serde_json::Value) -> serde_json::Value {
            self.spec.normalize_input(input)
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a ToolContext,
            input: serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
            self.ran.lock().unwrap().push(format!("{} {}", self.spec.name(), self.spec.rule_key(&input)));
            Box::pin(async { ToolResult::ok("RAN") })
        }
    }

    impl Rig {
        async fn owner_apps(&self) -> Arc<Mutex<Vec<String>>> {
            let ran = Arc::new(Mutex::new(Vec::new()));
            self.reg.register(Box::new(AppSpec { spec: Box::new(tools::OsTool::new()), ran: ran.clone() })).await;
            self.reg
                .register(Box::new(AppSpec { spec: Box::new(tools::MessageTool::new(self.store.clone())), ran: ran.clone() }))
                .await;
            ran
        }
    }

    fn workflow_ctx(agent: &str) -> ToolContext {
        let mut c = ToolContext::new(Origin::Workflow).with_session(format!("agent:{agent}:workflow:run-1:run::0"), "s1");
        c.door = Door::Workflow;
        c
    }

    /// Bake-off 2026-10-10: a test employee's unattended workflow read the
    /// owner's Calendar through `os` and tried Mail and Messages, with
    /// nothing asking. Each of his apps now waits for his OK, in every
    /// mode, Full Access included, a workflow step too; "Allow always" grants
    /// that one call to that employee, and another employee is still asked.
    #[tokio::test]
    async fn the_owners_apps_wait_for_his_ok_and_allow_always_grants_that_employee() {
        let r = rig().await;
        let ran = r.owner_apps().await;
        let calls = [
            ("os", json!({ "resource": "calendar", "action": "today" }), "Calendar"),
            ("os", json!({ "resource": "mail", "action": "unread" }), "Mail"),
            ("os", json!({ "resource": "reminders", "action": "list" }), "Reminders"),
            ("os", json!({ "resource": "contacts", "action": "search", "query": "Ann" }), "Contacts"),
            ("message", json!({ "resource": "sms", "action": "read", "phone": "+15550142" }), "Messages"),
            ("message", json!({ "resource": "sms", "action": "send", "phone": "+15550142", "text": "hi" }), "Messages"),
        ];
        for agent in ["emp", "other"] {
            r.mode(agent, Mode::FullAccess);
        }
        for (tool, input, app) in &calls {
            let t = r.reg.target(tool, input).await.unwrap();
            assert_eq!(t.effects.reaches_owner, Some(OwnerReach::App { app: Some(app.to_string()) }), "{tool} {input}");
            for c in [workflow_ctx("emp"), ctx("agent:emp:web", Door::Chat)] {
                let parked = r.reg.execute(&c, tool, input.clone()).await;
                let id = parked.parked_ask.clone().unwrap_or_else(|| panic!("{tool} {input}: {}", parked.content));
                let ask = r.asks.get(&id).unwrap().unwrap();
                assert!(matches!(ask.case, AskCase::ReachesOwner { .. }), "{tool} {input}");
                assert!(ask.allow_always_offered(&r.store), "{tool} {input}: Allow always is offered");
            }
        }
        assert!(ran.lock().unwrap().is_empty(), "nothing of his was read or sent");

        // Allow always on the calendar read: that call, for that employee.
        let calendar = r.reg.execute(&workflow_ctx("emp"), "os", calls[0].1.clone()).await;
        r.answer(&calendar.parked_ask.unwrap(), Answer::AllowAlways, AnsweredVia::Chat).await.unwrap();
        let saved: Vec<Rule> = r.store.permission_rules("emp").unwrap();
        assert!(
            saved.iter().any(|x| x.key == RuleKey::Tool("calendar_event_list".into())
                && x.field.is_none()
                && x.effect == Effect::Allow
                && x.scope == Scope::Employee("emp".into())),
            "{saved:?}"
        );
        let again = r.reg.execute(&workflow_ctx("emp"), "os", calls[0].1.clone()).await;
        assert_eq!((again.content.as_str(), again.parked_ask.is_none()), ("RAN", true));
        // Not his mail, and not another employee's calendar.
        assert!(r.reg.execute(&workflow_ctx("emp"), "os", calls[1].1.clone()).await.parked_ask.is_some());
        assert!(r.reg.execute(&workflow_ctx("other"), "os", calls[0].1.clone()).await.parked_ask.is_some());
    }

    /// What isn't his app doesn't ask: a file search, listing what runs,
    /// and a text from the employee's own texting line.
    #[tokio::test]
    async fn calls_that_are_not_his_apps_do_not_reach_him() {
        let r = rig().await;
        let _ran = r.owner_apps().await;
        for (tool, input) in [
            ("os", json!({ "resource": "search", "action": "search", "query": "invoice" })),
            ("os", json!({ "resource": "app", "action": "list" })),
            ("message", json!({ "resource": "sms", "action": "send", "phone": "+15550142", "text": "hi", "from": "+15550100" })),
        ] {
            let t = r.reg.target(tool, &input).await.unwrap();
            assert_eq!(t.effects.reaches_owner, None, "{tool} {input}");
        }
    }
}
