//! The owner's permission surfaces (Turn-Controller-Technical-Design
//! §2.12.5, §2.12.9): the one ask card as the Inbox, the phone and the open
//! chat read and answer it; one employee's Permissions page, the company
//! defaults with the same controls, and the activity with why each action
//! was allowed.
//!
//! Every rule reaches the owner as a sentence rendered here, never as a rule
//! key or field. The pages read and write the one rule store
//! (`permission_rules`, `permission_modes`) through the store's writers, as
//! the owner.

use std::collections::BTreeSet;
use std::path::Path as FsPath;

use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};

use agent::harness::permissions::{Answer, AnsweredVia, Ask, AskError, AskStatus};
use types::permissions::{
    AskCase, Effect, Mode, MoneyLimit, Rule, RuleError, RuleField, RuleKey, RuleSource, Scope, Why, Writer,
};
use types::NeboError;

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

/// One ask as the owner sees it: who wants to do what, why it asked, and
/// where it stands.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionAskCard {
    pub id: String,
    pub agent_id: String,
    /// The employee's name.
    pub employee: String,
    pub session_key: String,
    /// What it wants to do, as its activity line ("sending a text to …").
    pub sentence: String,
    /// Why it asked, in plain words.
    pub reason: String,
    /// Whether "Allow always" is offered (a locked must-ask can't be loosened).
    pub allow_always: bool,
    /// Whether "This once" is offered (an employee's extra needs are granted
    /// for good or not at all).
    pub this_once: bool,
    /// open | allowed | declined | expired
    pub status: String,
    /// allow_always | this_once | no, once answered.
    pub answer: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

/// The card for `ask`.
pub(crate) fn card(state: &AppState, ask: &Ask) -> PermissionAskCard {
    let (status, answer) = match ask.status {
        AskStatus::Open => ("open", None),
        AskStatus::Answered { answer: Answer::No, .. } => ("declined", Some(Answer::No)),
        AskStatus::Answered { answer, .. } => ("allowed", Some(answer)),
        AskStatus::Expired => ("expired", None),
    };
    PermissionAskCard {
        id: ask.id.clone(),
        agent_id: ask.agent_id.clone(),
        employee: employee_name(state, &ask.agent_id),
        session_key: ask.session_key.clone(),
        sentence: ask.sentence.clone(),
        reason: ask.reason().to_string(),
        allow_always: ask.allow_always_offered(&state.store),
        this_once: ask.this_once_offered(),
        status: status.to_string(),
        answer: answer.map(|a| a.as_str().to_string()),
        created_at: ask.created_at,
        expires_at: ask.expires_at,
    }
}

/// The employee's name; the main assistant goes by the bot's own name.
fn employee_name(state: &AppState, agent_id: &str) -> String {
    let named = |n: String| (!n.trim().is_empty()).then_some(n);
    let agent = (!agent_id.is_empty())
        .then(|| state.store.get_agent(agent_id).ok().flatten())
        .flatten()
        .and_then(|a| named(a.name));
    agent
        .or_else(|| state.store.get_agent_profile().ok().flatten().and_then(|p| named(p.name)))
        .unwrap_or_else(|| "Nebo".to_string())
}

fn ask_error(e: AskError) -> (axum::http::StatusCode, Json<types::api::ErrorResponse>) {
    to_error_response(match e {
        AskError::NotFound => NeboError::NotFound,
        AskError::Settled(_) => NeboError::Validation("this was already answered".into()),
        AskError::Store(msg) => NeboError::Database(msg),
    })
}

#[derive(Debug, Deserialize)]
pub struct ListAsksQuery {
    /// Only the asks of this session (the open chat).
    pub session: Option<String>,
}

/// The asks waiting on the owner.
#[derive(Debug, Serialize)]
pub struct PermissionAsksResponse {
    pub asks: Vec<PermissionAskCard>,
}

/// GET /api/v1/permissions/asks — the asks waiting on the owner, oldest
/// first; `?session=` narrows them to one chat.
pub async fn list_permission_asks(
    State(state): State<AppState>,
    Query(q): Query<ListAsksQuery>,
) -> HandlerResult<PermissionAsksResponse> {
    let asks = state.permission_asks.open(q.session.as_deref()).map_err(ask_error)?;
    Ok(Json(PermissionAsksResponse { asks: asks.iter().map(|a| card(&state, a)).collect() }))
}

/// GET /api/v1/permissions/asks/{id} — one ask and where it stands.
pub async fn get_permission_ask(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<PermissionAskCard> {
    let ask = state.permission_asks.get(&id).map_err(ask_error)?.ok_or_else(|| ask_error(AskError::NotFound))?;
    Ok(Json(card(&state, &ask)))
}

#[derive(Debug, Deserialize)]
pub struct AnswerAskBody {
    /// allow_always | this_once | no
    pub answer: String,
    /// chat | inbox | mobile
    pub via: String,
}

/// POST /api/v1/permissions/asks/{id}/answer — the owner's answer. The
/// first answer anywhere wins; a later one gets the card as it was settled.
pub async fn answer_permission_ask(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AnswerAskBody>,
) -> HandlerResult<PermissionAskCard> {
    let invalid = |msg: &str| to_error_response(NeboError::Validation(msg.to_string()));
    let answer = Answer::parse(&body.answer).ok_or_else(|| invalid("answer must be allow_always, this_once or no"))?;
    let via = AnsweredVia::parse(&body.via).ok_or_else(|| invalid("via must be chat, inbox or mobile"))?;
    match state.permission_asks.answer(&state.tools, &id, answer, via) {
        Ok(settled) => Ok(Json(card(&state, &settled.ask))),
        Err(AskError::Settled(ask)) => Ok(Json(card(&state, &ask))),
        Err(e) => Err(ask_error(e)),
    }
}

// ── The Permissions pages ───────────────────────────────────────────────

/// One employee's permissions, or the company defaults, in plain words.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsPage {
    /// automatic | ask | plan | full_access
    pub mode: String,
    /// The employee has no mode of its own and follows the company default.
    pub mode_from_company: bool,
    /// The company default mode.
    pub company_mode: String,
    /// What the job includes: the capabilities it may use.
    pub job: Vec<PermissionItem>,
    /// Capabilities the owner can add to the job.
    pub can_add: Vec<PermissionItem>,
    /// Money it may spend without asking.
    pub money: Vec<PermissionItem>,
    /// Folders it may change files in.
    pub folders: Vec<PermissionItem>,
    /// Actions allowed from past answers and the owner's own settings.
    pub always_allowed: Vec<PermissionItem>,
    /// Actions that ask the owner first.
    pub asks_first: Vec<PermissionItem>,
    /// Actions that never run.
    pub never: Vec<PermissionItem>,
    /// Rules a company law or the employee's package sets; they can't change here.
    pub fixed: Vec<PermissionItem>,
}

