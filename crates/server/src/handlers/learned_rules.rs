//! Learned rules: the assistant handles a kind of decision for the owner,
//! only once he said yes to it.
//!
//! Built on "Allow always", as one mechanism: a learned rule IS a permission
//! rule (`RuleSource::Learned`) the one check decides with, so anything
//! outside it still asks (over its money ceiling: case 1; someone new: case
//! 2). The offer, the owner's yes (when, where, his time zone) and every
//! later change are `pending_writes` rows (kind `rule`), the proposals the
//! Inbox already decides (`learn:<id>`), kept for good.
//!
//! - The offer: after the owner answers the same kind of ask "This once"
//!   three times in a row, the assistant offers to handle it ([`after_answer`]).
//!   Mixed answers make no offer. "Ask me each time" is not offered again
//!   for 30 days, "Don't offer again" never (until he takes it off that list).
//! - Handled: a call the rule let through that would have asked is one
//!   "Handled for you" update ([`handled`]), with Why? and, where it can be
//!   reversed, Undo.
//! - The list on the assistant's page: every rule with its switch, its uses,
//!   when he said yes and its history; the "never offer again" list; and the
//!   kinds of decision he can add himself.

use axum::extract::{Path, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};

use agent::harness::permissions::{Answer, Ask, AskKind, AskStatus, Handled, RuleSet};
use types::permissions::AskCase;
use db::models::PendingWrite;
use types::NeboError;
use types::permissions::{MoneyLimit, Rule, RuleField, RuleKey, RuleSource, Scope, Writer};

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

/// How many matching answers in a row make an offer.
pub(crate) const MATCHING_ANSWERS: usize = 3;

/// How long "Ask me each time" keeps the same offer from coming back.
const ASK_EACH_TIME_SECS: i64 = 30 * 24 * 3600;

/// The `pending_writes` kind of an offer and of a rule's history.
const RULE: &str = "rule";
/// The `pending_writes` action of an offer.
const OFFER: &str = "offer";

/// A kind of decision the owner answers the same way: what a learned rule
/// covers. Stored as the content of its offer's row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Pattern {
    pub agent_id: String,
    pub employee: String,
    pub key: RuleKey,
    pub field: Option<RuleField>,
    /// The newest of the asks, in the employee's own words.
    pub example: String,
    /// The most any of the answered calls spent, rounded up: the ceiling
    /// the offer proposes. None when they spent nothing.
    pub ceiling_cents: Option<i64>,
    /// Who the answered calls reached; anyone new still asks.
    pub counterparties: Vec<String>,
    /// One of them removed, sent or published something: it can't be undone.
    pub cant_undo: bool,
    /// The bot the rule is on.
    pub bot: String,
}

/// What a rule covers, what the assistant does, and what still asks, in
/// the owner's words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleWords {
    pub covers: String,
    pub does: String,
    pub still_asks: String,
}

/// `cents` in dollars: "$500", "$12.50".
fn dollars(cents: i64) -> String {
    if cents % 100 == 0 { format!("${}", cents / 100) } else { format!("${}.{:02}", cents / 100, cents % 100) }
}

/// The ceiling an offer proposes for the most the answered calls spent:
/// the next of 1, 2, 5 × 10ⁿ dollars at or over it ($312 → $500). The owner
/// can change it; there is no fixed limit.
pub(crate) fn ceiling_for(max_cents: i64) -> i64 {
    let mut step = 100_i64;
    loop {
        for m in [1, 2, 5] {
            if step * m >= max_cents {
                return step * m;
            }
        }
        step *= 10;
    }
}

