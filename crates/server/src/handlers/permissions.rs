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

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path as FsPath;

use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};

use agent::harness::permissions::{Answer, AnsweredVia, Ask, AskError, AskStatus};
use types::permissions::{
    AskCase, Effect, JudgementMode, Mode, MoneyLimit, Rule, RuleError, RuleField, RuleKey, RuleSource, Scope, Why,
    Writer,
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
    /// permission (Allow always · This once · No) | send_check (a send whose
    /// outcome never came back: It went out · It didn't go out).
    pub kind: String,
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
    /// open | allowed | declined | withdrawn (the work that waited on it
    /// ended without it) | answered (a send_check card: `answer` says
    /// whether it went out). An open ask stays open until it is answered.
    pub status: String,
    /// allow_always | this_once | no | sent | not_sent, once answered.
    pub answer: Option<String>,
    pub created_at: i64,
}

/// The card for `ask`.
pub(crate) fn card(state: &AppState, ask: &Ask) -> PermissionAskCard {
    let (status, answer) = match ask.status {
        AskStatus::Open => ("open", None),
        AskStatus::Answered { answer: Answer::No, .. } => ("declined", Some(Answer::No)),
        AskStatus::Answered { answer: answer @ (Answer::Sent | Answer::NotSent), .. } => ("answered", Some(answer)),
        AskStatus::Answered { answer, .. } => ("allowed", Some(answer)),
        AskStatus::Withdrawn => ("withdrawn", None),
    };
    PermissionAskCard {
        id: ask.id.clone(),
        kind: ask.kind().as_str().to_string(),
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
    }
}

/// The employee's name; the main assistant goes by the bot's own name.
pub(crate) fn employee_name(state: &AppState, agent_id: &str) -> String {
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
        AskError::NotOffered => NeboError::Validation("that answer doesn't fit this question".into()),
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
    /// allow_always | this_once | no; sent | not_sent for a send_check card
    pub answer: String,
    /// chat | inbox | mobile | notification
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
    let answer = Answer::parse(&body.answer)
        .ok_or_else(|| invalid("answer must be allow_always, this_once, no, sent or not_sent"))?;
    // A spoken answer comes only from the owner's own call (`voice.rs`),
    // never from a client claiming one.
    let via = AnsweredVia::parse(&body.via)
        .filter(|v| *v != AnsweredVia::Voice)
        .ok_or_else(|| invalid("via must be chat, inbox, mobile or notification"))?;
    match state.permission_asks.answer(&id, answer, via) {
        Ok(ask) => Ok(Json(card(&state, &ask))),
        Err(AskError::Settled(ask)) => Ok(Json(card(&state, &ask))),
        Err(e) => Err(ask_error(e)),
    }
}

// ── The phone as it shipped ─────────────────────────────────────────────
//
// The phone app released before nebo-mobile #68 answers a parked workflow
// run by its run id, on its `wf-approval:<run>` Inbox rows. A run now parks
// on an ask, so these two routes read and answer the ask that holds the run,
// through the ask's one answer path; nothing else is kept. #68 moves the
// phone to the ask routes above and ships in the same release train.

/// A parked run's standing in the words the shipped phone reads: pending
/// earns its Approve and Deny buttons.
fn run_approval_status(ask: &Ask) -> &'static str {
    match ask.status {
        AskStatus::Open => "pending",
        AskStatus::Answered {
            answer: Answer::No, ..
        } => "denied",
        AskStatus::Answered { .. } => "approved",
        AskStatus::Withdrawn => "withdrawn",
    }
}

/// GET /api/v1/agents/workflow-runs/{run_id}/approval — where the ask a
/// workflow run parked on stands.
pub async fn get_workflow_run_approval(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let status = state
        .permission_asks
        .for_run(&run_id)
        .map_err(ask_error)?
        .map_or("unknown", |a| run_approval_status(&a));
    Ok(Json(serde_json::json!({ "status": status })))
}

#[derive(Debug, Deserialize)]
pub struct WorkflowRunApprovalBody {
    pub approved: bool,
}

/// POST /api/v1/agents/workflow-runs/{run_id}/approval — Approve is "This
/// once", Deny is "No", on the ask the run is parked on. The first answer
/// anywhere wins; the status says which one it was.
pub async fn answer_workflow_run_approval(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(body): Json<WorkflowRunApprovalBody>,
) -> HandlerResult<serde_json::Value> {
    let ask = state
        .permission_asks
        .for_run(&run_id)
        .map_err(ask_error)?
        .ok_or_else(|| ask_error(AskError::NotFound))?;
    let answer = if body.approved {
        Answer::ThisOnce
    } else {
        Answer::No
    };
    let settled = match state.permission_asks.answer(&ask.id, answer, AnsweredVia::Mobile) {
        Ok(ask) => ask,
        Err(AskError::Settled(ask)) => *ask,
        Err(e) => return Err(ask_error(e)),
    };
    Ok(Json(
        serde_json::json!({ "status": run_approval_status(&settled), "runId": run_id }),
    ))
}

// ── The Permissions pages ───────────────────────────────────────────────

/// One employee's permissions, or the company defaults, in plain words.
/// Every choice on it is the same three-way switch: always allow, ask
/// first, or off.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsPage {
    /// automatic | ask | plan | full_access
    pub mode: String,
    /// The employee has no mode of its own and follows the company default.
    pub mode_from_company: bool,
    /// The company default mode.
    pub company_mode: String,
    /// What employees can do: one switch per built-in capability.
    pub capabilities: Vec<PermissionSwitch>,
    /// Each connected service (what a plugin lets employees do, or an MCP
    /// server): its default, and one switch per action or tool.
    pub groups: Vec<PermissionGroup>,
    /// Settings on one command, site, recipient or tool, from the owner's
    /// answers and settings.
    pub specific: Vec<PermissionSwitch>,
    /// Money it may spend without asking.
    pub money: Vec<PermissionItem>,
    /// Folders it may change files in.
    pub folders: Vec<PermissionItem>,
    /// Safety rules: these always ask the owner and have no switch.
    pub always_asks: Vec<PermissionItem>,
    /// Rules a company law or the employee's package sets; they can't change here.
    pub fixed: Vec<PermissionItem>,
}

/// One three-way choice on a Permissions page.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionSwitch {
    /// What `set` names.
    pub id: String,
    pub sentence: String,
    /// allow | ask | deny: what applies now.
    pub value: String,
    /// No setting of this page's own: `value` is the default's or the
    /// company's.
    pub inherited: bool,
    /// Where an inherited value comes from: `default` (its group's default,
    /// or the standing default) or `company`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inherits_from: Option<String>,
    /// Whether its own setting can be cleared, back to the default or the
    /// company's (`set` with `inherit`).
    pub can_inherit: bool,
    /// A company "off" binds it: it changes on the company page only.
    pub locked: bool,
}

/// A connected service's switches.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionGroup {
    pub id: String,
    /// The MCP server's name, or what the plugin lets employees do.
    pub title: String,
    /// The plugins that provide it; empty for an MCP server.
    pub subtitle: String,
    /// What everything in it does without a setting of its own.
    pub default: PermissionSwitch,
    /// One switch per action or tool.
    pub rows: Vec<PermissionSwitch>,
}

