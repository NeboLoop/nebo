use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;

use agent::harness::permissions::{Answer, AnsweredVia, AskError};
use db::models::{InboxView, Notification};
use types::NeboError;

use super::{HandlerResult, to_error_response};
use crate::middleware::AuthClaims;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    /// needs_you | updates | history; every item not put away when absent.
    #[serde(default)]
    pub view: Option<String>,
    /// One employee's items.
    #[serde(default)]
    pub agent: Option<String>,
}

fn default_limit() -> i64 {
    50
}

/// Extract user_id from optional JWT claims, falling back to the local user for desktop mode.
fn user_id(claims: Option<axum::Extension<AuthClaims>>, state: &AppState) -> String {
    claims
        .map(|c| c.0.user_id)
        .or_else(|| state.store.ensure_local_user_id().ok())
        .unwrap_or_default()
}

/// GET /api/v1/notifications — one page of the Inbox, newest first, each
/// item with where it stands (`status`) and the answers it takes.
pub async fn list_notifications(
    State(state): State<AppState>,
    claims: Option<axum::Extension<AuthClaims>>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    let view = match q.view.as_deref() {
        None => InboxView::All,
        Some(v) => InboxView::parse(v).ok_or_else(|| {
            to_error_response(NeboError::Validation("view must be needs_you, updates or history".into()))
        })?,
    };
    let limit = q.limit.clamp(1, 200);
    let notifs = state
        .store
        .list_user_notifications(&user_id(claims, &state), view, q.agent.as_deref(), limit, q.offset.max(0))
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"notifications": notifs})))
}

/// PUT /api/v1/notifications/:id/read
pub async fn mark_read(
    State(state): State<AppState>,
    claims: Option<axum::Extension<AuthClaims>>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .mark_notification_read(&id, &user_id(claims, &state))
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// PUT /api/v1/notifications/read-all
pub async fn mark_all_read(
    State(state): State<AppState>,
    claims: Option<axum::Extension<AuthClaims>>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .mark_all_notifications_read(&user_id(claims, &state))
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// DELETE /api/v1/notifications/:id — put an item away. It moves to
/// History; nothing is deleted. (The route keeps its method so every app
/// already shipped keeps working.)
pub async fn delete_notification(
    State(state): State<AppState>,
    claims: Option<axum::Extension<AuthClaims>>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .archive_notification(&id, &user_id(claims, &state))
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /api/v1/notifications/unread-count — what waits on the owner: the
/// decisions still open and the updates not yet read. `count` is both (the
/// badge).
pub async fn unread_count(
    State(state): State<AppState>,
    claims: Option<axum::Extension<AuthClaims>>,
) -> HandlerResult<serde_json::Value> {
    let (needs_you, unread) = state.store.count_inbox(&user_id(claims, &state)).unwrap_or((0, 0));
    Ok(Json(serde_json::json!({"count": needs_you + unread, "needsYou": needs_you, "unread": unread})))
}

#[derive(Debug, Deserialize)]
pub struct AnswerItemBody {
    /// The option's value, or the owner's own words for a question that
    /// takes them.
    pub value: String,
    /// Where the owner answered: mobile | desktop | web | chat | notification.
    #[serde(default)]
    pub via: Option<String>,
}

/// POST /api/v1/notifications/:id/answer — answer a needs_you item. The ONE
/// answer endpoint every surface uses (the Inbox on the phone, the desktop
/// and the web, the assistant's chat card): it hands the answer to the
/// item's own answer path, which settles the item everywhere. An item
/// already settled comes back as it stands.
pub async fn answer_notification(
    State(state): State<AppState>,
    claims: Option<axum::Extension<AuthClaims>>,
    Path(id): Path<String>,
    Json(body): Json<AnswerItemBody>,
) -> HandlerResult<serde_json::Value> {
    let user = user_id(claims, &state);
    let item = state
        .store
        .get_notification(&id, &user)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;
    let via = body.via.as_deref().unwrap_or("web");
    answer_item(&state, &item, body.value.trim(), via).await.map_err(to_error_response)?;
    let item = state.store.get_notification(&id, &user).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"notification": item})))
}

/// The surfaces an answer comes from, as an ask records them. The phone is
/// `mobile`; the desktop and the web answer from their Inbox.
fn ask_via(via: &str) -> AnsweredVia {
    match via {
        "mobile" => AnsweredVia::Mobile,
        "chat" => AnsweredVia::Chat,
        "notification" => AnsweredVia::Notification,
        _ => AnsweredVia::Inbox,
    }
}