/// One line on a Permissions page.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionItem {
    pub id: String,
    pub sentence: String,
    pub removable: bool,
    /// Comes from the company defaults (shown on an employee's page).
    pub from_company: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub money: Option<MoneyAmounts>,
}

/// A money limit's amounts, for editing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoneyAmounts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_action_cents: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_day_cents: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_day_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_counterparty_day_cents: Option<i64>,
}

impl From<&MoneyLimit> for MoneyAmounts {
    fn from(m: &MoneyLimit) -> Self {
        MoneyAmounts {
            per_action_cents: m.per_action_cents,
            per_day_cents: m.per_day_cents,
            per_day_count: m.per_day_count,
            per_counterparty_day_cents: m.per_counterparty_day_cents,
        }
    }
}

impl From<&MoneyAmounts> for MoneyLimit {
    fn from(m: &MoneyAmounts) -> Self {
        let positive = |v: Option<i64>| v.filter(|n| *n >= 0);
        MoneyLimit {
            per_action_cents: positive(m.per_action_cents),
            per_day_cents: positive(m.per_day_cents),
            per_day_count: positive(m.per_day_count),
            per_counterparty_day_cents: positive(m.per_counterparty_day_cents),
        }
    }
}

/// A change to a Permissions page. Each field present is applied.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsUpdate {
    /// `automatic`, `ask`, `plan` or `full_access`; on an employee's page,
    /// `company` follows the company default.
    pub mode: Option<String>,
    /// The id of a `canAdd` item.
    pub add_capability: Option<String>,
    /// A folder the job may change files in.
    pub add_folder: Option<String>,
    /// New amounts for a money item.
    pub money: Option<MoneyEdit>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoneyEdit {
    pub id: String,
    #[serde(flatten)]
    pub amounts: MoneyAmounts,
}

/// Which activity to list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityQuery {
    pub agent_id: Option<String>,
    /// chat | helper | workflow | schedule | heartbeat | coworker | voice | mcp | local_api
    pub door: Option<String>,
    /// allow | ask | deny
    pub decision: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// The activity: what employees did, and why each action was allowed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPage {
    pub rows: Vec<ActivityRow>,
    pub total: i64,
}

/// One recorded action.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRow {
    pub at: i64,
    pub employee_id: String,
    pub employee: String,
    pub action: String,
    /// allow | ask | deny
    pub decision: String,
    /// Why it was allowed, asked or refused, in plain words.
    pub why: String,
    /// The entry the run came through (a door id).
    pub door: String,
    /// Neither judge could review it; it ran unchecked.
    pub unreviewed: bool,
}

// ── Handlers ────────────────────────────────────────────────────────────

/// GET /api/v1/agents/{id}/permissions
pub async fn get_agent_permissions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<PermissionsPage> {
    let connected = connected(&state);
    page(&state.store, Some(&id), &connected).map(Json).map_err(to_error_response)
}

/// PUT /api/v1/agents/{id}/permissions
pub async fn update_agent_permissions(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PermissionsUpdate>,
) -> HandlerResult<PermissionsPage> {
    let connected = connected(&state);
    update(&state.store, Some(&id), &body, &connected).map_err(to_error_response)?;
    page(&state.store, Some(&id), &connected).map(Json).map_err(to_error_response)
}

/// DELETE /api/v1/agents/{id}/permissions/items/{rule_id}
pub async fn remove_agent_permission(
    State(state): State<AppState>,
    Path((id, rule_id)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    remove(&state.store, Some(&id), &rule_id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "message": "Removed" })))
}

/// GET /api/v1/permissions/company
pub async fn get_company_permissions(State(state): State<AppState>) -> HandlerResult<PermissionsPage> {
    let connected = connected(&state);
    page(&state.store, None, &connected).map(Json).map_err(to_error_response)
}

/// PUT /api/v1/permissions/company
pub async fn update_company_permissions(
    State(state): State<AppState>,
    Json(body): Json<PermissionsUpdate>,
) -> HandlerResult<PermissionsPage> {
    let connected = connected(&state);
    update(&state.store, None, &body, &connected).map_err(to_error_response)?;
    page(&state.store, None, &connected).map(Json).map_err(to_error_response)
}

/// DELETE /api/v1/permissions/company/items/{rule_id}
pub async fn remove_company_permission(
    State(state): State<AppState>,
    Path(rule_id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    remove(&state.store, None, &rule_id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "message": "Removed" })))
}

/// GET /api/v1/permissions/activity — every employee's actions, or one
/// employee's (`agentId`), filterable by door and decision.
pub async fn list_permission_activity(
    State(state): State<AppState>,
    Query(q): Query<ActivityQuery>,
) -> HandlerResult<ActivityPage> {
    activity(&state.store, &q).map(Json).map_err(to_error_response)
}

fn connected(state: &AppState) -> BTreeSet<String> {
    super::agents::connected_capabilities(state).into_keys().collect()
}

// ── The page ────────────────────────────────────────────────────────────

fn scope_of(agent_id: Option<&str>) -> Scope {
    match agent_id {
        Some(id) => Scope::Employee(id.to_string()),
        None => Scope::Company,
    }
}