/// "A", "A or B", "A, B or C".
fn either(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

impl Pattern {
    /// Its words with the ceiling the rule holds now.
    pub(crate) fn words(&self, ceiling_cents: Option<i64>) -> RuleWords {
        let mut covers = format!("Like \u{201c}{}\u{201d}", self.example.trim().trim_end_matches('.'));
        if let Some(c) = ceiling_cents {
            covers.push_str(&format!(", up to {} each", dollars(c)));
        }
        if !self.counterparties.is_empty() {
            covers.push_str(&format!(", with {}", either(&self.counterparties)));
        }
        let mut still = Vec::new();
        if let Some(c) = ceiling_cents {
            still.push(format!("Anything over {}", dollars(c)));
        }
        if !self.counterparties.is_empty() {
            still.push("anyone new".to_string());
        }
        still.push("anything not like this".to_string());
        let mut still_asks = still.join(", ");
        if let Some(first) = still_asks.get(..1) {
            still_asks.replace_range(..1, &first.to_uppercase());
        }
        RuleWords { covers, does: format!("Let {} go ahead without asking you", self.employee), still_asks }
    }

    /// The offer row's target: one employee's one kind of call.
    fn target(&self) -> String {
        target_of(&self.agent_id, &self.key)
    }
}

fn target_of(agent_id: &str, key: &RuleKey) -> String {
    format!("{agent_id}|{}:{}", key.kind(), key.value())
}

/// The kind of decision an ask is: the one standing allow "Allow always"
/// would write for it, without naming its one person (a rule names a kind
/// of call; who is new is still asked about). None when "Allow always"
/// isn't offered for it, it would write more than one rule, or it is a
/// first message to someone new (a rule can't cover that: the next new
/// person is asked about anyway).
fn kind_of(store: &db::Store, ask: &Ask) -> Option<(RuleKey, Option<RuleField>)> {
    if ask.kind() != AskKind::Permission
        || matches!(ask.case, AskCase::NewCounterparty { .. })
        || !ask.allow_always_offered(store)
    {
        return None;
    }
    let mut rules = agent::harness::permissions::ask::allow_always_rules(store, ask)?;
    if rules.len() != 1 {
        return None;
    }
    let rule = rules.remove(0);
    let field = rule.field.filter(|f| !matches!(f, RuleField::Recipient(_) | RuleField::Domain(_)));
    Some((rule.key, field))
}

/// The pattern behind `asks` (newest first, one kind).
fn pattern_of(state: &AppState, asks: &[Ask]) -> Option<Pattern> {
    let newest = asks.first()?;
    let (key, field) = kind_of(&state.store, newest)?;
    let spent = asks.iter().filter_map(|a| a.target.effects.money_cents).filter(|c| *c > 0).max();
    let mut counterparties: Vec<String> = Vec::new();
    for a in asks {
        // The people the calls reached: anyone else is someone new, which
        // the check still asks about (case 2).
        for who in &a.target.effects.recipients {
            if !who.is_empty() && !counterparties.iter().any(|c| c.eq_ignore_ascii_case(who)) {
                counterparties.push(who.clone());
            }
        }
    }
    let cant_undo = asks.iter().any(|a| {
        matches!(a.case, AskCase::Irreversible { .. }) || !Handled::undoable(&a.target.effects)
    });
    Some(Pattern {
        agent_id: newest.agent_id.clone(),
        employee: super::permissions::employee_name(state, &newest.agent_id),
        key,
        field,
        example: newest.sentence.clone(),
        ceiling_cents: spent.map(ceiling_for),
        counterparties,
        cant_undo,
        bot: super::permissions::employee_name(state, ""),
    })
}

/// The owner's asks of the same kind as `ask`, newest answer first.
fn same_kind(state: &AppState, ask: &Ask) -> Vec<Ask> {
    let Some(kind) = kind_of(&state.store, ask) else { return Vec::new() };
    state
        .permission_asks
        .answered(&ask.agent_id, 200)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| kind_of(&state.store, a).as_ref() == Some(&kind))
        .collect()
}

/// Whether the owner answered `ask` "This once".
fn this_once(ask: &Ask) -> bool {
    matches!(ask.status, AskStatus::Answered { answer: Answer::ThisOnce, .. })
}