/// One line on a Permissions page without a switch.
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
    /// A folder the job may change files in.
    pub add_folder: Option<String>,
    /// New amounts for a money item.
    pub money: Option<MoneyEdit>,
    /// One switch moved.
    pub set: Option<SwitchEdit>,
}

/// A switch moved: its `id` and `allow`, `ask`, `deny`, or `inherit` (clear
/// this page's own setting, back to the default or the company's).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchEdit {
    pub id: String,
    pub value: String,
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
    /// Who decided it, in plain words: the permission rules, or the
    /// reviewer that judged what the rules could not (Jev, or the backup
    /// reviewer).
    pub decided_by: String,
    /// The reviewer's verdict and its reason, in plain words; empty when
    /// the rules decided alone.
    pub verdict: String,
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

/// The capabilities the company's active plugins provide, each with the
/// names of the plugins that provide it.
fn connected(state: &AppState) -> BTreeMap<String, Vec<String>> {
    super::agents::connected_capabilities(state)
        .into_iter()
        .map(|(capability, slugs)| {
            let names = slugs
                .iter()
                .map(|slug| {
                    state
                        .plugin_store
                        .get_manifest(slug)
                        .map(|m| m.name)
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or_else(|| tools::humanize::service_name(slug))
                })
                .collect();
            (capability, names)
        })
        .collect()
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
/// and field, or any folder of its own for the company's folders). A company
/// deny is always shown: no rule of the employee's undoes it.
fn shown_rules(store: &db::Store, agent_id: Option<&str>) -> Result<Vec<(Rule, bool)>, NeboError> {
    let own = store.permission_rules_in(&scope_of(agent_id))?;
    let mut shown: Vec<(Rule, bool)> = own.iter().cloned().map(|r| (r, false)).collect();
    if agent_id.is_some() {
        let own_folders = own.iter().any(|r| section_of(r) == Section::Folders);
        for r in store.permission_rules_in(&Scope::Company)? {
            let overridden = r.effect != Effect::Deny && own.iter().any(|o| o.key == r.key && o.field == r.field)
                || (own_folders && section_of(&r) == Section::Folders);
            if !overridden {
                shown.push((r, true));
            }
        }
    }
    Ok(shown)
}

/// Whether two rule keys name the same thing (an operation by its port
/// suffix, as the rules engine matches it).
fn same_key(a: &RuleKey, b: &RuleKey) -> bool {
    match (a, b) {
        (RuleKey::Operation(x), RuleKey::Operation(y)) => {
            tools::plugin_tool::port_suffix(x) == tools::plugin_tool::port_suffix(y)
        }
        _ => a == b,
    }
}

/// An operation as the catalog names it (`accounting.ap.ledger.bill.create`
/// → `ledger.bill.create`), or its port suffix when the catalog has none.
fn catalog_operation(op: &str) -> String {
    let suffix = tools::plugin_tool::port_suffix(op);
    tools::interface_catalog::gated_operations()
        .iter()
        .find(|c| **c == op || tools::plugin_tool::port_suffix(c) == suffix)
        .map(|c| c.to_string())
        .unwrap_or(suffix)
}

/// The capability an operation belongs to: its first word.
fn capability_of(op: &str) -> &str {
    op.split('.').next().unwrap_or("")
}

/// A safety rule's operation: giving an employee more access, or changing
/// the company's own file. It always asks the owner and has no switch.
fn is_safety(op: &str) -> bool {
    matches!(capability_of(op), "authority" | "layers") && tools::interface_catalog::is_critical(op)
}

/// The MCP server a tool key belongs to (`mcp__crm__lookup` → `crm`).
fn mcp_server(key: &str) -> Option<&str> {
    key.strip_prefix("mcp__").map(|rest| rest.split_once("__").map_or(rest, |(slug, _)| slug))
}

/// The id a switch is set by.
fn switch_id(key: &RuleKey) -> String {
    match key {
        RuleKey::Capability(c) => format!("capability:{c}"),
        RuleKey::Tool(t) => format!("tool:{t}"),
        RuleKey::Operation(o) => format!("operation:{o}"),
    }
}

/// What one scope's rules set for a switch whose keys are `covers`, its
/// own first and then its defaults: the effect, and whether a rule on its
/// own key sets it, and whether a locked rule on its own key fixes it. As
/// in the rules engine, the narrowest key with a rule decides and a locked
/// rule always counts.
fn scope_setting(rules: &[&Rule], covers: &[RuleKey]) -> Option<(Effect, bool, bool)> {
    let matched: Vec<(&Rule, usize)> = rules
        .iter()
        .filter(|r| r.field.is_none())
        .filter_map(|r| covers.iter().position(|k| same_key(k, &r.key)).map(|i| (*r, i)))
        .collect();
    let narrowest = matched.iter().map(|(_, i)| *i).min()?;
    let effect = matched.iter().filter(|(r, i)| *i == narrowest || r.locked).map(|(r, _)| r.effect).max()?;
    let own = |locked: bool| matched.iter().any(|(r, i)| *i == 0 && r.locked == locked);
    Some((effect, own(false), own(true)))
}

/// The rules a page's switches read: its own scope's, and on an employee's
/// page the company's.
struct Scopes<'a> {
    own: Vec<&'a Rule>,
    company: Option<Vec<&'a Rule>>,
}

impl Scopes<'_> {
    /// One switch. `standing` is what applies with no rule at all.
    fn switch(&self, sentence: String, covers: &[RuleKey], standing: Effect) -> PermissionSwitch {
        let mine = scope_setting(&self.own, covers);
        let theirs = self.company.as_ref().and_then(|c| scope_setting(c, covers));
        let own_setting = mine.is_some_and(|(_, own, _)| own);
        // A law or a package's rule on it fixes it for both pages.
        let fixed = [mine, theirs].iter().flatten().any(|(_, _, fixed)| *fixed);
        let (value, from, locked) = match (mine, theirs) {
            // No rule of the employee's undoes a company "off".
            (_, Some((Effect::Deny, _, _))) => (Effect::Deny, Some("company"), true),
            (Some((e, true, _)), _) => (e, None, fixed),
            (Some((e, false, _)), _) => (e, Some("default"), fixed),
            (None, Some((e, _, _))) => (e, Some("company"), fixed),
            (None, None) => (standing, Some(if self.company.is_some() { "company" } else { "default" }), false),
        };
        PermissionSwitch {
            id: switch_id(&covers[0]),
            sentence,
            value: value.as_str().to_string(),
            inherited: from.is_some(),
            inherits_from: from.map(str::to_string),
            can_inherit: own_setting && !locked,
            locked,
        }
    }
}