fn rule_error(e: RuleError) -> NeboError {
    match e {
        RuleError::NotFound => NeboError::NotFound,
        RuleError::Store(s) => NeboError::Database(s),
        other => NeboError::Validation(other.to_string()),
    }
}

/// Built-in capabilities the job can hold, in display order.
fn builtin_capabilities() -> impl Iterator<Item = &'static str> {
    tools::capabilities::CAPABILITIES
        .iter()
        .map(|c| c.key)
        .filter(|k| *k != "chat")
}

/// Where a rule shows on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Job,
    Money,
    Folders,
    AlwaysAllowed,
    AsksFirst,
    Never,
    Fixed,
}

fn section_of(rule: &Rule) -> Section {
    if rule.locked {
        return Section::Fixed;
    }
    match (rule.effect, &rule.key, &rule.field) {
        (Effect::Deny, _, _) => Section::Never,
        (Effect::Ask, _, _) => Section::AsksFirst,
        (Effect::Allow, _, _) if rule.money.is_some() => Section::Money,
        (Effect::Allow, RuleKey::Capability(_), Some(RuleField::Folder(_))) => Section::Folders,
        (Effect::Allow, RuleKey::Capability(_), None) => Section::Job,
        (Effect::Allow, _, _) => Section::AlwaysAllowed,
    }
}

/// The rules a page shows: the scope's own, and on an employee's page the
/// company defaults it doesn't override (a rule of its own on the same key
/// and field, or any folder of its own for the company's folders).
fn shown_rules(store: &db::Store, agent_id: Option<&str>) -> Result<Vec<(Rule, bool)>, NeboError> {
    let own = store.permission_rules_in(&scope_of(agent_id))?;
    let mut shown: Vec<(Rule, bool)> = own.iter().cloned().map(|r| (r, false)).collect();
    if agent_id.is_some() {
        let own_folders = own.iter().any(|r| section_of(r) == Section::Folders);
        for r in store.permission_rules_in(&Scope::Company)? {
            let overridden = own.iter().any(|o| o.key == r.key && o.field == r.field)
                || (own_folders && section_of(&r) == Section::Folders);
            if !overridden {
                shown.push((r, true));
            }
        }
    }
    Ok(shown)
}

/// Build one page: an employee's (`Some`) or the company defaults (`None`).
fn page(store: &db::Store, agent_id: Option<&str>, connected: &BTreeSet<String>) -> Result<PermissionsPage, NeboError> {
    let company_mode = store.permission_mode(&Scope::Company)?.unwrap_or_default();
    let (mode, mode_from_company) = match agent_id {
        Some(id) => match store.permission_mode(&Scope::Employee(id.to_string()))? {
            Some(m) => (m, false),
            None => (company_mode, true),
        },
        None => (company_mode, false),
    };
    let mut p = PermissionsPage {
        mode: mode.as_str().to_string(),
        mode_from_company,
        company_mode: company_mode.as_str().to_string(),
        job: Vec::new(),
        can_add: Vec::new(),
        money: Vec::new(),
        folders: Vec::new(),
        always_allowed: Vec::new(),
        asks_first: Vec::new(),
        never: Vec::new(),
        fixed: Vec::new(),
    };
    let shown = shown_rules(store, agent_id)?;
    let mut in_job: BTreeSet<String> = BTreeSet::new();
    for (rule, from_company) in &shown {
        let section = section_of(rule);
        if section == Section::Job {
            in_job.insert(rule.key.value().to_string());
        }
        // A company job item leaves one employee's job from its page; every
        // other company default changes on the company page.
        let removable = !rule.locked && (!from_company || section == Section::Job);
        let item = PermissionItem {
            id: rule.id.clone(),
            sentence: item_sentence(rule, section),
            removable,
            from_company: *from_company,
            money: rule.money.as_ref().map(MoneyAmounts::from),
        };
        match section {
            Section::Job => p.job.push(item),
            Section::Money => p.money.push(item),
            Section::Folders => p.folders.push(item),
            Section::AlwaysAllowed => p.always_allowed.push(item),
            Section::AsksFirst => p.asks_first.push(item),
            Section::Never => p.never.push(item),
            Section::Fixed => p.fixed.push(item),
        }
    }
    for list in [&mut p.job, &mut p.money, &mut p.folders, &mut p.always_allowed, &mut p.asks_first, &mut p.never, &mut p.fixed] {
        list.sort_by(|a, b| a.sentence.cmp(&b.sentence));
    }
    for cap in builtin_capabilities().map(str::to_string).chain(connected.iter().cloned()) {
        if !in_job.contains(&cap) && !p.can_add.iter().any(|i| i.id == cap) {
            p.can_add.push(PermissionItem {
                sentence: capability_phrase(&cap),
                id: cap,
                removable: false,
                from_company: false,
                money: None,
            });
        }
    }
    Ok(p)
}

fn owner_rule(scope: Scope, key: RuleKey, field: Option<RuleField>, effect: Effect, source: RuleSource) -> Rule {
    Rule {
        id: uuid::Uuid::new_v4().to_string(),
        scope,
        key,
        field,
        effect,
        money: None,
        source,
        locked: false,
        created_at: chrono::Utc::now().timestamp(),
    }
}

