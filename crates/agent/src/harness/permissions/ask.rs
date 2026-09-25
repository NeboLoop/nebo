//! The ask: a call parked on the owner (Technical Design §2.12.5).
//!
//! Only that step waits. The ask is written and the model hears at once
//! that the call is waiting, so it carries on with everything else. One
//! card goes out everywhere (the Inbox, the phone, the open chat), and the
//! first answer anywhere wins. The answer re-runs the stored call with the
//! stored seat through the one check and reaches the employee as a
//! notification; a workflow step parked on the ask is released instead.
//! An ask nobody answers expires as a No. Nothing here ever approves on
//! its own.

use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use tools::ResolvedCall;
use types::permissions::{AskCase, Door, Effect, Grant, MoneyLimit, Rule, RuleField, RuleKey, RuleSource, Scope, Target, Writer};

use super::{CheckCx, RuleSet};
use crate::harness::delegation::notify::{AskOutcome, render_ask_outcome};

/// How long an ask waits for the owner before it expires as a No.
pub const EXPIRES_AFTER_SECS: i64 = 72 * 3600;

/// The owner's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    AllowAlways,
    ThisOnce,
    No,
}

impl Answer {
    pub fn as_str(self) -> &'static str {
        match self {
            Answer::AllowAlways => "allow_always",
            Answer::ThisOnce => "this_once",
            Answer::No => "no",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow_always" => Some(Answer::AllowAlways),
            "this_once" => Some(Answer::ThisOnce),
            "no" => Some(Answer::No),
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
}

impl AnsweredVia {
    pub fn as_str(self) -> &'static str {
        match self {
            AnsweredVia::Chat => "chat",
            AnsweredVia::Inbox => "inbox",
            AnsweredVia::Mobile => "mobile",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "chat" => Some(AnsweredVia::Chat),
            "inbox" => Some(AnsweredVia::Inbox),
            "mobile" => Some(AnsweredVia::Mobile),
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
    /// Unanswered past its time: a No.
    Expired,
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
    pub created_at: i64,
    pub expires_at: i64,
}

impl Ask {
    fn from_row(row: db::PermissionAskRow) -> Option<Ask> {
        let status = match row.status.as_str() {
            "open" => AskStatus::Open,
            "expired" => AskStatus::Expired,
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
            created_at: row.created_at,
            expires_at: row.expires_at,
        })
    }

    /// Whether "Allow always" can be offered: a locked must-ask can't be
    /// loosened by an answer.
    pub fn allow_always_offered(&self, store: &db::Store) -> bool {
        match &self.case {
            AskCase::AskRule { rule_id } => {
                !store.get_permission_rule(rule_id).ok().flatten().is_some_and(|r| r.locked)
            }
            _ => true,
        }
    }

    /// Why it asked, in plain words for the card.
    pub fn reason(&self) -> &'static str {
        match &self.case {
            AskCase::Money { .. } => "It's over this employee's money limit.",
            AskCase::NewCounterparty { .. } => "It's the first time it would contact them.",
            AskCase::Irreversible { .. } => "It can't be undone.",
            AskCase::OutsideJob { .. } => "It's outside this employee's job.",
            AskCase::UntrustedInput { .. } => "It acts on something that came from outside.",
            AskCase::AskRule { .. } => "This needs your OK every time.",
            AskCase::AskMode => "This employee asks before it changes anything.",
        }
    }
}

/// Where the card goes and where answers are delivered. The server
/// implements it: the Inbox and the phone, the open chat, the wake rail,
/// the workflow engine.
pub trait AskSurfaces: Send + Sync {
    /// Show the card everywhere the owner might answer it.
    fn card(&self, ask: &Ask);
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
    /// Someone already answered it, somewhere else, or it expired.
    #[error("this was already answered")]
    Settled(Box<Ask>),
    #[error("{0}")]
    Store(String),
}

/// A settled ask and, when the call runs, the task running it.
pub struct Settled {
    pub ask: Ask,
    pub resumed: Option<tokio::task::JoinHandle<()>>,
}