/// Build one page: an employee's (`Some`) or the company defaults (`None`).
/// `connected` is what the company's plugins provide, with their names.
fn page(
    store: &db::Store,
    agent_id: Option<&str>,
    connected: &BTreeMap<String, Vec<String>>,
) -> Result<PermissionsPage, NeboError> {
    let company_mode = store.permission_mode(&Scope::Company)?.unwrap_or_default();
    let (mode, mode_from_company) = match agent_id {
        Some(id) => match store.permission_mode(&Scope::Employee(id.to_string()))? {
            Some(m) => (m, false),
            None => (company_mode, true),
        },
        None => (company_mode, false),
    };
    let shown = shown_rules(store, agent_id)?;
    let company_rules = match agent_id {
        Some(_) => store.permission_rules_in(&Scope::Company)?,
        None => Vec::new(),
    };
    let scopes = Scopes {
        own: shown.iter().filter(|(_, from_company)| !from_company).map(|(r, _)| r).collect(),
        company: agent_id.map(|_| company_rules.iter().collect()),
    };
    let mut p = PermissionsPage {
        mode: mode.as_str().to_string(),
        mode_from_company,
        company_mode: company_mode.as_str().to_string(),
        capabilities: Vec::new(),
        groups: Vec::new(),
        specific: Vec::new(),
        money: Vec::new(),
        folders: Vec::new(),
        always_asks: Vec::new(),
        fixed: Vec::new(),
    };

    // What applies with no setting at all: Full Access runs it without
    // asking; any other mode asks for work outside the job. A connected
    // server's tool is no capability, so outside Ask and Plan mode it runs
    // as everyday work (the surfaced cases still ask).
    let standing = if mode == Mode::FullAccess { Effect::Allow } else { Effect::Ask };
    let tool_standing = if matches!(mode, Mode::Ask | Mode::Plan) { Effect::Ask } else { Effect::Allow };

    // What employees can do: the built-in capabilities.
    for cap in builtin_capabilities() {
        p.capabilities.push(scopes.switch(capability_phrase(cap), &[RuleKey::Capability(cap.into())], standing));
    }

    // What the plugins provide: the connected capabilities, and any other a
    // rule names, each with its actions.
    let mut services: BTreeMap<String, Vec<String>> =
        connected.iter().filter(|(c, _)| !builtin_capabilities().any(|b| b == c.as_str())).map(|(c, n)| (c.clone(), n.clone())).collect();
    let mut actions: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (rule, _) in &shown {
        match (&rule.key, &rule.field) {
            (RuleKey::Operation(op), None) if !rule.locked => {
                let op = catalog_operation(op);
                if !is_safety(&op) {
                    services.entry(capability_of(&op).to_string()).or_default();
                    actions.entry(capability_of(&op).to_string()).or_default().insert(op);
                }
            }
            (RuleKey::Capability(c), None)
                if !rule.locked && c != "chat" && !builtin_capabilities().any(|b| b == c.as_str()) =>
            {
                services.entry(c.clone()).or_default();
            }
            _ => {}
        }
    }
    for (cap, providers) in &services {
        let mut ops = actions.remove(cap).unwrap_or_default();
        if connected.contains_key(cap) {
            ops.extend(
                tools::interface_catalog::gated_operations()
                    .iter()
                    .filter(|op| capability_of(op) == cap && !is_safety(op))
                    .map(|op| op.to_string()),
            );
        }
        let default_key = RuleKey::Capability(cap.clone());
        let mut rows: Vec<PermissionSwitch> = ops
            .iter()
            .map(|op| {
                scopes.switch(operation_phrase(op), &[RuleKey::Operation(op.clone()), default_key.clone()], standing)
            })
            .collect();
        rows.sort_by(|a, b| a.sentence.cmp(&b.sentence));
        p.groups.push(PermissionGroup {
            id: format!("group:{cap}"),
            title: capability_phrase(cap),
            subtitle: providers.join(", "),
            default: scopes.switch(capability_phrase(cap), &[default_key], standing),
            rows,
        });
    }

    // The MCP servers, and any other a rule names, each with its tools.
    let mut servers: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();
    for integration in store.list_mcp_integrations()? {
        let prefix = mcp::bridge::tool_name_prefix(&integration.name);
        let tools = store.get_mcp_known_tools(&integration.id)?;
        let entry = servers.entry(mcp::bridge::server_slug(&prefix)).or_insert_with(|| (integration.name.clone(), BTreeSet::new()));
        entry.1.extend(tools.iter().map(|t| mcp::bridge::make_tool_name(&prefix, t)));
    }
    for (rule, _) in &shown {
        if let (RuleKey::Tool(key), None, false) = (&rule.key, &rule.field, rule.locked)
            && let Some(slug) = mcp_server(key)
        {
            let entry = servers.entry(slug.to_string()).or_insert_with(|| (tools::humanize::service_name(slug), BTreeSet::new()));
            if !key.ends_with('*') {
                entry.1.insert(key.clone());
            }
        }
    }
    let mut mcp_groups = Vec::new();
    for (slug, (name, tool_keys)) in &servers {
        let default_key = RuleKey::Tool(format!("mcp__{slug}__*"));
        let family = format!("mcp__{slug}__");
        let mut rows: Vec<PermissionSwitch> = tool_keys
            .iter()
            .map(|key| {
                let tool = key.strip_prefix(&family).unwrap_or(key);
                scopes.switch(sentence_case(&words(tool)), &[RuleKey::Tool(key.clone()), default_key.clone()], tool_standing)
            })
            .collect();
        rows.sort_by(|a, b| a.sentence.cmp(&b.sentence));
        mcp_groups.push(PermissionGroup {
            id: format!("group:mcp:{slug}"),
            title: name.clone(),
            subtitle: String::new(),
            default: scopes.switch(key_phrase(&format!("{family}*")), &[default_key], tool_standing),
            rows,
        });
    }
    p.groups.sort_by(|a, b| a.title.cmp(&b.title));
    mcp_groups.sort_by(|a, b| a.title.cmp(&b.title));
    p.groups.extend(mcp_groups);

    // Every other rule: money, folders, safety, fixed, or a specific setting.
    let switched: Vec<&str> = p
        .capabilities
        .iter()
        .chain(p.groups.iter().flat_map(|g| std::iter::once(&g.default).chain(&g.rows)))
        .map(|s| s.id.as_str())
        .collect();
    for (rule, from_company) in &shown {
        let item = |sentence: String, removable: bool| PermissionItem {
            id: rule.id.clone(),
            sentence,
            removable,
            from_company: *from_company,
            money: rule.money.as_ref().map(MoneyAmounts::from),
        };
        let own_switch = rule.field.is_none() && switched.contains(&switch_id(&normalized(&rule.key)).as_str());
        match section_of(rule) {
            Section::Fixed if rule.effect == Effect::Ask => p.always_asks.push(item(rule_phrase(rule), false)),
            Section::Fixed => p.fixed.push(item(item_sentence(rule, Section::Fixed), false)),
            Section::Folders => p.folders.push(item(item_sentence(rule, Section::Folders), !from_company)),
            Section::Money => p.money.push(item(item_sentence(rule, Section::Money), !from_company)),
            _ if rule.effect == Effect::Ask
                && rule.field.is_none()
                && matches!(&rule.key, RuleKey::Operation(op) if is_safety(&catalog_operation(op))) =>
            {
                p.always_asks.push(item(rule_phrase(rule), false))
            }
            _ if own_switch => {}
            _ => {
                let locked = *from_company && rule.effect == Effect::Deny;
                p.specific.push(PermissionSwitch {
                    id: format!("rule:{}", rule.id),
                    sentence: rule_phrase(rule),
                    value: rule.effect.as_str().to_string(),
                    inherited: *from_company,
                    inherits_from: from_company.then(|| "company".to_string()),
                    can_inherit: !from_company,
                    locked,
                })
            }
        }
    }
    for list in [&mut p.money, &mut p.folders, &mut p.always_asks, &mut p.fixed] {
        list.sort_by(|a, b| a.sentence.cmp(&b.sentence));
    }
    p.specific.sort_by(|a, b| a.sentence.cmp(&b.sentence));
    Ok(p)
}