/// Whether an offer for `target` must not be made now: one is waiting, the
/// owner said "Ask me each time" in the last 30 days, or "Don't offer again".
fn offer_held(store: &db::Store, agent_id: &str, target: &str) -> bool {
    let on = |rows: Vec<PendingWrite>| rows.iter().any(|r| r.target == target);
    on(store.open_pending_writes(agent_id, RULE, OFFER).unwrap_or_default())
        || on(store.declined_pending_writes(agent_id, RULE, OFFER, ASK_EACH_TIME_SECS).unwrap_or_default())
        || on(store.pending_writes_in(RULE, OFFER, "never").unwrap_or_default())
}

/// The owner just answered `ask`. When it is the third "This once" in a row
/// for the same kind of decision, and no rule, open offer or "no" stands in
/// the way, the assistant offers to handle it. The offer is a proposal in
/// the Inbox (`learn:<id>`); nothing changes until he says yes.
pub(crate) fn after_answer(state: &AppState, ask: &Ask) {
    if !this_once(ask) {
        return;
    }
    let asks = same_kind(state, ask);
    let last: Vec<Ask> = asks.into_iter().take(MATCHING_ANSWERS).collect();
    if last.len() < MATCHING_ANSWERS || !last.iter().all(this_once) {
        return;
    }
    let rules = match RuleSet::load(&state.store, &ask.agent_id) {
        Ok(r) => r,
        Err(e) => return tracing::warn!(error = %e, "learned rules: the employee's rules can't be read"),
    };
    if rules.answered_always(&ask.target) {
        return;
    }
    let Some(pattern) = pattern_of(state, &last) else { return };
    if offer_held(&state.store, &ask.agent_id, &pattern.target()) {
        return;
    }
    if let Err(e) = offer(state, &pattern) {
        tracing::warn!(error = %e, agent = %ask.agent_id, "learned rules: the offer was not made");
    }
}

/// The answers an offer takes on its card.
pub(crate) fn offer_options() -> Vec<db::models::ItemOption> {
    [("Yes, handle these", "yes"), ("Ask me each time", "no"), ("Don't offer again", "never")]
        .into_iter()
        .map(|(label, value)| db::models::ItemOption { label: label.into(), value: value.into() })
        .collect()
}

/// Put the offer before the owner: its proposal row and its Inbox card,
/// from the assistant.
fn offer(state: &AppState, pattern: &Pattern) -> Result<String, NeboError> {
    let id = uuid::Uuid::new_v4().to_string();
    let content = serde_json::to_string(pattern).map_err(|e| NeboError::Internal(e.to_string()))?;
    let words = pattern.words(pattern.ceiling_cents);
    state.store.create_pending_write(&id, &pattern.agent_id, RULE, OFFER, &pattern.target(), Some(&content), &words.covers, "", None)?;
    let item = format!("learn:{id}");
    let title = format!("Want me to handle these for {}?", pattern.employee);
    let body = format!(
        "The last {MATCHING_ANSWERS} times, you said yes to \u{201c}{}\u{201d}.",
        pattern.example.trim().trim_end_matches('.')
    );
    let options = offer_options();
    tools::owner_notify::emit(
        &state.store,
        Some(&|ev, payload| state.hub.broadcast(ev, payload)),
        &tools::owner_notify::OwnerNotification {
            id: &item,
            kind: OFFER_TYPE,
            title: &title,
            body: Some(&body),
            agent_id: Some(tools::team_tool::PRIMARY_AGENT_ID),
            needs_you: true,
            options: &options,
            ..Default::default()
        },
    );
    Ok(id)
}

/// The Inbox type of an offer card.
pub(crate) const OFFER_TYPE: &str = "rule_offer";
/// The Inbox type of a "Handled for you" update.
pub(crate) const HANDLED_TYPE: &str = "handled";

/// The owner's time zone now, as his consent records it.
fn time_zone(store: &db::Store) -> Option<&'static str> {
    tools::owner_clock::OwnerZone::of(store).name()
}