/// The asks: parking, the card, answers and expiry.
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
            created_at: now,
            expires_at: now + EXPIRES_AFTER_SECS,
        };
        let row = db::PermissionAskRow {
            id: ask.id.clone(),
            agent_id: ask.agent_id.clone(),
            session_key: ask.session_key.clone(),
            chat_id: None,
            door: json(&ask.door),
            ask_case: json(&ask.case),
            sentence: ask.sentence.clone(),
            target: json(&ask.target),
            call: json(&ask.call),
            seat: json(&ask.seat),
            status: "open".to_string(),
            created_at: ask.created_at,
            expires_at: ask.expires_at,
            ..Default::default()
        };
        match cx.store.insert_permission_ask(&row) {
            Ok(()) => {
                if let Some(s) = self.surfaces() {
                    s.card(&ask);
                }
            }
            Err(e) => tracing::warn!(tool = %call.name(), error = %e, "ask not written"),
        }
        ask.id
    }

    /// The ask the owner said no to (or let expire) for this same call in
    /// this session, if any: it is refused without a card.
    pub(super) fn declined_before(&self, session_key: &str, t: &Target, input: &serde_json::Value) -> Option<String> {
        let rows = self.store.declined_permission_asks(session_key).ok()?;
        rows.into_iter().filter_map(Ask::from_row).find_map(|a| {
            (a.target.key == t.key && a.target.field == t.field && a.call.input == *input).then_some(a.id)
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<Ask>, AskError> {
        let row = self.store.get_permission_ask(id).map_err(|e| AskError::Store(e.to_string()))?;
        Ok(row.and_then(Ask::from_row))
    }

    /// The asks waiting on the owner, oldest first; `session_key` narrows
    /// them to one session (the open chat).
    pub fn open(&self, session_key: Option<&str>) -> Result<Vec<Ask>, AskError> {
        let rows = self.store.open_permission_asks(session_key).map_err(|e| AskError::Store(e.to_string()))?;
        Ok(rows.into_iter().filter_map(Ask::from_row).collect())
    }

    /// The owner's answer. The first answer anywhere wins; a later one
    /// gets [`AskError::Settled`]. "Allow always" writes the rule the case
    /// names; "Allow always" and "This once" run the stored call through
    /// the one check; "No" tells the employee and never runs it.
    pub fn answer(
        &self,
        registry: &Arc<tools::Registry>,
        id: &str,
        answer: Answer,
        via: AnsweredVia,
    ) -> Result<Settled, AskError> {
        let mut ask = self.get(id)?.ok_or(AskError::NotFound)?;
        let now = chrono::Utc::now().timestamp();
        let won = self
            .store
            .settle_permission_ask(id, "answered", answer.as_str(), Some(via.as_str()), now)
            .map_err(|e| AskError::Store(e.to_string()))?;
        if !won {
            return Err(AskError::Settled(Box::new(self.get(id)?.ok_or(AskError::NotFound)?)));
        }
        ask.status = AskStatus::Answered { answer, via: Some(via) };
        let mut always = false;
        if answer == Answer::AllowAlways {
            match self.store.write_permission_rule(&allow_always_rule(&self.store, &ask), &Writer::Owner) {
                Ok(_) => always = true,
                // A locked must-ask can't be loosened: it runs this once.
                Err(e) => tracing::warn!(ask = %id, error = %e, "allow-always rule not written; runs once"),
            }
        }
        if let Some(s) = self.surfaces() {
            s.resolved(&ask);
        }
        let resumed = match answer {
            Answer::No => {
                self.settle_without_running(&ask, AskOutcome::Declined);
                None
            }
            Answer::AllowAlways | Answer::ThisOnce => self.run(registry, ask.clone(), always),
        };
        Ok(Settled { ask, resumed })
    }

    /// Expire every ask unanswered past its time as a No, and tell each
    /// employee. Never approves. Returns how many expired.
    pub fn expire_due(&self, now: i64) -> usize {
        let due = match self.store.due_permission_asks(now) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "expiry sweep: asks unreadable");
                return 0;
            }
        };
        let mut expired = 0;
        for row in due {
            let Some(mut ask) = Ask::from_row(row) else { continue };
            match self.store.settle_permission_ask(&ask.id, "expired", Answer::No.as_str(), None, now) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(e) => {
                    tracing::warn!(ask = %ask.id, error = %e, "expiry sweep: ask not settled");
                    continue;
                }
            }
            ask.status = AskStatus::Expired;
            if let Some(s) = self.surfaces() {
                s.resolved(&ask);
            }
            self.settle_without_running(&ask, AskOutcome::Expired { hours: EXPIRES_AFTER_SECS / 3600 });
            expired += 1;
        }
        expired
    }

    /// A No or an expiry: the parked workflow run ends refused, or the
    /// employee is told.
    fn settle_without_running(&self, ask: &Ask, outcome: AskOutcome<'_>) {
        let Some(s) = self.surfaces() else { return };
        match &ask.run_id {
            Some(run) => s.release_run(run, false),
            None => s.notify(&ask.session_key, &render_ask_outcome(&ask.id, &ask.sentence, &outcome)),
        }
    }

    /// Run the allowed call. A parked workflow run is released and resumes
    /// at the call itself; any other run gets the call's result as a
    /// notification.
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