/// Answer one needs_you item through its own answer path: an ask's, a
/// question's, an approval's, a proposal's, an update's, or an employee's
/// own question (answered into the session that asked). Each path settles
/// the item itself. The ONE dispatch: the answer endpoint and the
/// assistant's `inbox` tool both call it.
pub(crate) async fn answer_item(state: &AppState, item: &Notification, value: &str, via: &str) -> Result<(), NeboError> {
    if item.answered_at.is_some() {
        return Ok(());
    }
    if !item.needs_you && item.options.is_empty() {
        return Err(NeboError::Validation("this item is an update; there is nothing to answer".into()));
    }
    if value.is_empty() {
        return Err(NeboError::Validation("an answer can't be empty".into()));
    }
    let id = item.id.as_str();
    if let Some(ask_id) = id.strip_prefix("permission-ask:") {
        let answer = Answer::parse(value)
            .ok_or_else(|| NeboError::Validation("answer must be allow_always, this_once, no, sent or not_sent".into()))?;
        return match state.permission_asks.answer(ask_id, answer, ask_via(via)) {
            Ok(_) | Err(AskError::Settled(_)) => Ok(()),
            Err(AskError::NotFound) => Err(NeboError::NotFound),
            Err(e) => Err(NeboError::Validation(e.to_string())),
        };
    }
    if let Some(request_id) = id.strip_prefix("question-ask:") {
        if !crate::chat_dispatch::answer_ask(state, request_id, value.to_string()).await {
            // Nothing waits on it any more (the run ended): it is settled.
            tools::owner_notify::settle(&state.store, Some(&|ev, p| state.hub.broadcast(ev, p)), id, "Withdrawn", None);
        }
        return Ok(());
    }
    if let Some(request_id) = id.strip_prefix("approval:") {
        if !crate::handlers::asks::APPROVAL_DECISIONS.iter().any(|(_, d)| *d == value) {
            return Err(NeboError::Validation("answer must be once, always or deny".into()));
        }
        if !crate::chat_dispatch::answer_approval(state, request_id, value).await {
            tools::owner_notify::settle(&state.store, Some(&|ev, p| state.hub.broadcast(ev, p)), id, "Withdrawn", None);
        }
        return Ok(());
    }
    if let Some(key) = id.strip_prefix("held:") {
        return crate::team::answer_held(state, key, value, via).map_err(NeboError::Validation);
    }
    if let Some(rest) = id.strip_prefix("limit:") {
        let project_id = rest.split(':').next().unwrap_or_default();
        let project = state.store.get_project(project_id)?.ok_or(NeboError::NotFound)?;
        return match crate::team::answer_limit(state, &project, value, via).await {
            Ok(true) => Ok(()),
            // Answered already (on the project, or another device).
            Ok(false) => {
                tools::owner_notify::settle(&state.store, Some(&|ev, p| state.hub.broadcast(ev, p)), id, "Answered", Some(via));
                Ok(())
            }
            Err(e) => Err(NeboError::Validation(e)),
        };
    }
    if id.starts_with("handled:") {
        return match value {
            crate::handlers::learned_rules::UNDO => crate::handlers::learned_rules::undo(state, item, via),
            _ => Err(NeboError::Validation("a handled update takes Undo".into())),
        };
    }
    if let Some(pending_id) = id.strip_prefix("learn:") {
        let body = crate::handlers::agents::LearningResolveBody {
            approved: value == "yes",
            never: value == "never",
            content: None,
            via: Some(via.to_string()),
        };
        return crate::handlers::agents::decide_learning(state, pending_id, body)
            .await
            .map(|_| ())
            .map_err(|(_, e)| NeboError::Internal(e.0.error));
    }
    if let Some(rest) = id.strip_prefix("artifact-update:") {
        let artifact_id = rest.split(':').nth(1).unwrap_or_default();
        let broadcast = |ev: &str, p: serde_json::Value| state.hub.broadcast(ev, p);
        if value != "yes" {
            // Later: it stays in Settings → Updates.
            tools::owner_notify::settle(&state.store, Some(&broadcast), id, "Later", Some(via));
            return Ok(());
        }
        return match crate::handlers::artifact_updates::start_update(state, artifact_id) {
            Ok(_) => Ok(()),
            // No longer on offer (applied, or a newer one replaced it).
            Err(_) => {
                tools::owner_notify::settle(&state.store, Some(&broadcast), id, "Updated", Some(via));
                Ok(())
            }
        };
    }
    answer_own_question(state, item, value, via)
}

/// An employee's own question (posted with `push_notification`'s options):
/// the item is settled with the owner's answer and the answer goes to the
/// session that asked, on the wake rail, so the work goes on there.
fn answer_own_question(state: &AppState, item: &Notification, value: &str, via: &str) -> Result<(), NeboError> {
    let Some(reply_to) = item.reply_to.as_deref().filter(|r| !r.is_empty()) else {
        return Err(NeboError::Validation("this item is answered on its own screen".into()));
    };
    let label = item
        .options
        .iter()
        .find(|o| o.value == value)
        .map_or(value, |o| o.label.as_str());
    if !tools::owner_notify::settle(&state.store, Some(&|ev, p| state.hub.broadcast(ev, p)), &item.id, label, Some(via)) {
        return Ok(());
    }
    let text = format!(
        "The owner answered your Inbox question \"{}\": {}. Go on with the work that waited on it.",
        item.title, label
    );
    crate::wake::enqueue(state, reply_to, agent::harness::delegation::notify::WAKE_KIND, &text, &[], 0);
    Ok(())
}