/// Apply a page's change, as the owner.
fn update(
    store: &db::Store,
    agent_id: Option<&str>,
    body: &PermissionsUpdate,
    connected: &BTreeSet<String>,
) -> Result<(), NeboError> {
    let scope = scope_of(agent_id);
    if let Some(mode) = body.mode.as_deref() {
        match (mode, agent_id) {
            ("company", Some(_)) => store.clear_permission_mode(&scope)?,
            (m, _) => {
                let mode = Mode::parse(m).ok_or_else(|| NeboError::Validation(format!("unknown mode: {m}")))?;
                store.set_permission_mode(&scope, mode)?;
            }
        }
    }
    if let Some(cap) = body.add_capability.as_deref() {
        if !builtin_capabilities().any(|c| c == cap) && !connected.contains(cap) {
            return Err(NeboError::Validation("that can't be added to the job".into()));
        }
        let rule = owner_rule(scope.clone(), RuleKey::Capability(cap.to_string()), None, Effect::Allow, RuleSource::JobEdit);
        store.write_permission_rule(&rule, &Writer::Owner).map_err(rule_error)?;
    }
    if let Some(folder) = body.add_folder.as_deref() {
        let folder = folder.trim();
        if folder.is_empty() || !FsPath::new(folder).is_absolute() {
            return Err(NeboError::Validation("choose a folder".into()));
        }
        let rule = owner_rule(
            scope.clone(),
            RuleKey::Capability("file".into()),
            Some(RuleField::Folder(folder.into())),
            Effect::Allow,
            RuleSource::Owner,
        );
        store.write_permission_rule(&rule, &Writer::Owner).map_err(rule_error)?;
    }
    if let Some(edit) = &body.money {
        let rule = store.get_permission_rule(&edit.id)?.ok_or(NeboError::NotFound)?;
        if rule.scope != scope || section_of(&rule) != Section::Money {
            return Err(NeboError::NotFound);
        }
        let money = MoneyLimit::from(&edit.amounts);
        store
            .write_permission_rule(&Rule { money: Some(money), ..rule }, &Writer::Owner)
            .map_err(rule_error)?;
    }
    Ok(())
}

/// Remove one item, as the owner. On an employee's page a job item leaves
/// that employee's job: its own rule is deleted, and when the company
/// default still includes the capability the employee is set to ask first
/// for it, the way a capability outside its job asks.
fn remove(store: &db::Store, agent_id: Option<&str>, rule_id: &str) -> Result<(), NeboError> {
    let rule = store.get_permission_rule(rule_id)?.ok_or(NeboError::NotFound)?;
    let scope = scope_of(agent_id);
    if rule.scope == scope {
        let company_includes = agent_id.is_some()
            && section_of(&rule) == Section::Job
            && store
                .permission_rules_in(&Scope::Company)?
                .iter()
                .any(|c| c.key == rule.key && c.field.is_none() && section_of(c) == Section::Job);
        if company_includes {
            let ask = owner_rule(scope, rule.key.clone(), None, Effect::Ask, RuleSource::JobEdit);
            store.write_permission_rule(&ask, &Writer::Owner).map_err(rule_error)?;
            return Ok(());
        }
        return store.remove_permission_rule(rule_id, &Writer::Owner).map_err(rule_error);
    }
    match (&rule.scope, agent_id) {
        (Scope::Company, Some(_)) if section_of(&rule) == Section::Job => {
            let ask = owner_rule(scope, rule.key.clone(), None, Effect::Ask, RuleSource::JobEdit);
            store.write_permission_rule(&ask, &Writer::Owner).map_err(rule_error)?;
            Ok(())
        }
        (Scope::Company, Some(_)) => Err(NeboError::Validation("change this in the company defaults".into())),
        _ => Err(NeboError::NotFound),
    }
}

// ── Activity ────────────────────────────────────────────────────────────

fn activity(store: &db::Store, q: &ActivityQuery) -> Result<ActivityPage, NeboError> {
    let filter = db::PermissionActivityFilter {
        agent_id: q.agent_id.clone().filter(|s| !s.is_empty()),
        door: q.door.clone().filter(|s| !s.is_empty()),
        decision: q.decision.clone().filter(|s| !s.is_empty()),
        limit: q.limit.unwrap_or(50).clamp(1, 200),
        offset: q.offset.unwrap_or(0).max(0),
    };
    let (rows, total) = store.permission_activity(&filter)?;
    let mut names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut name_of = |id: &str| -> String {
        if let Some(n) = names.get(id) {
            return n.clone();
        }
        let lookup = if id.is_empty() { "assistant" } else { id };
        let name = store
            .get_agent(lookup)
            .ok()
            .flatten()
            .map(|a| a.name)
            .unwrap_or_else(|| "An employee".to_string());
        names.insert(id.to_string(), name.clone());
        name
    };
    let rows = rows
        .into_iter()
        .map(|r| {
            let (why, unreviewed) = why_sentence(store, &r.decision, &r.why);
            ActivityRow {
                at: r.created_at,
                employee: name_of(&r.agent_id),
                employee_id: r.agent_id,
                action: action_sentence(&r.activity, &r.rule_key),
                decision: r.decision,
                why,
                door: r.door,
                unreviewed,
            }
        })
        .collect();
    Ok(ActivityPage { rows, total })
}

fn action_sentence(activity: &str, key: &str) -> String {
    let a = activity.trim();
    if a.is_empty() { key_phrase(key) } else { sentence_case(a) }
}

/// Why a recorded decision went the way it did, and whether it ran
/// unreviewed.
fn why_sentence(store: &db::Store, decision: &str, why: &str) -> (String, bool) {
    if decision == "ask" {
        return match serde_json::from_str::<AskCase>(why) {
            Ok(case) => (ask_sentence(store, &case), false),
            Err(_) => ("It asked you first".into(), false),
        };
    }
    let Ok(why) = serde_json::from_str::<Why>(why) else {
        return ("Recorded before reasons were kept".into(), false);
    };
    let s = match why {
        Why::Rule { rule_id } => match store.get_permission_rule(&rule_id).ok().flatten() {
            Some(rule) => format!("{}: {}", source_words(&rule), item_sentence(&rule, Section::Fixed)),
            None => "A setting that has since been removed".into(),
        },
        Why::Mode { mode: Mode::FullAccess } => "Full Access is on, so nothing asks".into(),
        Why::Mode { mode: Mode::Plan } => "Plan mode: it can look and plan but not change anything".into(),
        Why::Mode { mode: Mode::Ask } => "Ask mode: it asks before changing anything".into(),
        Why::Mode { mode: Mode::Automatic } => "Inside its job".into(),
        Why::BasicWork => "Everyday work every employee does, like notes, tasks and memory".into(),
        Why::AnsweredOnce { .. } => "You allowed it once".into(),
        Why::Declined { .. } => "You already said no to this".into(),
        Why::Judged { reason, .. } => format!("Reviewed automatically: {}", reason.trim_end_matches('.')),
        Why::Unreviewed { .. } => return ("The permission check couldn't run, so this wasn't reviewed".into(), true),
        Why::HardLimit { limit } => match limit.as_str() {
            "safeguard" => "A safety limit blocks this kind of action".into(),
            "origin" => "Someone outside your company started this, so it can only reply".into(),
            "credentials" => "It would have shared a password or key".into(),
            _ => "A safety limit".into(),
        },
        Why::Ceiling => "It can't do more than the employee or run it works for".into(),
    };
    (s, false)
}

