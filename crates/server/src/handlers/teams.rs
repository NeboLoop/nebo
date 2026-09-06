//! Teams — the platform API over the ONE team core (`tools::team::create`
//! for creation, `crate::team::post` for posting). A team is a local object
//! with its own thread; the hub, when present, is only a mirror.

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;

use crate::handlers::{to_error_response, HandlerResult};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTeamRequest {
    pub name: String,
    #[serde(default)]
    pub mission: String,
    /// Local agent ids (or exact employee names) of the members.
    #[serde(default)]
    pub agent_ids: Vec<String>,
}

/// POST /teams — create (open) a team. Never touches the hub unless this Nebo is
/// in a hub loop, in which case the team is also mirrored.
pub async fn open_team(
    State(state): State<AppState>,
    Json(body): Json<CreateTeamRequest>,
) -> HandlerResult<serde_json::Value> {
    let comm = state.comm_manager.active_plugin().await;
    let mut member_ids: Vec<String> = Vec::new();
    for label in &body.agent_ids {
        let Some(agent) = tools::team::resolve_agent(&state.store, label) else {
            return Err(to_error_response(types::NeboError::Validation(format!(
                "No employee named \"{label}\" is installed"
            ))));
        };
        if !member_ids.contains(&agent.id) {
            member_ids.push(agent.id);
        }
    }
    // Created from the app: the owner is the organizer.
    let team = tools::team::create(
        comm.as_ref(),
        &state.store,
        &body.name,
        &body.mission,
        &member_ids,
        "",
    )
    .await
    .map_err(|e| to_error_response(types::NeboError::Validation(e)))?;

    state.hub.broadcast(
        tools::team::TEAM_CREATED_EVENT,
        serde_json::json!({ "team": team }),
    );

    Ok(Json(serde_json::json!({ "team": team })))
}

/// GET /teams — the sidebar's team list.
pub async fn list_teams(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let teams = state.store.list_teams().map_err(to_error_response)?;
    let total = teams.len();
    Ok(Json(serde_json::json!({
        "teams": teams,
        "total": total,
    })))
}

#[derive(Debug, Deserialize)]
pub struct SendTeamMessageRequest {
    pub text: String,
    /// Members asked to act (local agent ids or names). Without it, every
    /// member may answer once.
    #[serde(default)]
    pub mention: Vec<String>,
}

/// POST /teams/{teamId}/messages — the owner posts into the team. The
/// owner's post always reaches everyone and re-opens the floor.
pub async fn send_team_message(
    State(state): State<AppState>,
    Path(team_id): Path<String>,
    Json(body): Json<SendTeamMessageRequest>,
) -> HandlerResult<serde_json::Value> {
    let text = body.text.trim();
    if text.is_empty() {
        return Err(to_error_response(types::NeboError::Validation(
            "text required".into(),
        )));
    }
    let team = state
        .store
        .get_team(&team_id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    let mention = mention_ids(&state.store, &team, &body.mention);

    let receipt = crate::team::post(
        state.clone(),
        tools::coworker::TeamPost {
            team_id: team.id,
            from_agent_id: String::new(),
            text: text.to_string(),
            mention,
            handoff_depth: 0,
            provenance: Vec::new(),
            is_reply: false,
        },
    )
    .await
    .map_err(|e| to_error_response(types::NeboError::Internal(e)))?;

    Ok(Json(serde_json::json!({
        "message": "Sent",
        "messageId": receipt.message_id,
        "asked": receipt.asked,
    })))
}

/// Resolve the request's `mention` labels to member ids; labels that are
/// not members are dropped (the owner's composer only offers members).
fn mention_ids(store: &db::Store, team: &db::Team, labels: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for label in labels {
        let id = if team.member_agent_ids.contains(label) {
            Some(label.clone())
        } else {
            tools::team::resolve_agent(store, label)
                .map(|a| a.id)
                .filter(|id| team.member_agent_ids.contains(id))
        };
        if let Some(id) = id {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// GET /teams/{teamId}/messages — the team's local thread. Initial load
/// only; live updates arrive as `team_message` events, never by polling.
pub async fn get_team_messages(
    State(state): State<AppState>,
    Path(team_id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    if state
        .store
        .get_team(&team_id)
        .map_err(to_error_response)?
        .is_none()
    {
        return Err(to_error_response(types::NeboError::NotFound));
    }
    let messages: Vec<db::TeamMessage> = state
        .store
        .list_team_messages(&team_id, 200)
        .map_err(to_error_response)?
        .into_iter()
        .map(|mut m| {
            // Older rows (mirrored hub traffic) may lack a sender name; the
            // owner reads names, never ids.
            if m.from.is_empty() {
                m.from = if m.from_agent_id.is_empty() {
                    if m.role == "user" { "Owner".to_string() } else { String::new() }
                } else {
                    state
                        .store
                        .get_agent(&m.from_agent_id)
                        .ok()
                        .flatten()
                        .map(|a| a.name)
                        .unwrap_or_else(|| m.from_agent_id.clone())
                };
            }
            m
        })
        .collect();
    Ok(Json(serde_json::json!({ "messages": messages })))
}

/// DELETE /teams/{teamId} — remove (forget) the team. Its thread (and any hub
/// channel) stays: conversations are records; deleting the team is a
/// sidebar decision, not a history purge.
pub async fn remove_team(
    State(state): State<AppState>,
    Path(team_id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state.store.delete_team(&team_id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({
        "message": "Team removed"
    })))
}

#[cfg(test)]
mod tests {
    use super::mention_ids;

    fn store() -> db::Store {
        let path = std::env::temp_dir().join(format!("nebo-teams-handler-{}.db", uuid::Uuid::new_v4()));
        db::Store::new(&path.to_string_lossy()).expect("store")
    }

    /// The owner's `mention` list resolves by member id or by employee name,
    /// deduplicates, and drops anyone who is not in the team.
    #[test]
    fn mentions_resolve_to_members_only() {
        let s = store();
        s.create_agent("chief", None, "Chief of Staff", "d", "# a", "", None, None).unwrap();
        s.create_agent("ea", None, "Executive Assistant", "d", "# a", "", None, None).unwrap();
        s.create_agent("out", None, "Outsider", "d", "# a", "", None, None).unwrap();
        let team = s
            .create_team("t-1", "Ops", "", &["chief".into(), "ea".into()], "", None)
            .unwrap();
        let ids = mention_ids(
            &s,
            &team,
            &["ea".into(), "Chief of Staff".into(), "ea".into(), "Outsider".into(), "nobody".into()],
        );
        assert_eq!(ids, vec!["ea".to_string(), "chief".to_string()]);
    }
}