/// The standing allow "Allow always" writes: the rule the ask's case names
/// (§2.12.4), for this employee.
pub fn allow_always_rule(store: &db::Store, ask: &Ask) -> Rule {
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
        AskCase::Irreversible { .. } | AskCase::AskMode => (call_key(), t.field.clone(), None),
    };
    Rule {
        id: String::new(),
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

/// What the model hears for a parked call.
pub fn parked_text(sentence: &str, case: &AskCase) -> String {
    let why = match case {
        AskCase::Money { .. } => " It is over this employee's money limit.",
        AskCase::OutsideJob { .. } => " It is outside this employee's job.",
        AskCase::UntrustedInput { .. } => " It acts on words that came from outside.",
        _ => "",
    };
    format!(
        "Waiting for the owner to allow: {sentence}.{why} Carry on with anything else; the answer \
         arrives as a notification. Don't retry this action."
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
        resolved: Mutex<Vec<(String, AskStatus)>>,
        notes: Mutex<Vec<(String, String)>>,
        released: Mutex<Vec<(String, bool)>>,
    }

    impl AskSurfaces for Seen {
        fn card(&self, ask: &Ask) {
            self.cards.lock().unwrap().push(ask.id.clone());
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

        async fn answer(&self, id: &str, a: Answer, via: AnsweredVia) -> Result<Ask, AskError> {
            let settled = self.asks.answer(&self.reg, id, a, via)?;
            if let Some(h) = settled.resumed {
                h.await.unwrap();
            }
            Ok(settled.ask)
        }
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
        assert_eq!(ask.expires_at - ask.created_at, EXPIRES_AFTER_SECS);
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
            created_at: 0,
            expires_at: 0,
        };
        let rule = allow_always_rule(&store, &ask);
        assert_eq!(rule.field, Some(RuleField::Recipient("+15550142".into())));
        assert!(super::super::rules::matches(&rule, &target("+15550142")));
        assert!(!super::super::rules::matches(&rule, &target("+15550177")), "not texting in general");
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
        let settled = r.asks.answer(&r.reg, &id, Answer::ThisOnce, AnsweredVia::Inbox).unwrap();
        assert!(settled.resumed.is_none(), "the run resumes the call, not the answer");
        assert_eq!(*r.seen.released.lock().unwrap(), vec![("run-1".to_string(), true)]);
        assert!(r.seen.notes().is_empty());
        assert_eq!(r.ran(1), 0);
        let no = r.park("workflow:wf-2", Door::Workflow, "+15550177").await;
        r.store.link_permission_ask_run(&no, "run-2").unwrap();
        r.answer(&no, Answer::No, AnsweredVia::Inbox).await.unwrap();
        assert_eq!(r.seen.released.lock().unwrap()[1], ("run-2".to_string(), false));
    }

    #[tokio::test]
    async fn unanswered_ask_expires_as_no_after_72h() {
        let r = rig().await;
        let id = r.park(KEY, Door::Heartbeat, "+15550142").await;
        let created = r.asks.get(&id).unwrap().unwrap().created_at;
        assert_eq!(r.asks.expire_due(created + EXPIRES_AFTER_SECS - 60), 0, "not yet");
        assert_eq!(r.asks.expire_due(created + EXPIRES_AFTER_SECS), 1);
        assert_eq!(r.asks.expire_due(created + EXPIRES_AFTER_SECS + 60), 0, "once");
        assert_eq!(r.asks.get(&id).unwrap().unwrap().status, AskStatus::Expired);
        let told = r.seen.notes();
        assert_eq!(told.len(), 1);
        assert!(told[0].1.contains("no answer in 72 hours, so it counts as declined"), "{}", told[0].1);
        assert!(matches!(r.answer(&id, Answer::ThisOnce, AnsweredVia::Mobile).await, Err(AskError::Settled(_))));
    }

    #[tokio::test]
    async fn expiry_never_approves() {
        let r = rig().await;
        let id = r.park(KEY, Door::Heartbeat, "+15550142").await;
        let wf = r.park("workflow:wf-1", Door::Workflow, "+15550142").await;
        r.store.link_permission_ask_run(&wf, "run-1").unwrap();
        assert_eq!(r.asks.expire_due(i64::MAX), 2);
        assert_eq!(r.ran(1), 0, "nothing ran");
        assert!(r.store.permission_rules("emp").unwrap().is_empty(), "nothing granted");
        assert_eq!(*r.seen.released.lock().unwrap(), vec![("run-1".to_string(), false)]);
        let row = r.store.get_permission_ask(&id).unwrap().unwrap();
        assert_eq!((row.status.as_str(), row.answer.as_deref()), ("expired", Some("no")));
        // The employee's next try is a plain refusal, never a repeat ask.
        let again = r.reg.execute(&ctx(KEY, Door::Heartbeat), "text", text("+15550142")).await;
        assert!(again.is_error && again.parked_ask.is_none(), "{}", again.content);
        assert_eq!(r.seen.cards(), 2);
    }
}