/// The owner decided an offer (`value`: yes, no or never) on `via`. Yes
/// writes the rule, through the one rule writer, as his own; his yes, where
/// he gave it and his time zone are recorded on the offer, and its history
/// starts. The card moves to History with his answer.
pub(crate) fn decide_offer(state: &AppState, row: &PendingWrite, value: &str, via: Option<&str>) -> Result<serde_json::Value, NeboError> {
    let tz = time_zone(&state.store);
    let (status, answer) = match value {
        "yes" => ("approved", "Yes, handle these"),
        "never" => ("never", "Don't offer again"),
        _ => ("rejected", "Ask me each time"),
    };
    let mut rule_id = None;
    if status == "approved" {
        let pattern: Pattern = serde_json::from_str(row.content.as_deref().unwrap_or_default())
            .map_err(|e| NeboError::Internal(format!("the offer can't be read: {e}")))?;
        let written = turn_on(state, &pattern, &row.id, &format!("You turned it on from {}'s offer", assistant_name(state)), via, tz)?;
        rule_id = Some(written.id);
    }
    if !state.store.resolve_pending_write(&row.id, status)? {
        return Ok(serde_json::json!({ "status": row.status }));
    }
    state.store.stamp_pending_write(&row.id, via, tz)?;
    state.hub.broadcast("learning_resolved", serde_json::json!({ "id": row.id, "status": status, "agentId": row.agent_id }));
    let item = format!("learn:{}", row.id);
    tools::owner_notify::settle(&state.store, Some(&|ev, payload| state.hub.broadcast(ev, payload)), &item, answer, via);
    crate::codes::push_inbox(state, serde_json::json!({ "id": item, "resolved": true }));
    Ok(serde_json::json!({ "status": status, "ruleId": rule_id }))
}

/// Write the rule `pattern` describes, consented on `consent_id`, and
/// start its history with `gist`.
fn turn_on(state: &AppState, pattern: &Pattern, consent_id: &str, gist: &str, via: Option<&str>, tz: Option<&str>) -> Result<Rule, NeboError> {
    let rule = Rule {
        id: uuid::Uuid::new_v4().to_string(),
        scope: Scope::Employee(pattern.agent_id.clone()),
        key: pattern.key.clone(),
        field: pattern.field.clone(),
        effect: types::permissions::Effect::Allow,
        money: pattern.ceiling_cents.map(|c| MoneyLimit { per_action_cents: Some(c), ..Default::default() }),
        source: RuleSource::Learned { consent_id: consent_id.to_string() },
        locked: false,
        created_at: chrono::Utc::now().timestamp(),
    };
    let written = state
        .store
        .write_permission_rule(&rule, &Writer::Owner)
        .map_err(|e| NeboError::Validation(e.to_string()))?;
    state.store.record_rule_change(&written.id, &pattern.agent_id, "turned_on", &written, &written, gist, via, tz)?;
    Ok(written)
}

/// The assistant's name, as the owner named it.
fn assistant_name(state: &AppState) -> String {
    super::permissions::employee_name(state, tools::team_tool::PRIMARY_AGENT_ID)
}