fn ask_sentence(store: &db::Store, case: &AskCase) -> String {
    match case {
        AskCase::Money { cents, .. } => format!("It would spend {}, more than its limit", dollars(*cents)),
        AskCase::NewCounterparty { who } => format!("A first message to {who}"),
        AskCase::Irreversible { what } => format!("It would delete or overwrite {what}, which it didn't make"),
        AskCase::OutsideJob { capability } => {
            format!("Not part of its job: {}", lower_first(&key_or_capability_phrase(capability)))
        }
        AskCase::UntrustedInput { source } => {
            format!("It read something from outside ({source}) and then wanted to act on it")
        }
        AskCase::AskRule { rule_id } => match store.get_permission_rule(rule_id).ok().flatten() {
            Some(rule) => format!("Set to ask first: {}", lower_first(&rule_phrase(&rule))),
            None => "Set to ask first".into(),
        },
        AskCase::AskMode => "Ask mode: it asks before changing anything".into(),
        AskCase::Widens => "It would give an employee more room, which only you can do".into(),
        AskCase::CreatedExtras { capabilities } => {
            let needs: Vec<String> = capabilities.iter().map(|c| lower_first(&capability_phrase(c))).collect();
            format!("An employee it made needs more than it holds: {}", needs.join(", "))
        }
    }
}

/// Who or what put a rule in place, in the owner's words.
fn source_words(rule: &Rule) -> &'static str {
    match &rule.source {
        RuleSource::Hire { .. } => "Part of the job you agreed to when you hired it",
        RuleSource::Created { .. } => "Part of the job you agreed to when you created it",
        RuleSource::JobEdit => "You changed its job",
        RuleSource::AllowAlways { .. } => "You answered \u{201c}Allow always\u{201d}",
        RuleSource::Owner if rule.scope == Scope::Company => "Your company default",
        RuleSource::Owner => "Your setting",
        RuleSource::Package { .. } => "Set when it was hired",
        RuleSource::Law { .. } => "A company rule that can't change",
        RuleSource::Migrated { .. } => "Carried over from your earlier settings",
    }
}

// ── Sentences ───────────────────────────────────────────────────────────

/// The sentence one rule shows as in its section.
fn item_sentence(rule: &Rule, section: Section) -> String {
    let phrase = rule_phrase(rule);
    match section {
        Section::Folders => match &rule.field {
            Some(RuleField::Folder(p)) => format!("Change files in {}", folder_words(p)),
            _ => phrase,
        },
        Section::Money => {
            let limits = rule.money.as_ref().map(money_words).unwrap_or_default();
            if limits.is_empty() { phrase } else { format!("{phrase}, {limits}") }
        }
        Section::Fixed => match rule.effect {
            Effect::Deny => format!("Never: {}", lower_first(&phrase)),
            Effect::Ask => format!("Always asks first: {}", lower_first(&phrase)),
            Effect::Allow => phrase,
        },
        _ => phrase,
    }
}

/// A rule's key and field as one phrase ("Run commands that start with …").
fn rule_phrase(rule: &Rule) -> String {
    let base = match &rule.key {
        RuleKey::Capability(c) => capability_phrase(c),
        RuleKey::Operation(op) => operation_phrase(op),
        RuleKey::Tool(t) => key_phrase(t),
    };
    match &rule.field {
        None => base,
        Some(RuleField::CommandPrefix(p)) => format!("{base} that start with \u{201c}{p}\u{201d}"),
        Some(RuleField::Folder(p)) => format!("{base} in {}", folder_words(p)),
        Some(RuleField::Domain(d)) => format!("{base} on {d}"),
        Some(RuleField::Recipient(r)) => format!("{base} to {r}"),
    }
}

/// What a job capability lets an employee do.
fn capability_phrase(c: &str) -> String {
    let s = match c {
        "file" => "Read and change files",
        "shell" => "Run commands on this computer",
        "web" => "Look things up and open pages on the web",
        "browser" => "Use a web browser",
        "contacts" => "Use your contacts",
        "desktop" => "Control the screen, mouse and keyboard",
        "media" => "Use the camera, microphone and screen recording",
        "system" => "Read and change this computer's settings",
        "ledger" => "Work in your accounting",
        "payments" => "Take and send payments",
        "billing" => "Handle billing",
        "expense" => "Handle expenses",
        "mail" => "Read and send email",
        "sms" => "Read and send text messages",
        "telephony" => "Make and answer phone calls",
        "calendar" => "Manage your calendar",
        "meetings" => "Set up meetings",
        "esign" => "Send documents for signature",
        "crm" => "Work in your customer records",
        "deals" => "Work on deals",
        "email-marketing" => "Send email campaigns",
        "social" => "Post and reply on social media",
        "cms" => "Edit your website",
        "reviews" => "Answer reviews",
        "ads" => "Manage advertising",
        "helpdesk" => "Answer support requests",
        "kb" => "Edit your help articles",
        "store" => "Manage your store and orders",
        "shipping" => "Arrange shipping",
        "drive" => "Use your shared files",
        "projects" => "Work on projects",
        "tickets" => "Work on project issues",
        "hris" => "Work in your people records",
        "ats" => "Work in your hiring records",
        "layers" => "Change the company's own files",
        "authority" => "Act on standing authority you gave it",
        _ => return format!("Work with {}", words(c)),
    };
    s.to_string()
}

/// A rule key that may be a capability or a tool name.
fn key_or_capability_phrase(k: &str) -> String {
    if builtin_capabilities().any(|c| c == k) || !k.contains(['_', '.', '*']) {
        capability_phrase(k)
    } else if k.contains('.') {
        operation_phrase(k)
    } else {
        key_phrase(k)
    }
}