/// The server's side of the assistant's `inbox` tool: its answers go
/// through [`answer_item`], the one answer path.
pub(crate) struct InboxAnswerer(pub AppState);

#[async_trait::async_trait]
impl tools::owner_tools::InboxAnswers for InboxAnswerer {
    async fn answer(&self, id: &str, value: &str, via: &str) -> Result<(), String> {
        let state = &self.0;
        let user = state.store.ensure_local_user_id().map_err(|e| e.to_string())?;
        let item = state
            .store
            .get_notification(id, &user)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no such Inbox item".to_string())?;
        answer_item(state, &item, value, via).await.map_err(|e| e.to_string())
    }
}

/// The morning brief's schedule.
pub(crate) const MORNING_BRIEF: &str = "morning-brief";

/// When the assistant writes it: 8:00 every day, on the owner's clock (the
/// engine reads every schedule in the owner's zone).
pub(crate) const MORNING_BRIEF_AT: &str = "0 0 8 * * *";

/// What the assistant is asked each morning.
pub(crate) const MORNING_BRIEF_ASK: &str = "Write today's morning brief. Load the Inbox tools with find_tools(\"select:read_inbox,write_brief\"), \
     read the Inbox with read_inbox(since_hours: 24), \
     then write it with write_brief: two or three plain sentences, what \
     needs the owner first (and who is waiting), then what went well. Name employees by name, never twice \
     in a row. When you have the owner's email and calendar connected, the brief also gives today's \
     meetings with their times, the emails that need the owner (at most 3, each with a reply drafted and \
     saved as a draft, never sent), and the bills, renewals and deadlines coming up. Don't message the \
     owner; the brief is the message.";

/// Make the assistant's daily brief schedule when it is missing: a
/// `cron_jobs` row the engine fires like every schedule. Made once; the
/// owner may change its time, and that stands.
pub(crate) fn ensure_morning_brief(store: &db::Store) {
    match store.get_cron_job_by_name(MORNING_BRIEF) {
        Ok(Some(_)) => {}
        Ok(None) => {
            let made = store.create_cron_job(
                MORNING_BRIEF,
                MORNING_BRIEF_AT,
                "",
                "agent",
                Some(MORNING_BRIEF_ASK),
                None,
                None,
                true,
                Some(tools::team_tool::PRIMARY_AGENT_ID),
                None,
                None,
                &db::models::ScheduleProvenance::new(db::models::ScheduleCreator::System, "the assistant's daily brief"),
            );
            if let Err(e) = made {
                tracing::warn!(error = %e, "the morning brief's schedule was not made");
            }
        }
        Err(e) => tracing::warn!(error = %e, "the morning brief's schedule could not be read"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tools::owner_clock::OwnerZone;

    /// The assistant writes the brief every day at 8:00 on the owner's
    /// clock: one schedule, made once, owned by the assistant, fired by the
    /// engine in the owner's zone (8:00 in Denver is 15:00 UTC in winter
    /// and 14:00 UTC in summer). Made again, it is not doubled, and a time
    /// the owner chose stands.
    #[test]
    fn the_morning_brief_is_a_daily_schedule_on_the_owners_clock() {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(&dir.path().join("b.db").to_string_lossy()).unwrap();
        ensure_morning_brief(&store);
        let job = store.get_cron_job_by_name(MORNING_BRIEF).unwrap().expect("made");
        assert_eq!(job.agent_id.as_deref(), Some(tools::team_tool::PRIMARY_AGENT_ID));
        assert_eq!((job.task_type.as_str(), job.enabled), ("agent", Some(1)));
        assert!(job.message.as_deref().unwrap_or("").contains("write_brief"));

        let denver = OwnerZone::parse(Some("America/Denver"));
        let utc = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp();
        let next = |from: &str| crate::engine::next_occurrence(&job.schedule, utc(from), denver).unwrap();
        assert_eq!(next("2026-01-15T00:00:00Z"), Some(utc("2026-01-15T15:00:00Z")));
        assert_eq!(next("2026-07-15T00:00:00Z"), Some(utc("2026-07-15T14:00:00Z")));
        assert_eq!(next("2026-07-15T14:00:01Z"), Some(utc("2026-07-16T14:00:00Z")), "every day");

        store
            .upsert_cron_job(
                MORNING_BRIEF, "0 30 6 * * *", "", "agent", job.message.as_deref(), None, None, true,
                job.agent_id.as_deref(), None, None,
                &db::models::ScheduleProvenance::new(db::models::ScheduleCreator::Owner, ""),
            )
            .unwrap();
        ensure_morning_brief(&store);
        assert_eq!(store.get_cron_job_by_name(MORNING_BRIEF).unwrap().unwrap().schedule, "0 30 6 * * *", "the owner's time stands");
        let briefs = store.list_cron_jobs(100, 0).unwrap().into_iter().filter(|j| j.name == MORNING_BRIEF).count();
        assert_eq!(briefs, 1, "one schedule");
    }
}