/// A call a learned rule let through that would have asked the owner: one
/// "Handled for you" update from the assistant, with the rule it ran under
/// (its id is in the item's: `handled:<rule>:<n>`, what Why? opens) and,
/// when the call can be reversed, Undo, which asks the employee that did
/// it to reverse it, in its conversation.
pub(crate) fn handled(state: &AppState, h: &Handled) {
    let employee = super::permissions::employee_name(state, &h.agent_id);
    let id = format!("handled:{}:{}", h.rule_id, &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let mut title = h.sentence.trim().to_string();
    if let Some(first) = title.get(..1) {
        title.replace_range(..1, &first.to_uppercase());
    }
    let body = format!("{employee} went ahead under your rule, without asking you.");
    let link = tools::owner_notify::link::session_chat(&state.store, &h.agent_id, &h.session_key);
    let options: Vec<db::models::ItemOption> = if h.undoable {
        vec![db::models::ItemOption { label: "Undo".into(), value: UNDO.into() }]
    } else {
        Vec::new()
    };
    tools::owner_notify::emit(
        &state.store,
        Some(&|ev, payload| state.hub.broadcast(ev, payload)),
        &tools::owner_notify::OwnerNotification {
            id: &id,
            kind: HANDLED_TYPE,
            title: &title,
            body: Some(&body),
            action_url: Some(&link),
            agent_id: Some(tools::team_tool::PRIMARY_AGENT_ID),
            options: &options,
            link_label: Some("Chat"),
            reply_to: Some(&h.session_key),
            ..Default::default()
        },
    );
}

/// The answer a handled update's Undo sends.
pub(crate) const UNDO: &str = "undo";

/// The owner pressed Undo on a handled update: the employee that did it is
/// asked, in its conversation, to reverse it and say what it did; the item
/// moves to History saying so.
pub(crate) fn undo(state: &AppState, item: &db::models::Notification, via: &str) -> Result<(), NeboError> {
    let Some(session) = item.reply_to.as_deref().filter(|s| !s.is_empty()) else {
        return Err(NeboError::Validation("this can't be undone from here".into()));
    };
    if !tools::owner_notify::settle(&state.store, Some(&|ev, p| state.hub.broadcast(ev, p)), &item.id, "Undo asked", Some(via)) {
        return Ok(());
    }
    let text = format!(
        "The owner pressed Undo on \u{201c}{}\u{201d}, which you did under the rule they agreed to, without asking. \
         Reverse it if you can, then tell them what you did, or why it can't be reversed.",
        item.title
    );
    crate::wake::enqueue(state, session, agent::harness::delegation::notify::WAKE_KIND, &text, &[], 0);
    Ok(())
}

// ── The list on the assistant's page ────────────────────────────────────

/// When the owner said yes, or changed it, and where.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleEvent {
    /// turned_on | edited | paused | resumed
    pub event: String,
    pub at: i64,
    pub time_zone: Option<String>,
    /// The zone's UTC offset and abbreviation at `at`, for a device that
    /// can't look the zone up.
    pub offset_seconds: Option<i32>,
    pub zone_abbr: Option<String>,
    pub via: Option<String>,
    pub detail: String,
}

/// One learned rule as the owner reads it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnedRule {
    pub id: String,
    pub agent_id: String,
    pub employee: String,
    #[serde(flatten)]
    pub words: RuleWords,
    pub cant_undo: bool,
    pub ceiling_cents: Option<i64>,
    pub counterparties: Vec<String>,
    pub bot: String,
    pub paused: bool,
    /// When he said yes (the first entry of its history).
    pub consent: RuleEvent,
    pub history: Vec<RuleEvent>,
    /// How many times it handled something for him, and the last time.
    pub uses: i64,
    pub last_used_at: Option<i64>,
}

/// A kind of decision the owner said never to offer again.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeverOffer {
    pub id: String,
    pub agent_id: String,
    pub employee: String,
    pub what: String,
}

/// A kind of decision the owner can hand to his assistant himself, from
/// his own recent answers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleCandidate {
    /// The newest ask of its kind: what "Add" names.
    pub id: String,
    pub agent_id: String,
    pub employee: String,
    #[serde(flatten)]
    pub words: RuleWords,
    pub cant_undo: bool,
    pub ceiling_cents: Option<i64>,
    pub counterparties: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnedRulesResponse {
    pub rules: Vec<LearnedRule>,
    pub never_offer: Vec<NeverOffer>,
    pub candidates: Vec<RuleCandidate>,
}

fn event_of(row: &PendingWrite) -> RuleEvent {
    let at = row.resolved_at.unwrap_or(row.created_at);
    let local = row.resolved_tz.as_deref().and_then(|tz| tools::owner_clock::offset_at(tz, at));
    RuleEvent {
        event: row.action.clone(),
        at,
        time_zone: row.resolved_tz.clone(),
        offset_seconds: local.as_ref().map(|(o, _)| *o),
        zone_abbr: local.map(|(_, a)| a),
        via: row.resolved_via.clone(),
        detail: row.gist.clone(),
    }
}