/// What a tool key does, in words. A trailing `*` names a family of tools.
fn key_phrase(key: &str) -> String {
    let family = key.strip_suffix('*');
    let k = family.unwrap_or(key);
    if let Some(rest) = k.strip_prefix("mcp__") {
        let (slug, tool) = rest.split_once("__").unwrap_or((rest, ""));
        let service = tools::humanize::service_name(slug.trim_end_matches('_'));
        return if tool.is_empty() {
            format!("Use {service}")
        } else {
            format!("Use {service} to {}", words(tool))
        };
    }
    let s = match k.trim_end_matches('_') {
        "run_command" | "shell" | "bash" => "Run commands",
        "read_file" | "read" => "Read files",
        "write_file" | "edit_file" | "notebook_edit" | "write" | "edit" => "Change files",
        "web_search" => "Search the web",
        "fetch_url" | "web_fetch" | "http_request" => "Open web pages",
        "desktop" | "window" | "ui" | "menu" | "dialog" | "space" | "shortcut" => "Control the screen",
        "desktop_click" | "desktop_move_mouse" | "desktop_drag" | "desktop_scroll" => "Use the mouse",
        "desktop_key" | "desktop_type" | "desktop_paste" => "Type on the keyboard",
        "browser" => "Use the web browser",
        _ => return sentence_case(&words(k)),
    };
    s.to_string()
}

/// "ledger.billpayment.create" → "Create bill payment". A two-part address
/// is verb-first ("spend.above_company_bounds").
fn operation_phrase(op: &str) -> String {
    let parts: Vec<&str> = op.split('.').filter(|p| !p.is_empty()).collect();
    let (resource, action) = match parts.as_slice() {
        [] => return String::new(),
        [only] => return sentence_case(&words(only)),
        [verb, rest] => (*rest, *verb),
        [.., resource, action] => (*resource, *action),
    };
    let verb = match action {
        "create" => "Create".to_string(),
        "send" => "Send".to_string(),
        "update" => "Update".to_string(),
        "status" => "Change the status of".to_string(),
        "apply" => "Apply".to_string(),
        "record" => "Record".to_string(),
        "schedule" => "Schedule".to_string(),
        "publish" => "Publish".to_string(),
        "respond" => "Respond to".to_string(),
        "reply" => "Reply to".to_string(),
        "upsert" => "Save".to_string(),
        "attach" => "Attach".to_string(),
        "write" => "Write".to_string(),
        "remove" | "delete" => "Delete".to_string(),
        "void" => "Void".to_string(),
        other => sentence_case(&words(other)),
    };
    let noun = match resource {
        "billpayment" => "bill payments".to_string(),
        "creditmemo" => "credit memos".to_string(),
        "journalentry" => "journal entries".to_string(),
        "purchaseorder" | "po" => "purchase orders".to_string(),
        "opportunity" => "deals".to_string(),
        "company" => "the company file".to_string(),
        "industry" => "the industry file".to_string(),
        "franchise" => "the franchise file".to_string(),
        other => words(other),
    };
    format!("{verb} {noun}").trim().to_string()
}

fn money_words(m: &MoneyLimit) -> String {
    let mut parts = Vec::new();
    if let Some(c) = m.per_action_cents {
        parts.push(format!("up to {} each time", dollars(c)));
    }
    if let Some(c) = m.per_day_cents {
        parts.push(format!("up to {} a day", dollars(c)));
    }
    if let Some(n) = m.per_day_count {
        parts.push(if n == 1 { "once a day".to_string() } else { format!("{n} times a day") });
    }
    if let Some(c) = m.per_counterparty_day_cents {
        parts.push(format!("up to {} a day to any one recipient", dollars(c)));
    }
    parts.join(", ")
}

fn dollars(cents: i64) -> String {
    if cents % 100 == 0 { format!("${}", cents / 100) } else { format!("${}.{:02}", cents / 100, cents % 100) }
}

/// A folder as the owner knows it: the home folder written `~`.
fn folder_words(p: &FsPath) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = p.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "your home folder".to_string()
        } else {
            format!("~/{}", rest.to_string_lossy())
        };
    }
    p.to_string_lossy().into_owned()
}