/// A rule's key as its switch names it (an operation as the catalog does).
fn normalized(key: &RuleKey) -> RuleKey {
    match key {
        RuleKey::Operation(op) => RuleKey::Operation(catalog_operation(op)),
        other => other.clone(),
    }
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
    connected: &BTreeMap<String, Vec<String>>,
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
    if let Some(edit) = &body.set {
        set_switch(store, agent_id, edit, connected)?;
    }
    Ok(())
}

/// Move one switch: write this page's own rule on its key, or clear it.
fn set_switch(
    store: &db::Store,
    agent_id: Option<&str>,
    edit: &SwitchEdit,
    connected: &BTreeMap<String, Vec<String>>,
) -> Result<(), NeboError> {
    let p = page(store, agent_id, connected)?;
    let switch = p
        .capabilities
        .iter()
        .chain(p.groups.iter().flat_map(|g| std::iter::once(&g.default).chain(&g.rows)))
        .chain(&p.specific)
        .find(|s| s.id == edit.id)
        .ok_or_else(|| NeboError::Validation("that can't be set here".into()))?;
    if switch.locked {
        return Err(NeboError::Validation("the company defaults turn this off; change it in the company defaults".into()));
    }
    let (key, field) = match edit.id.split_once(':') {
        Some(("capability", c)) => (RuleKey::Capability(c.into()), None),
        Some(("tool", t)) => (RuleKey::Tool(t.into()), None),
        Some(("operation", o)) => (RuleKey::Operation(o.into()), None),
        Some(("rule", id)) => {
            let rule = store.get_permission_rule(id)?.ok_or(NeboError::NotFound)?;
            (rule.key, rule.field)
        }
        _ => return Err(NeboError::NotFound),
    };
    let scope = scope_of(agent_id);
    let own = store.permission_rules_in(&scope)?.into_iter().find(|r| same_key(&r.key, &key) && r.field == field);
    if edit.value == "inherit" {
        if !switch.can_inherit {
            return Err(NeboError::Validation("this has no setting of its own to clear".into()));
        }
        return match own {
            Some(rule) => store.remove_permission_rule(&rule.id, &Writer::Owner).map_err(rule_error),
            None => Ok(()),
        };
    }
    let effect = Effect::parse(&edit.value)
        .ok_or_else(|| NeboError::Validation("value must be allow, ask, deny or inherit".into()))?;
    let source = if matches!(key, RuleKey::Capability(_)) { RuleSource::JobEdit } else { RuleSource::Owner };
    let rule = match own {
        Some(rule) => Rule { effect, source, ..rule },
        None => owner_rule(scope, key, field, effect, source),
    };
    store.write_permission_rule(&rule, &Writer::Owner).map_err(rule_error)?;
    Ok(())
}

/// Remove one item (a folder or a money limit), as the owner. A company
/// default changes on the company page.
fn remove(store: &db::Store, agent_id: Option<&str>, rule_id: &str) -> Result<(), NeboError> {
    let rule = store.get_permission_rule(rule_id)?.ok_or(NeboError::NotFound)?;
    if rule.scope == scope_of(agent_id) {
        return store.remove_permission_rule(rule_id, &Writer::Owner).map_err(rule_error);
    }
    match (&rule.scope, agent_id) {
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
            let (why, why_unreviewed) = why_sentence(store, &r.decision, &r.why, &r.door);
            let (decided_by, verdict) = review_sentences(r.judgement.as_deref());
            ActivityRow {
                at: r.created_at,
                employee: name_of(&r.agent_id),
                employee_id: r.agent_id,
                action: action_sentence(&r.activity, &r.rule_key),
                decision: r.decision,
                why,
                door: r.door,
                unreviewed: r.unreviewed || why_unreviewed,
                decided_by,
                verdict,
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
fn why_sentence(store: &db::Store, decision: &str, why: &str, door: &str) -> (String, bool) {
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
            "coworker" => "Another employee asked for this, and a coworker's request can only reply".into(),
            "credentials" => "It would have shared a password or key".into(),
            _ => "A safety limit".into(),
        },
        Why::Ceiling => "It can't do more than the employee or run it works for".into(),
        Why::CannotWait { case } => format!(
            "It needed your OK, and {} can't wait for one, so it didn't run: {}",
            types::permissions::Door::unattended_words(door),
            lower_first(&ask_sentence(store, &case))
        ),
    };
    (s, false)
}

/// Who decided a recorded call, and the reviewer's verdict: from the
/// judgement recorded with it (`{mode, verdict, by, reason}`), or the
/// permission rules alone when no reviewer was asked. A shadow verdict is
/// recorded only; the rules' decision stood.
fn review_sentences(judgement: Option<&str>) -> (String, String) {
    let Some(j) = judgement.and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok()) else {
        return ("Decided by the permission rules".into(), String::new());
    };
    let reason = j["reason"].as_str().unwrap_or_default().trim().trim_end_matches('.');
    let by = match j["by"].as_str() {
        Some("jev") => "Reviewed by Jev",
        Some(_) => "Reviewed by the backup reviewer",
        None => "Not reviewed: neither reviewer could run",
    };
    let shadow = j["mode"].as_str() == Some(JudgementMode::Shadow.as_str());
    let verdict = match (j["verdict"].as_str(), shadow) {
        (Some("allow"), _) => format!("It judged this fine: {reason}"),
        (Some("ask"), false) => format!("It judged you should be asked: {reason}"),
        (Some("ask"), true) => format!("It would have asked you, but its verdicts are only recorded for now: {reason}"),
        _ => "The permission check couldn't run, so it went ahead".to_string(),
    };
    (by.to_string(), verdict)
}