/// One rule with its consent, history and uses.
fn learned_rule(state: &AppState, rule: &Rule, paused: bool) -> Option<LearnedRule> {
    let RuleSource::Learned { consent_id } = &rule.source else { return None };
    let consent = state.store.get_pending_write(consent_id).ok().flatten()?;
    let pattern: Pattern = serde_json::from_str(consent.content.as_deref()?).ok()?;
    let history: Vec<RuleEvent> = state
        .store
        .pending_writes_on(RULE, &rule.id)
        .unwrap_or_default()
        .iter()
        .map(event_of)
        .collect();
    let ceiling = rule.money.as_ref().and_then(|m| m.per_action_cents);
    let (uses, last_used_at) = state
        .store
        .count_notifications_like(&format!("handled:{}:", rule.id))
        .unwrap_or((0, None));
    Some(LearnedRule {
        id: rule.id.clone(),
        agent_id: pattern.agent_id.clone(),
        employee: super::permissions::employee_name(state, &pattern.agent_id),
        words: pattern.words(ceiling),
        cant_undo: pattern.cant_undo,
        ceiling_cents: ceiling,
        counterparties: pattern.counterparties.clone(),
        bot: pattern.bot.clone(),
        paused,
        consent: history.first().cloned().unwrap_or_else(|| event_of(&consent)),
        history,
        uses,
        last_used_at,
    })
}

/// The kinds of decision the owner answered "This once" in the last 30 days
/// that no rule covers and he didn't say never to: what he can add himself.
fn candidates(state: &AppState) -> Vec<RuleCandidate> {
    let since = chrono::Utc::now().timestamp() - ASK_EACH_TIME_SECS;
    let mut out: Vec<RuleCandidate> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for agent_id in state.store.agents_with_answered_asks(since).unwrap_or_default() {
        let Ok(rules) = RuleSet::load(&state.store, &agent_id) else { continue };
        let never = state.store.pending_writes_in(RULE, OFFER, "never").unwrap_or_default();
        for ask in state.permission_asks.answered(&agent_id, 200).unwrap_or_default() {
            if !this_once(&ask) || rules.answered_always(&ask.target) {
                continue;
            }
            let Some((key, _)) = kind_of(&state.store, &ask) else { continue };
            let target = target_of(&agent_id, &key);
            if seen.contains(&target) || never.iter().any(|r| r.target == target) {
                continue;
            }
            seen.push(target);
            let kind: Vec<Ask> = same_kind(state, &ask).into_iter().filter(this_once).take(MATCHING_ANSWERS).collect();
            if let Some(p) = pattern_of(state, &kind) {
                out.push(RuleCandidate {
                    id: ask.id.clone(),
                    agent_id: p.agent_id.clone(),
                    employee: p.employee.clone(),
                    words: p.words(p.ceiling_cents),
                    cant_undo: p.cant_undo,
                    ceiling_cents: p.ceiling_cents,
                    counterparties: p.counterparties.clone(),
                });
            }
        }
    }
    out
}