fn words(s: &str) -> String {
    s.split(['_', '-', '.'])
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn sentence_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, db::Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = db::Store::new(&dir.path().join("perm.db").to_string_lossy()).expect("store");
        (dir, store)
    }

    fn put(store: &db::Store, scope: Scope, key: RuleKey, field: Option<RuleField>, effect: Effect, source: RuleSource) -> Rule {
        store
            .write_permission_rule(&owner_rule(scope, key, field, effect, source), &Writer::Owner)
            .unwrap()
    }

    fn emp() -> Scope {
        Scope::Employee("a".into())
    }

    fn cap(c: &str) -> RuleKey {
        RuleKey::Capability(c.into())
    }

    /// Every string a page or an activity row sends, but ids.
    fn strings(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
            serde_json::Value::Object(o) => o
                .iter()
                .filter(|(k, _)| !k.ends_with("id") && !k.ends_with("Id") && *k != "mode" && *k != "decision" && *k != "door")
                .for_each(|(_, x)| strings(x, out)),
            _ => {}
        }
    }

    #[test]
    fn no_rule_string_reaches_the_client() {
        let (_d, store) = store();
        put(&store, Scope::Company, cap("shell"), None, Effect::Allow, RuleSource::Migrated { from: "x".into() });
        put(&store, emp(), RuleKey::Tool("run_command".into()), Some(RuleField::CommandPrefix("git status".into())), Effect::Allow, RuleSource::AllowAlways { ask_id: "k".into() });
        put(&store, emp(), RuleKey::Operation("mail.message.send".into()), Some(RuleField::Recipient("pat@example.com".into())), Effect::Allow, RuleSource::Owner);
        put(&store, emp(), RuleKey::Tool("mcp__acme_crm__search_contacts".into()), None, Effect::Ask, RuleSource::Owner);
        put(&store, emp(), RuleKey::Tool("desktop_*".into()), None, Effect::Deny, RuleSource::Owner);
        put(&store, emp(), cap("file"), Some(RuleField::Folder("/srv/work".into())), Effect::Allow, RuleSource::Owner);
        let money = Rule {
            money: Some(MoneyLimit { per_day_cents: Some(5000), ..Default::default() }),
            ..owner_rule(emp(), RuleKey::Operation("ledger.billpayment.create".into()), None, Effect::Allow, RuleSource::Owner)
        };
        store.write_permission_rule(&money, &Writer::Owner).unwrap();
        let law = Rule {
            locked: true,
            ..owner_rule(emp(), RuleKey::Operation("ledger.invoice.void".into()), None, Effect::Deny, RuleSource::Law { pack: "p".into() })
        };
        store.write_permission_rule(&law, &Writer::Package { package: "p".into() }).unwrap();

        let page = serde_json::to_value(page(&store, Some("a"), &BTreeSet::from(["mail".to_string()])).unwrap()).unwrap();
        let mut out = Vec::new();
        strings(&page, &mut out);
        assert!(out.len() >= 9, "{page}");
        let forbidden = [
            "run_command", "mail.message.send", "mcp__", "desktop_", "command_prefix", "capability",
            "ledger.", "billpayment", "_", "{", "\"kind\"",
        ];
        for s in &out {
            for f in forbidden {
                assert!(!s.contains(f), "{s:?} carries {f:?}");
            }
        }
        assert!(out.contains(&"Run commands that start with \u{201c}git status\u{201d}".to_string()), "{out:?}");
        assert!(out.contains(&"Send message to pat@example.com".to_string()), "{out:?}");
        assert!(out.contains(&"Create bill payments, up to $50 a day".to_string()), "{out:?}");
        assert!(out.contains(&"Never: void invoice".to_string()), "{out:?}");
    }

    #[test]
    fn removing_a_job_item_deletes_its_rule() {
        let (_d, store) = store();
        let own = put(&store, emp(), cap("web"), None, Effect::Allow, RuleSource::Owner);
        remove(&store, Some("a"), &own.id).unwrap();
        assert!(store.get_permission_rule(&own.id).unwrap().is_none());
        let p = page(&store, Some("a"), &BTreeSet::new()).unwrap();
        assert!(p.job.is_empty());
        assert!(p.can_add.iter().any(|i| i.id == "web"), "it can be added back");

        // A job item from the company defaults leaves this employee's job
        // only: it asks first, the company default stays.
        let company = put(&store, Scope::Company, cap("shell"), None, Effect::Allow, RuleSource::Owner);
        let p = page(&store, Some("a"), &BTreeSet::new()).unwrap();
        let item = p.job.iter().find(|i| i.id == company.id).unwrap();
        assert!(item.removable && item.from_company);
        remove(&store, Some("a"), &company.id).unwrap();
        assert!(store.get_permission_rule(&company.id).unwrap().is_some());
        let p = page(&store, Some("a"), &BTreeSet::new()).unwrap();
        assert!(p.job.is_empty());
        assert_eq!(p.asks_first.len(), 1);
        assert!(page(&store, Some("b"), &BTreeSet::new()).unwrap().job.iter().any(|i| i.id == company.id));

        // Other company defaults change on the company page only; locked
        // rules never.
        let default = put(&store, Scope::Company, RuleKey::Tool("run_command".into()), None, Effect::Ask, RuleSource::Owner);
        assert!(matches!(remove(&store, Some("a"), &default.id), Err(NeboError::Validation(_))));
        remove(&store, None, &default.id).unwrap();
        let law = Rule { locked: true, ..owner_rule(emp(), cap("desktop"), None, Effect::Deny, RuleSource::Law { pack: "p".into() }) };
        let law = store.write_permission_rule(&law, &Writer::Package { package: "p".into() }).unwrap();
        assert!(!page(&store, Some("a"), &BTreeSet::new()).unwrap().fixed[0].removable);
        assert!(remove(&store, Some("a"), &law.id).is_err());
        // Another employee's rule is not this page's.
        let other = put(&store, Scope::Employee("b".into()), cap("web"), None, Effect::Allow, RuleSource::Owner);
        assert!(matches!(remove(&store, Some("a"), &other.id), Err(NeboError::NotFound)));
    }

    #[test]
    fn adding_to_the_job_and_folders_writes_the_owners_rules() {
        let (_d, store) = store();
        let connected = BTreeSet::from(["mail".to_string()]);
        let add = |cap: &str| PermissionsUpdate { add_capability: Some(cap.into()), ..Default::default() };
        update(&store, Some("a"), &add("mail"), &connected).unwrap();
        update(&store, Some("a"), &add("shell"), &connected).unwrap();
        assert!(update(&store, Some("a"), &add("ledger"), &connected).is_err(), "nothing connected provides it");
        let folder = PermissionsUpdate { add_folder: Some("/srv/work".into()), ..Default::default() };
        update(&store, Some("a"), &folder, &connected).unwrap();
        assert!(update(&store, Some("a"), &PermissionsUpdate { add_folder: Some("relative".into()), ..Default::default() }, &connected).is_err());
        let p = page(&store, Some("a"), &connected).unwrap();
        let job: Vec<&str> = p.job.iter().map(|i| i.sentence.as_str()).collect();
        assert_eq!(job, ["Read and send email", "Run commands on this computer"]);
        assert_eq!(p.folders[0].sentence, "Change files in /srv/work");
        assert!(!p.can_add.iter().any(|i| i.id == "mail" || i.id == "shell"), "what the job holds is not offered again");
    }

    #[test]
    fn money_amounts_are_edited_in_place() {
        let (_d, store) = store();
        let rule = Rule {
            money: Some(MoneyLimit { per_day_cents: Some(5000), ..Default::default() }),
            ..owner_rule(emp(), RuleKey::Operation("payments.payment.send".into()), None, Effect::Allow, RuleSource::Owner)
        };
        let rule = store.write_permission_rule(&rule, &Writer::Owner).unwrap();
        let edit = PermissionsUpdate {
            money: Some(MoneyEdit {
                id: rule.id.clone(),
                amounts: MoneyAmounts { per_action_cents: Some(2500), per_day_cents: Some(10000), ..Default::default() },
            }),
            ..Default::default()
        };
        update(&store, Some("a"), &edit, &BTreeSet::new()).unwrap();
        let p = page(&store, Some("a"), &BTreeSet::new()).unwrap();
        assert_eq!(p.money[0].sentence, "Send payment, up to $25 each time, up to $100 a day");
        assert_eq!(p.money[0].money.as_ref().unwrap().per_day_cents, Some(10000));
    }

    #[test]
    fn mode_picker_writes_the_mode() {
        let (_d, store) = store();
        let set = |agent: Option<&str>, m: &str| {
            update(&store, agent, &PermissionsUpdate { mode: Some(m.into()), ..Default::default() }, &BTreeSet::new())
        };
        let p = page(&store, Some("a"), &BTreeSet::new()).unwrap();
        assert_eq!((p.mode.as_str(), p.mode_from_company), ("automatic", true));
        set(Some("a"), "plan").unwrap();
        assert_eq!(store.permission_mode(&emp()).unwrap(), Some(Mode::Plan));
        let p = page(&store, Some("a"), &BTreeSet::new()).unwrap();
        assert_eq!((p.mode.as_str(), p.mode_from_company, p.company_mode.as_str()), ("plan", false, "automatic"));
        set(Some("a"), "company").unwrap();
        assert_eq!(store.permission_mode(&emp()).unwrap(), None);
        set(None, "full_access").unwrap();
        assert_eq!(page(&store, Some("a"), &BTreeSet::new()).unwrap().mode, "full_access");
        assert!(set(None, "company").is_err(), "the company has no default above it");
        assert!(set(Some("a"), "everything").is_err());
    }

    #[test]
    fn company_default_applies_until_overridden() {
        let (_d, store) = store();
        let company = put(&store, Scope::Company, cap("web"), None, Effect::Allow, RuleSource::Owner);
        let folder = put(&store, Scope::Company, cap("file"), Some(RuleField::Folder("/srv/shared".into())), Effect::Allow, RuleSource::Owner);
        let p = page(&store, Some("a"), &BTreeSet::new()).unwrap();
        assert!(p.job.iter().any(|i| i.id == company.id && i.from_company));
        assert!(p.folders.iter().any(|i| i.id == folder.id && i.from_company && !i.removable));
        put(&store, emp(), cap("web"), None, Effect::Deny, RuleSource::Owner);
        put(&store, emp(), cap("file"), Some(RuleField::Folder("/srv/own".into())), Effect::Allow, RuleSource::Owner);
        let p = page(&store, Some("a"), &BTreeSet::new()).unwrap();
        assert!(p.job.is_empty(), "the employee's own rule decides");
        assert_eq!(p.never.len(), 1);
        assert_eq!(p.folders.iter().map(|i| i.sentence.as_str()).collect::<Vec<_>>(), ["Change files in /srv/own"]);
        // The company page shows only the company's own.
        let c = page(&store, None, &BTreeSet::new()).unwrap();
        assert_eq!((c.job.len(), c.folders.len(), c.never.len()), (1, 1, 0));
        assert!(c.job.iter().all(|i| !i.from_company && i.removable));
    }

    #[test]
    fn activity_why_names_rule_consent_or_answer() {
        let (_d, store) = store();
        let hired = put(&store, emp(), cap("mail"), None, Effect::Allow, RuleSource::Hire { package: "p".into() });
        let answered = put(&store, emp(), RuleKey::Tool("run_command".into()), Some(RuleField::CommandPrefix("git".into())), Effect::Allow, RuleSource::AllowAlways { ask_id: "k".into() });
        let record = |decision: &str, why: String, activity: &str| {
            store
                .record_permission_activity(&db::PermissionActivityRow {
                    agent_id: "a".into(),
                    door: "chat".into(),
                    tool: "run_command".into(),
                    rule_key: "run_command".into(),
                    activity: activity.into(),
                    decision: decision.into(),
                    why,
                    created_at: 1,
                    ..Default::default()
                })
                .unwrap()
        };
        let why = |w: Why| serde_json::to_string(&w).unwrap();
        record("allow", why(Why::Rule { rule_id: hired.id.clone() }), "sending an email");
        record("allow", why(Why::Rule { rule_id: answered.id.clone() }), "running git status");
        record("allow", why(Why::AnsweredOnce { ask_id: "k2".into() }), "");
        record("allow", why(Why::Unreviewed { reason: "both judges down".into() }), "posting a form");
        record("ask", serde_json::to_string(&AskCase::OutsideJob { capability: "shell".into() }).unwrap(), "running ls");
        record("deny", why(Why::HardLimit { limit: "origin".into() }), "running rm");

        let page = activity(&store, &ActivityQuery { agent_id: Some("a".into()), ..Default::default() }).unwrap();
        assert_eq!(page.total, 6);
        let whys: Vec<&str> = page.rows.iter().rev().map(|r| r.why.as_str()).collect();
        assert_eq!(whys[0], "Part of the job you agreed to when you hired it: Read and send email");
        assert_eq!(whys[1], "You answered \u{201c}Allow always\u{201d}: Run commands that start with \u{201c}git\u{201d}");
        assert_eq!(whys[2], "You allowed it once");
        assert!(page.rows.iter().rev().nth(3).unwrap().unreviewed);
        assert_eq!(whys[4], "Not part of its job: run commands on this computer");
        assert_eq!(whys[5], "Someone outside your company started this, so it can only reply");
        assert_eq!(page.rows.last().unwrap().action, "Sending an email");
        assert_eq!(page.rows.iter().rev().nth(2).unwrap().action, "Run commands", "no label: the key in words");
        let only_asks = activity(&store, &ActivityQuery { decision: Some("ask".into()), ..Default::default() }).unwrap();
        assert_eq!(only_asks.total, 1);
    }
}