fn ask_sentence(store: &db::Store, case: &AskCase) -> String {
    match case {
        AskCase::Money { cents, .. } => format!("It would spend {}, more than its limit", dollars(*cents)),
        AskCase::CompanyMoney { cents, .. } => {
            format!(
                "It would spend {}, more than the company allows unattended today",
                dollars(*cents)
            )
        }
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
        AskCase::UnconfirmedSend { .. } => "A send whose outcome never came back: did it go out?".into(),
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
    match (resource, action) {
        ("grant", "grant") => return "Give itself or another employee more access".to_string(),
        ("grant", "widen") => return "Widen the access it or another employee has".to_string(),
        ("company", "write") => return "Change the company file".to_string(),
        _ => {}
    }
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

    fn none() -> BTreeMap<String, Vec<String>> {
        BTreeMap::new()
    }

    fn set(store: &db::Store, agent: Option<&str>, id: &str, value: &str) -> Result<(), NeboError> {
        let body = PermissionsUpdate { set: Some(SwitchEdit { id: id.into(), value: value.into() }), ..Default::default() };
        update(store, agent, &body, &none())
    }

    fn all_switches(p: &PermissionsPage) -> Vec<&PermissionSwitch> {
        p.capabilities
            .iter()
            .chain(p.groups.iter().flat_map(|g| std::iter::once(&g.default).chain(&g.rows)))
            .chain(&p.specific)
            .collect()
    }

    fn switch<'a>(p: &'a PermissionsPage, id: &str) -> &'a PermissionSwitch {
        all_switches(p).into_iter().find(|s| s.id == id).unwrap_or_else(|| panic!("no switch {id}"))
    }

    /// (value, inherits from) — `None` when the page's own rule sets it.
    fn shows<'a>(p: &'a PermissionsPage, id: &str) -> (&'a str, Option<&'a str>) {
        let s = switch(p, id);
        (s.value.as_str(), s.inherits_from.as_deref())
    }

    /// A connected MCP server with the tools it offered at its last sync.
    fn mcp_server(store: &db::Store, name: &str, tools: &[&str]) {
        let id = format!("{name}-1");
        store.create_mcp_integration(&id, name, "http", Some("https://mcp.example.com"), "none", None, None).unwrap();
        store.set_mcp_known_tools(&id, &tools.iter().map(|t| t.to_string()).collect::<Vec<_>>()).unwrap();
    }

    /// What the rules engine decides for an employee's call to `key`.
    fn engine(store: &db::Store, agent: &str, key: &str, capability: Option<&str>, operation: Option<&str>) -> Option<Effect> {
        let t = types::permissions::Target {
            tool: key.into(),
            key: key.into(),
            operation: operation.map(str::to_string),
            capability: capability.map(str::to_string),
            field: None,
            subject: None,
            read_only: false,
            effects: types::permissions::CallEffects::unknown(),
        };
        agent::harness::permissions::RuleSet::load(store, agent).unwrap().decide(&t).map(|d| d.1)
    }

    #[test]
    fn no_rule_string_reaches_the_client() {
        let (_d, store) = store();
        mcp_server(&store, "Acme CRM", &["search_contacts"]);
        put(&store, Scope::Company, cap("shell"), None, Effect::Allow, RuleSource::Migrated { from: "x".into() });
        put(&store, emp(), RuleKey::Tool("run_command".into()), Some(RuleField::CommandPrefix("git status".into())), Effect::Allow, RuleSource::AllowAlways { ask_id: "k".into() });
        put(&store, emp(), RuleKey::Operation("mail.message.send".into()), Some(RuleField::Recipient("pat@example.com".into())), Effect::Allow, RuleSource::Owner);
        put(&store, emp(), RuleKey::Tool("mcp__acme_crm__search_contacts".into()), None, Effect::Ask, RuleSource::Owner);
        put(&store, emp(), RuleKey::Tool("desktop_*".into()), None, Effect::Deny, RuleSource::Owner);
        put(&store, emp(), cap("file"), Some(RuleField::Folder("/srv/work".into())), Effect::Allow, RuleSource::Owner);
        put(&store, Scope::Company, RuleKey::Operation("authority.grant.grant".into()), None, Effect::Ask, RuleSource::Migrated { from: "x".into() });
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

        let connected = BTreeMap::from([("ledger".to_string(), vec!["Books".to_string()])]);
        let page = serde_json::to_value(page(&store, Some("a"), &connected).unwrap()).unwrap();
        let mut out = Vec::new();
        strings(&page, &mut out);
        assert!(out.len() >= 9, "{page}");
        let forbidden = [
            "run_command", "mail.message.send", "mcp__", "desktop_", "command_prefix", "capability",
            "ledger.", "billpayment", "authority", "_", "{", "\"kind\"",
        ];
        for s in &out {
            for f in forbidden {
                assert!(!s.contains(f), "{s:?} carries {f:?}");
            }
        }
        for sentence in [
            "Run commands that start with \u{201c}git status\u{201d}",
            "Send message to pat@example.com",
            "Create bill payments, up to $50 a day",
            "Never: void invoice",
            "Give itself or another employee more access",
            "Search contacts",
            "Acme CRM",
            "Books",
        ] {
            assert!(out.contains(&sentence.to_string()), "{sentence:?} in {out:?}");
        }
    }

    /// A capability's switch writes the page's own rule on it and clears it
    /// again; an employee's page shows the company's value until it has its
    /// own, and a company "off" binds it.
    #[test]
    fn capability_switches_set_and_inherit() {
        let (_d, store) = store();
        let p = page(&store, None, &none()).unwrap();
        let caps: Vec<&str> = p.capabilities.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(caps.len(), builtin_capabilities().count());
        assert_eq!(shows(&p, "capability:web"), ("ask", Some("default")), "outside the job, it asks");
        assert!(!switch(&p, "capability:web").can_inherit);

        set(&store, None, "capability:web", "allow").unwrap();
        let rule = store.permission_rules_in(&Scope::Company).unwrap().pop().unwrap();
        assert_eq!((&rule.key, rule.effect, &rule.source), (&cap("web"), Effect::Allow, &RuleSource::JobEdit));
        let p = page(&store, None, &none()).unwrap();
        assert_eq!(shows(&p, "capability:web"), ("allow", None));
        assert!(switch(&p, "capability:web").can_inherit);

        // An employee follows the company until it has its own setting.
        let e = page(&store, Some("a"), &none()).unwrap();
        assert_eq!(shows(&e, "capability:web"), ("allow", Some("company")));
        assert!(!switch(&e, "capability:web").can_inherit);
        set(&store, Some("a"), "capability:web", "ask").unwrap();
        assert_eq!(shows(&page(&store, Some("a"), &none()).unwrap(), "capability:web"), ("ask", None));
        assert_eq!(shows(&page(&store, None, &none()).unwrap(), "capability:web"), ("allow", None), "the company's stays");
        assert_eq!(engine(&store, "a", "web_fetch", Some("web"), None), Some(Effect::Ask));
        set(&store, Some("a"), "capability:web", "inherit").unwrap();
        assert!(store.permission_rules_in(&emp()).unwrap().is_empty());
        assert_eq!(shows(&page(&store, Some("a"), &none()).unwrap(), "capability:web"), ("allow", Some("company")));

        // A company "off" binds every employee: locked on the employee's page.
        set(&store, None, "capability:shell", "deny").unwrap();
        let e = page(&store, Some("a"), &none()).unwrap();
        let shell = switch(&e, "capability:shell");
        assert_eq!((shell.value.as_str(), shell.locked, shell.can_inherit), ("deny", true, false));
        assert!(matches!(set(&store, Some("a"), "capability:shell", "allow"), Err(NeboError::Validation(_))));
        // Clearing the company's own setting returns it to the default.
        set(&store, None, "capability:web", "inherit").unwrap();
        assert_eq!(shows(&page(&store, None, &none()).unwrap(), "capability:web"), ("ask", Some("default")));
        assert!(matches!(set(&store, None, "capability:web", "inherit"), Err(NeboError::Validation(_))));
        assert!(matches!(set(&store, None, "capability:web", "sometimes"), Err(NeboError::Validation(_))));
        assert!(matches!(set(&store, None, "capability:ledger", "allow"), Err(NeboError::Validation(_))), "not on the page");
    }

    /// Every switch round-trips each of its three states, as the page's JSON
    /// carries it and as the rules engine then decides, and `inherit` puts
    /// it back where it came from, on the company page and an employee's.
    #[test]
    fn every_switch_round_trips_allow_ask_and_off() {
        let (_d, store) = store();
        mcp_server(&store, "Acme CRM", &["lookup"]);
        let connected = BTreeMap::from([("ledger".to_string(), vec!["Books".to_string()])]);
        let switches = [
            ("capability:web", ("web_fetch", Some("web"), None)),
            ("tool:mcp__acme_crm__*", ("mcp__acme_crm__lookup", None, None)),
            ("tool:mcp__acme_crm__lookup", ("mcp__acme_crm__lookup", None, None)),
            ("capability:ledger", ("ledger.bill.create", Some("ledger"), Some("accounting.ap.ledger.bill.create"))),
            ("operation:ledger.bill.create", ("ledger.bill.create", Some("ledger"), Some("accounting.ap.ledger.bill.create"))),
        ];
        for agent in [None, Some("a")] {
            for (id, (key, capability, operation)) in switches {
                let before = page(&store, agent, &connected).unwrap();
                let was = shows(&before, id);
                for value in ["allow", "ask", "deny"] {
                    let body = PermissionsUpdate { set: Some(SwitchEdit { id: id.into(), value: value.into() }), ..Default::default() };
                    update(&store, agent, &body, &connected).unwrap();
                    let json = serde_json::to_value(page(&store, agent, &connected).unwrap()).unwrap();
                    let sw = json["capabilities"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .chain(json["groups"].as_array().unwrap().iter().flat_map(|g| std::iter::once(&g["default"]).chain(g["rows"].as_array().unwrap())))
                        .find(|s| s["id"] == id)
                        .unwrap_or_else(|| panic!("no {id}"));
                    assert_eq!((sw["value"].as_str(), sw["inherited"].as_bool(), sw["canInherit"].as_bool()), (Some(value), Some(false), Some(true)), "{agent:?} {id} {value}");
                    assert_eq!(engine(&store, "a", key, capability, operation).map(|e| e.as_str()), Some(value), "{agent:?} {id} {value}");
                }
                let body = PermissionsUpdate { set: Some(SwitchEdit { id: id.into(), value: "inherit".into() }), ..Default::default() };
                update(&store, agent, &body, &connected).unwrap();
                assert_eq!(shows(&page(&store, agent, &connected).unwrap(), id), was, "{agent:?} {id} back where it was");
            }
        }
    }

    /// A switch with no setting shows what the mode does: Full Access runs
    /// it without asking, any other mode asks for work outside the job. An
    /// Off set anywhere stays Off in every mode, and locks the employee's
    /// switch when the company set it.
    #[test]
    fn a_switch_with_no_setting_shows_the_mode_and_off_wins() {
        let (_d, store) = store();
        // A newly connected server has no setting: its tools follow the mode.
        mcp_server(&store, "Acme CRM", &["lookup"]);
        let lookup = "tool:mcp__acme_crm__lookup";
        assert_eq!(shows(&page(&store, None, &none()).unwrap(), lookup), ("allow", Some("default")));
        store.set_permission_mode(&Scope::Company, Mode::Ask).unwrap();
        assert_eq!(shows(&page(&store, None, &none()).unwrap(), lookup), ("ask", Some("default")), "Ask mode asks");
        store.set_permission_mode(&Scope::Company, Mode::Automatic).unwrap();
        assert_eq!(shows(&page(&store, None, &none()).unwrap(), "capability:web"), ("ask", Some("default")));
        store.set_permission_mode(&Scope::Company, Mode::FullAccess).unwrap();
        assert_eq!(shows(&page(&store, None, &none()).unwrap(), "capability:web"), ("allow", Some("default")), "Full Access never asks");
        assert_eq!(shows(&page(&store, None, &none()).unwrap(), lookup), ("allow", Some("default")));
        assert_eq!(shows(&page(&store, Some("a"), &none()).unwrap(), "capability:web"), ("allow", Some("company")));
        store.set_permission_mode(&emp(), Mode::Ask).unwrap();
        assert_eq!(shows(&page(&store, Some("a"), &none()).unwrap(), "capability:web"), ("ask", Some("company")), "its own mode asks");

        set(&store, None, "capability:web", "deny").unwrap();
        for mode in [Mode::Automatic, Mode::Ask, Mode::Plan, Mode::FullAccess] {
            store.set_permission_mode(&emp(), mode).unwrap();
            let web = page(&store, Some("a"), &none()).unwrap();
            let web = switch(&web, "capability:web");
            assert_eq!((web.value.as_str(), web.locked), ("deny", true), "{mode:?}");
            assert_eq!(engine(&store, "a", "web_fetch", Some("web"), None), Some(Effect::Deny), "{mode:?}");
        }
        assert!(matches!(set(&store, Some("a"), "capability:web", "allow"), Err(NeboError::Validation(_))));
    }

    /// An MCP server is a group: its default, and one switch per tool that
    /// follows the default until it has its own setting. What the page shows
    /// is what the rules engine decides.
    #[test]
    fn an_mcp_servers_tools_follow_its_default_until_set() {
        let (_d, store) = store();
        mcp_server(&store, "Acme CRM", &["lookup", "delete_all"]);
        // The owner set the server to ask first.
        let all = put(&store, Scope::Company, RuleKey::Tool("mcp__acme_crm__*".into()), None, Effect::Ask, RuleSource::Owner);
        let p = page(&store, None, &none()).unwrap();
        let g = p.groups.iter().find(|g| g.id == "group:mcp:acme_crm").unwrap();
        assert_eq!((g.title.as_str(), g.default.id.as_str(), g.default.value.as_str()), ("Acme CRM", "tool:mcp__acme_crm__*", "ask"));
        let rows: Vec<(&str, &str, Option<&str>)> =
            g.rows.iter().map(|s| (s.sentence.as_str(), s.value.as_str(), s.inherits_from.as_deref())).collect();
        assert_eq!(rows, [("Delete all", "ask", Some("default")), ("Lookup", "ask", Some("default"))]);

        // A tool's own setting outranks the default, looser or stricter.
        set(&store, None, "tool:mcp__acme_crm__lookup", "allow").unwrap();
        set(&store, None, "tool:mcp__acme_crm__*", "deny").unwrap();
        let p = page(&store, None, &none()).unwrap();
        assert_eq!(shows(&p, "tool:mcp__acme_crm__lookup"), ("allow", None));
        assert_eq!(shows(&p, "tool:mcp__acme_crm__delete_all"), ("deny", Some("default")));
        assert_eq!(store.get_permission_rule(&all.id).unwrap().unwrap().effect, Effect::Deny, "one rule per key");
        assert_eq!(engine(&store, "a", "mcp__acme_crm__lookup", None, None), Some(Effect::Allow));
        assert_eq!(engine(&store, "a", "mcp__acme_crm__delete_all", None, None), Some(Effect::Deny));
        set(&store, None, "tool:mcp__acme_crm__lookup", "inherit").unwrap();
        assert_eq!(shows(&page(&store, None, &none()).unwrap(), "tool:mcp__acme_crm__lookup"), ("deny", Some("default")));

        // On an employee's page a company "off" binds; the company's other
        // settings can be changed for that employee alone.
        set(&store, None, "tool:mcp__acme_crm__*", "ask").unwrap();
        set(&store, None, "tool:mcp__acme_crm__delete_all", "deny").unwrap();
        let e = page(&store, Some("a"), &none()).unwrap();
        assert_eq!(shows(&e, "tool:mcp__acme_crm__lookup"), ("ask", Some("company")));
        assert!(switch(&e, "tool:mcp__acme_crm__delete_all").locked);
        set(&store, Some("a"), "tool:mcp__acme_crm__lookup", "allow").unwrap();
        let e = page(&store, Some("a"), &none()).unwrap();
        assert_eq!(shows(&e, "tool:mcp__acme_crm__lookup"), ("allow", None));
        assert_eq!(engine(&store, "a", "mcp__acme_crm__lookup", None, None), Some(Effect::Allow));
        assert_eq!(engine(&store, "b", "mcp__acme_crm__lookup", None, None), Some(Effect::Ask));
        assert!(matches!(set(&store, Some("a"), "tool:mcp__acme_crm__delete_all", "allow"), Err(NeboError::Validation(_))));

        // A setting for a tool the server no longer offers still shows.
        put(&store, Scope::Company, RuleKey::Tool("mcp__acme_crm__export".into()), None, Effect::Allow, RuleSource::Owner);
        assert_eq!(shows(&page(&store, None, &none()).unwrap(), "tool:mcp__acme_crm__export"), ("allow", None));
        assert!(matches!(set(&store, None, "tool:mcp__other__x", "allow"), Err(NeboError::Validation(_))));
    }

    /// A plugin's capability is a group: its default is the capability, its
    /// rows the actions, each following the default until it has its own.
    #[test]
    fn a_plugins_actions_follow_its_default_until_set() {
        let (_d, store) = store();
        let connected = BTreeMap::from([("ledger".to_string(), vec!["Books".to_string()])]);
        // The upgrade's critical asks are the actions' own settings.
        put(&store, Scope::Company, RuleKey::Operation("ledger.billpayment.create".into()), None, Effect::Ask, RuleSource::Migrated { from: "x".into() });
        let p = page(&store, None, &connected).unwrap();
        let g = p.groups.iter().find(|g| g.id == "group:ledger").unwrap();
        assert_eq!((g.title.as_str(), g.subtitle.as_str(), g.default.id.as_str()), ("Work in your accounting", "Books", "capability:ledger"));
        assert!(g.rows.len() > 1 && g.rows.iter().all(|r| r.id.starts_with("operation:ledger.")));
        assert!(!p.capabilities.iter().any(|s| s.id == "capability:ledger"), "a plugin's capability is its group");

        let update = |agent: Option<&str>, id: &str, value: &str| {
            let body = PermissionsUpdate { set: Some(SwitchEdit { id: id.into(), value: value.into() }), ..Default::default() };
            update(&store, agent, &body, &connected)
        };
        update(None, "capability:ledger", "allow").unwrap();
        let p = page(&store, None, &connected).unwrap();
        assert_eq!(shows(&p, "operation:ledger.bill.create"), ("allow", Some("default")));
        assert_eq!(shows(&p, "operation:ledger.billpayment.create"), ("ask", None));
        let pay = Some("ledger.billpayment.create");
        assert_eq!(engine(&store, "a", "ledger.billpayment.create", Some("ledger"), pay), Some(Effect::Ask));
        update(None, "operation:ledger.billpayment.create", "allow").unwrap();
        assert_eq!(engine(&store, "a", "ledger.billpayment.create", Some("ledger"), pay), Some(Effect::Allow));
        update(Some("a"), "operation:ledger.billpayment.create", "deny").unwrap();
        assert_eq!(engine(&store, "a", "ledger.billpayment.create", Some("ledger"), pay), Some(Effect::Deny));
        assert_eq!(shows(&page(&store, Some("a"), &connected).unwrap(), "operation:ledger.billpayment.create"), ("deny", None));
    }

    /// Giving an employee more access and changing the company's own file
    /// always ask: plain words, no switch. A locked rule is fixed.
    #[test]
    fn safety_rules_always_ask_and_never_switch() {
        let (_d, store) = store();
        for op in ["authority.grant.grant", "authority.grant.widen", "layers.company.write", "layers.company.remove"] {
            put(&store, Scope::Company, RuleKey::Operation(op.into()), None, Effect::Ask, RuleSource::Migrated { from: "x".into() });
        }
        let law = Rule {
            locked: true,
            ..owner_rule(emp(), RuleKey::Operation("ledger.invoice.void".into()), None, Effect::Deny, RuleSource::Law { pack: "p".into() })
        };
        store.write_permission_rule(&law, &Writer::Package { package: "p".into() }).unwrap();
        for agent in [None, Some("a")] {
            let p = page(&store, agent, &none()).unwrap();
            let asks: Vec<&str> = p.always_asks.iter().map(|i| i.sentence.as_str()).collect();
            assert_eq!(
                asks,
                [
                    "Change the company file",
                    "Delete the company file",
                    "Give itself or another employee more access",
                    "Widen the access it or another employee has",
                ]
            );
            assert!(p.always_asks.iter().all(|i| !i.removable));
            assert!(p.groups.is_empty() && p.specific.is_empty(), "{:?} {:?}", p.groups, p.specific);
            assert!(matches!(set(&store, agent, "operation:authority.grant.grant", "allow"), Err(NeboError::Validation(_))));
        }
        let p = page(&store, Some("a"), &none()).unwrap();
        assert_eq!(p.fixed.iter().map(|i| (i.sentence.as_str(), i.removable)).collect::<Vec<_>>(), [("Never: void invoice", false)]);
        assert!(remove(&store, Some("a"), &law.id).is_err());
    }

    /// A setting on one command, site or recipient is its own switch: moved,
    /// cleared, or on an employee's page set for that employee alone.
    #[test]
    fn specific_settings_switch_and_clear() {
        let (_d, store) = store();
        let git = put(&store, Scope::Company, RuleKey::Tool("run_command".into()), Some(RuleField::CommandPrefix("git status".into())), Effect::Allow, RuleSource::AllowAlways { ask_id: "k".into() });
        let id = format!("rule:{}", git.id);
        let p = page(&store, None, &none()).unwrap();
        assert_eq!(p.specific.len(), 1);
        assert_eq!((p.specific[0].sentence.as_str(), shows(&p, &id)), ("Run commands that start with \u{201c}git status\u{201d}", ("allow", None)));

        // For one employee: its own rule on the same command.
        let e = page(&store, Some("a"), &none()).unwrap();
        assert_eq!(shows(&e, &id), ("allow", Some("company")));
        set(&store, Some("a"), &id, "ask").unwrap();
        let own = store.permission_rules_in(&emp()).unwrap();
        assert_eq!((own.len(), own[0].effect, &own[0].field), (1, Effect::Ask, &git.field));
        assert_eq!(store.get_permission_rule(&git.id).unwrap().unwrap().effect, Effect::Allow);

        set(&store, None, &id, "deny").unwrap();
        assert_eq!(store.get_permission_rule(&git.id).unwrap().unwrap().effect, Effect::Deny, "the same rule, moved");
        set(&store, None, &id, "inherit").unwrap();
        assert!(store.get_permission_rule(&git.id).unwrap().is_none());
        assert!(page(&store, None, &none()).unwrap().specific.is_empty());
    }

    #[test]
    fn folders_are_added_and_removed_on_their_own_page() {
        let (_d, store) = store();
        let folder = PermissionsUpdate { add_folder: Some("/srv/work".into()), ..Default::default() };
        update(&store, None, &folder, &none()).unwrap();
        assert!(update(&store, None, &PermissionsUpdate { add_folder: Some("relative".into()), ..Default::default() }, &none()).is_err());
        let company = page(&store, None, &none()).unwrap().folders.pop().unwrap();
        assert_eq!((company.sentence.as_str(), company.removable), ("Change files in /srv/work", true));
        let e = page(&store, Some("a"), &none()).unwrap();
        assert!(e.folders.iter().any(|i| i.id == company.id && i.from_company && !i.removable));
        assert!(e.capabilities.iter().all(|s| s.id != "capability:file" || s.inherited), "a folder is not the file switch");
        assert!(matches!(remove(&store, Some("a"), &company.id), Err(NeboError::Validation(_))));
        remove(&store, None, &company.id).unwrap();
        assert!(page(&store, None, &none()).unwrap().folders.is_empty());
        let other = put(&store, Scope::Employee("b".into()), cap("web"), None, Effect::Allow, RuleSource::Owner);
        assert!(matches!(remove(&store, Some("a"), &other.id), Err(NeboError::NotFound)));
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
        update(&store, Some("a"), &edit, &none()).unwrap();
        let p = page(&store, Some("a"), &none()).unwrap();
        assert_eq!(p.money[0].sentence, "Send payment, up to $25 each time, up to $100 a day");
        assert_eq!(p.money[0].money.as_ref().unwrap().per_day_cents, Some(10000));
        // Its action's switch keeps the amounts when it moves.
        set(&store, Some("a"), "operation:payments.payment.send", "ask").unwrap();
        let moved = store.get_permission_rule(&rule.id).unwrap().unwrap();
        assert_eq!((moved.effect, moved.money.and_then(|m| m.per_day_cents)), (Effect::Ask, Some(10000)));
    }

    #[test]
    fn mode_picker_writes_the_mode() {
        let (_d, store) = store();
        let set = |agent: Option<&str>, m: &str| {
            update(&store, agent, &PermissionsUpdate { mode: Some(m.into()), ..Default::default() }, &none())
        };
        let p = page(&store, Some("a"), &none()).unwrap();
        assert_eq!((p.mode.as_str(), p.mode_from_company), ("automatic", true));
        set(Some("a"), "plan").unwrap();
        assert_eq!(store.permission_mode(&emp()).unwrap(), Some(Mode::Plan));
        let p = page(&store, Some("a"), &none()).unwrap();
        assert_eq!((p.mode.as_str(), p.mode_from_company, p.company_mode.as_str()), ("plan", false, "automatic"));
        set(Some("a"), "company").unwrap();
        assert_eq!(store.permission_mode(&emp()).unwrap(), None);
        set(None, "full_access").unwrap();
        assert_eq!(page(&store, Some("a"), &none()).unwrap().mode, "full_access");
        assert!(set(None, "company").is_err(), "the company has no default above it");
        assert!(set(Some("a"), "everything").is_err());
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
        record("deny", why(Why::HardLimit { limit: "coworker".into() }), "writing rates.txt");

        let page = activity(&store, &ActivityQuery { agent_id: Some("a".into()), ..Default::default() }).unwrap();
        assert_eq!(page.total, 7);
        let whys: Vec<&str> = page.rows.iter().rev().map(|r| r.why.as_str()).collect();
        assert_eq!(whys[0], "Part of the job you agreed to when you hired it: Read and send email");
        assert_eq!(whys[1], "You answered \u{201c}Allow always\u{201d}: Run commands that start with \u{201c}git\u{201d}");
        assert_eq!(whys[2], "You allowed it once");
        assert!(page.rows.iter().rev().nth(3).unwrap().unreviewed);
        assert_eq!(whys[4], "Not part of its job: run commands on this computer");
        assert_eq!(whys[5], "Someone outside your company started this, so it can only reply");
        assert_eq!(whys[6], "Another employee asked for this, and a coworker's request can only reply");
        assert_eq!(page.rows.last().unwrap().action, "Sending an email");
        assert_eq!(page.rows.iter().rev().nth(2).unwrap().action, "Run commands", "no label: the key in words");
        let only_asks = activity(&store, &ActivityQuery { decision: Some("ask".into()), ..Default::default() }).unwrap();
        assert_eq!(only_asks.total, 1);
    }

    /// The activity names who decided each call (the rules, Jev or the
    /// backup reviewer), the reviewer's verdict, and flags a call neither
    /// reviewer could check, all in plain words.
    #[test]
    fn activity_shows_the_review_and_who_gave_it() {
        let (_d, store) = store();
        let record = |why: Why, judgement: Option<serde_json::Value>, unreviewed: bool| {
            store
                .record_permission_activity(&db::PermissionActivityRow {
                    agent_id: "a".into(),
                    door: "local_api".into(),
                    tool: "mail_message_send".into(),
                    rule_key: "mail.message.send".into(),
                    activity: "sending an email".into(),
                    decision: "allow".into(),
                    why: serde_json::to_string(&why).unwrap(),
                    unreviewed,
                    judgement: judgement.map(|j| j.to_string()),
                    created_at: 1,
                    ..Default::default()
                })
                .unwrap()
        };
        let inside = || Why::Mode { mode: Mode::Automatic };
        record(inside(), None, false);
        record(
            Why::Judged { by: "jev".into(), reason: "a reply to the sender.".into() },
            Some(serde_json::json!({ "mode": "enforce", "verdict": "allow", "by": "jev", "reason": "a reply to the sender." })),
            false,
        );
        record(
            inside(),
            Some(serde_json::json!({ "mode": "shadow", "verdict": "ask", "by": "aux_classifier", "reason": "posts publicly" })),
            false,
        );
        record(
            inside(),
            Some(serde_json::json!({ "mode": "shadow", "verdict": "unjudged", "by": null, "reason": "the permission check couldn't run" })),
            true,
        );

        let page = activity(&store, &ActivityQuery::default()).unwrap();
        let rows: Vec<&ActivityRow> = page.rows.iter().rev().collect();
        let seen: Vec<(&str, &str, bool)> =
            rows.iter().map(|r| (r.decided_by.as_str(), r.verdict.as_str(), r.unreviewed)).collect();
        assert_eq!(
            seen,
            vec![
                ("Decided by the permission rules", "", false),
                ("Reviewed by Jev", "It judged this fine: a reply to the sender", false),
                (
                    "Reviewed by the backup reviewer",
                    "It would have asked you, but its verdicts are only recorded for now: posts publicly",
                    false
                ),
                ("Not reviewed: neither reviewer could run", "The permission check couldn't run, so it went ahead", true),
            ]
        );
        for r in &rows {
            for s in [&r.action, &r.why, &r.decided_by, &r.verdict] {
                for f in ["mail.message.send", "_", "{", "jev", "aux", "shadow", "enforce"] {
                    assert!(!s.contains(f), "{s:?} carries {f:?}");
                }
            }
        }
    }
}