/// GET /api/v1/permissions/learned — what the assistant handles for the
/// owner: every learned rule (on or off), the offers he said never to make
/// again, and what he can add himself.
pub async fn list_learned_rules(State(state): State<AppState>) -> HandlerResult<LearnedRulesResponse> {
    let rules = state
        .store
        .learned_permission_rules()
        .map_err(to_error_response)?
        .iter()
        .filter_map(|(rule, paused)| learned_rule(&state, rule, paused.is_some()))
        .collect();
    let never_offer = state
        .store
        .pending_writes_in(RULE, OFFER, "never")
        .map_err(to_error_response)?
        .into_iter()
        .filter_map(|row| {
            let p: Pattern = serde_json::from_str(row.content.as_deref()?).ok()?;
            Some(NeverOffer { id: row.id, agent_id: p.agent_id.clone(), employee: p.employee.clone(), what: p.words(p.ceiling_cents).covers })
        })
        .collect();
    Ok(Json(LearnedRulesResponse { rules, never_offer, candidates: candidates(&state) }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleChangeBody {
    /// Where the owner made the change: mobile, desktop, web.
    #[serde(default)]
    pub via: Option<String>,
    /// The new most each call may spend, in cents (edit and add).
    #[serde(default)]
    pub ceiling_cents: Option<i64>,
    /// The kind of decision to add (`RuleCandidate::id`).
    #[serde(default)]
    pub candidate_id: Option<String>,
}

/// The learned rule `id`, as stored, on or off.
fn stored_learned(state: &AppState, id: &str) -> Result<Rule, NeboError> {
    match state.store.get_permission_rule(id)? {
        Some(rule) if matches!(rule.source, RuleSource::Learned { .. }) => Ok(rule),
        _ => Err(NeboError::NotFound),
    }
}

fn rule_agent(rule: &Rule) -> &str {
    match &rule.scope {
        Scope::Employee(a) => a,
        Scope::Company => "",
    }
}

/// The rule as the owner reads it now.
fn read_back(state: &AppState, id: &str) -> HandlerResult<serde_json::Value> {
    let paused = state
        .store
        .learned_permission_rules()
        .map_err(to_error_response)?
        .into_iter()
        .find(|(r, _)| r.id == id)
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;
    Ok(Json(serde_json::json!({ "rule": learned_rule(&state, &paused.0, paused.1.is_some()) })))
}

/// Turn a learned rule off (`paused`) or back on, recorded in its history.
fn set_paused(state: &AppState, id: &str, paused: bool, via: Option<&str>) -> HandlerResult<serde_json::Value> {
    let rule = stored_learned(state, id).map_err(to_error_response)?;
    if state.store.set_permission_rule_paused(id, paused).map_err(to_error_response)? {
        let (event, gist) = if paused { ("paused", "You paused it") } else { ("resumed", "You turned it back on") };
        state
            .store
            .record_rule_change(id, rule_agent(&rule), event, &rule, &rule, gist, via, time_zone(&state.store))
            .map_err(to_error_response)?;
    }
    read_back(state, id)
}

/// POST /api/v1/permissions/learned/{id}/pause — "Stop doing this": the
/// rule is off, kept with its history, and everything it covered asks again.
pub async fn pause_learned_rule(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RuleChangeBody>,
) -> HandlerResult<serde_json::Value> {
    set_paused(&state, &id, true, body.via.as_deref())
}

/// POST /api/v1/permissions/learned/{id}/resume — turn it back on.
pub async fn resume_learned_rule(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RuleChangeBody>,
) -> HandlerResult<serde_json::Value> {
    set_paused(&state, &id, false, body.via.as_deref())
}

/// PUT /api/v1/permissions/learned/{id} — change its money ceiling. No
/// fixed limits: the owner decides.
pub async fn update_learned_rule(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RuleChangeBody>,
) -> HandlerResult<serde_json::Value> {
    let before = stored_learned(&state, &id).map_err(to_error_response)?;
    let ceiling = body.ceiling_cents.filter(|c| *c > 0);
    let old = before.money.as_ref().and_then(|m| m.per_action_cents);
    if ceiling == old {
        return read_back(&state, &id);
    }
    let money = match (before.money.clone(), ceiling) {
        (Some(m), Some(c)) => Some(MoneyLimit { per_action_cents: Some(c), ..m }),
        (None, Some(c)) => Some(MoneyLimit { per_action_cents: Some(c), ..Default::default() }),
        (Some(m), None) => Some(MoneyLimit { per_action_cents: None, ..m }).filter(|m| *m != MoneyLimit::default()),
        (None, None) => None,
    };
    let after = state
        .store
        .write_permission_rule(&Rule { money, ..before.clone() }, &Writer::Owner)
        .map_err(|e| to_error_response(NeboError::Validation(e.to_string())))?;
    let words = |c: Option<i64>| c.map_or("no limit".to_string(), dollars);
    let gist = format!("You changed the limit from {} to {}", words(old), words(ceiling));
    state
        .store
        .record_rule_change(&id, rule_agent(&after), "edited", &before, &after, &gist, body.via.as_deref(), time_zone(&state.store))
        .map_err(to_error_response)?;
    read_back(&state, &id)
}

/// POST /api/v1/permissions/learned — "Add a rule yourself": the owner
/// hands one of his own recent kinds of decision (`candidateId`) to his
/// assistant, with the ceiling he chooses. His yes is recorded as for an
/// offer he accepted.
pub async fn add_learned_rule(
    State(state): State<AppState>,
    Json(body): Json<RuleChangeBody>,
) -> HandlerResult<serde_json::Value> {
    let invalid = |m: &str| to_error_response(NeboError::Validation(m.to_string()));
    let ask_id = body.candidate_id.as_deref().ok_or_else(|| invalid("candidateId is required"))?;
    let ask = state
        .permission_asks
        .get(ask_id)
        .map_err(|e| to_error_response(NeboError::Internal(e.to_string())))?
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;
    let kind: Vec<Ask> = same_kind(&state, &ask).into_iter().filter(this_once).take(MATCHING_ANSWERS).collect();
    let mut pattern = pattern_of(&state, &kind).ok_or_else(|| invalid("that kind of decision can't be handed over"))?;
    if body.ceiling_cents.is_some() {
        pattern.ceiling_cents = body.ceiling_cents.filter(|c| *c > 0);
    }
    let consent_id = uuid::Uuid::new_v4().to_string();
    let content = serde_json::to_string(&pattern).map_err(|e| to_error_response(NeboError::Internal(e.to_string())))?;
    let words = pattern.words(pattern.ceiling_cents);
    let via = body.via.as_deref();
    let tz = time_zone(&state.store);
    state
        .store
        .record_applied_write(&consent_id, &pattern.agent_id, RULE, OFFER, &pattern.target(), Some(&content), &words.covers, "", None)
        .map_err(to_error_response)?;
    state.store.stamp_pending_write(&consent_id, via, tz).map_err(to_error_response)?;
    let rule = turn_on(&state, &pattern, &consent_id, "You added it yourself", via, tz).map_err(to_error_response)?;
    read_back(&state, &rule.id)
}

/// POST /api/v1/permissions/learned/never/{id}/offer-again — take a kind of
/// decision off the "never offer again" list: it may be offered again.
pub async fn offer_again(State(state): State<AppState>, Path(id): Path<String>) -> HandlerResult<serde_json::Value> {
    if !state.store.restatus_pending_write(&id, "never", "reopened").map_err(to_error_response)? {
        return Err(to_error_response(NeboError::NotFound));
    }
    Ok(Json(serde_json::json!({ "success": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ceiling_is_the_next_round_amount_at_or_over_the_most_spent() {
        assert_eq!(ceiling_for(31_200), 50_000);
        assert_eq!(ceiling_for(50_000), 50_000);
        assert_eq!(ceiling_for(1_499), 2_000);
        assert_eq!(ceiling_for(1), 100);
        assert_eq!(ceiling_for(120_000), 200_000);
    }

    #[test]
    fn the_words_say_what_it_covers_and_what_still_asks() {
        let p = Pattern {
            agent_id: "store".into(),
            employee: "Ecommerce Store Manager".into(),
            key: RuleKey::Operation("orders.purchase.create".into()),
            field: None,
            example: "placing an order with Lumen.".into(),
            ceiling_cents: Some(50_000),
            counterparties: vec!["Lumen".into(), "Northfield".into()],
            cant_undo: false,
            bot: "Nebo".into(),
        };
        let w = p.words(Some(50_000));
        assert_eq!(w.covers, "Like \u{201c}placing an order with Lumen\u{201d}, up to $500 each, with Lumen or Northfield");
        assert_eq!(w.does, "Let Ecommerce Store Manager go ahead without asking you");
        assert_eq!(w.still_asks, "Anything over $500, anyone new, anything not like this");
        assert_eq!(p.words(None).still_asks, "Anyone new, anything not like this");
    }
}
