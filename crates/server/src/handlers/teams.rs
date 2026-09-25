//! Teams — the platform API over the ONE team core (`tools::team::create`
//! for creation, `crate::team::post` for posting). A team is a local object
//! with its own thread; the hub, when present, is only a mirror.

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;

use crate::handlers::{to_error_response, HandlerResult};
use crate::state::AppState;

/// One member as the app names it.
///
/// An empty `botId` means this machine, and then `agentId` may be a local
/// agent id OR an exact employee name, because that is how the picker has
/// always addressed people here. A member on another computer carries that
/// computer's bot id, and then `agentId` is the hub agent id and `name` is the
/// label to show, since there is no local roster to resolve either from.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberRef {
    #[serde(default)]
    pub bot_id: String,
    pub agent_id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTeamRequest {
    pub name: String,
    #[serde(default)]
    pub mission: String,
    /// Everyone in the team, local and remote.
    #[serde(default)]
    pub members: Vec<MemberRef>,
    /// The lead (local agent id or exact name). Empty or absent = the owner leads.
    #[serde(default)]
    pub organizer_agent_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTeamRequest {
    pub name: Option<String>,
    pub mission: Option<String>,
    /// Full member list, local and remote; absent = unchanged.
    pub members: Option<Vec<MemberRef>>,
    /// The lead (local agent id or exact name); "" = the owner leads; absent = unchanged.
    pub organizer_agent_id: Option<String>,
}

/// Turn what the app named into what the store holds.
///
/// A local member is resolved against this machine's roster, so an employee
/// that is not installed is a refusal rather than a member nobody can reach. A
/// remote member is taken as given: this machine has no way to check another
/// computer's roster, and refusing what it cannot verify would make a
/// cross-bot team impossible to create at all.
fn resolve_members(
    state: &AppState,
    refs: &[MemberRef],
) -> Result<Vec<db::TeamMember>, (reqwest::StatusCode, axum::Json<types::api::ErrorResponse>)> {
    let mut out: Vec<db::TeamMember> = Vec::new();
    for r in refs {
        let member = if r.bot_id.is_empty() {
            let Some(agent) = tools::team::resolve_agent(&state.store, &r.agent_id) else {
                return Err(to_error_response(types::NeboError::Validation(format!(
                    "No employee named \"{}\" is installed",
                    r.agent_id
                ))));
            };
            db::TeamMember::local(agent.id)
        } else {
            db::TeamMember {
                bot_id: r.bot_id.clone(),
                agent_id: r.agent_id.clone(),
                name: r.name.clone(),
            }
        };
        if !out.iter().any(|m| m.agent_id == member.agent_id) {
            out.push(member);
        }
    }
    Ok(out)
}

/// PUT /teams/{teamId} — rename, re-mission, or change members (`edit_team`: the
/// commander graph already owns the `update_team` name). The rules
/// (two-member floor, organizer stays) live in `tools::team::update`.
pub async fn edit_team(
    State(state): State<AppState>,
    Path(team_id): Path<String>,
    Json(body): Json<UpdateTeamRequest>,
) -> HandlerResult<serde_json::Value> {
    let members = match &body.members {
        Some(refs) => Some(resolve_members(&state, refs)?),
        None => None,
    };
    let organizer = match body.organizer_agent_id.as_deref() {
        None => None,
        Some("") => Some(String::new()),
        Some(label) => Some(resolve_label(&state, label)?),
    };
    let team = tools::team::update(
        &state.store,
        &team_id,
        body.name.as_deref(),
        body.mission.as_deref(),
        members.as_deref(),
        organizer.as_deref(),
    )
    .map_err(|e| to_error_response(types::NeboError::Validation(e)))?;

    state.hub.broadcast(
        tools::team::TEAM_UPDATED_EVENT,
        serde_json::json!({ "team": team }),
    );
    Ok(Json(serde_json::json!({ "team": team })))
}

/// An employee label (local id or exact name) → local id, or the 400 that names it.
fn resolve_label(
    state: &AppState,
    label: &str,
) -> Result<String, (axum::http::StatusCode, Json<crate::handlers::ErrorResponse>)> {
    tools::team::resolve_agent(&state.store, label)
        .map(|a| a.id)
        .ok_or_else(|| {
            to_error_response(types::NeboError::Validation(format!(
                "No employee named \"{label}\" is installed"
            )))
        })
}

/// POST /teams — create (open) a team. Never touches the hub unless this Nebo is
/// in a hub loop, in which case the team is also mirrored.
pub async fn open_team(
    State(state): State<AppState>,
    Json(body): Json<CreateTeamRequest>,
) -> HandlerResult<serde_json::Value> {
    let comm = state.comm_manager.active_plugin().await;
    let members = resolve_members(&state, &body.members)?;
    // Created from the app: the owner leads unless a member is named lead.
    let organizer = if body.organizer_agent_id.is_empty() {
        String::new()
    } else {
        resolve_label(&state, &body.organizer_agent_id)?
    };
    let team = tools::team::create(
        comm.as_ref(),
        &state.store,
        &body.name,
        &body.mission,
        &members,
        &organizer,
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
    /// Uploaded files (POST /files/upload metadata), as the chat composer sends them.
    #[serde(default)]
    pub attachments: Vec<comm::wire::Attachment>,
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
            attachments: body.attachments,
            team_id: team.id,
            from_agent_id: String::new(),
            text: text.to_string(),
            mention,
            handoff_depth: 0,
            provenance: Vec::new(),
            is_reply: false,
            // The owner reads the team thread.
            reply_to: None,
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

/// GET /teams/other-computers — the employees a team can borrow from the
/// owner's other machines.
///
/// Grouped by computer, because that is how the owner thinks about them: not
/// one flat list of strangers, but "the people on Dwight". This machine's own
/// employees are left out — they are already the local roster.
///
/// It answers with an empty list rather than an error when the hub is not
/// reachable, because an unlinked Nebo genuinely has no other computers, and a
/// picker that errors would suggest something is broken when nothing is.
pub async fn other_computers(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let Some(plugin) = state.comm_manager.active_plugin().await else {
        return Ok(Json(serde_json::json!({ "computers": [] })));
    };
    let mine = config::read_bot_id().unwrap_or_default();
    let mut computers: Vec<serde_json::Value> = Vec::new();
    let loops = plugin.list_loops().await.unwrap_or_default();
    // One agent can only be in one loop, so a bot seen in an earlier loop is
    // not listed twice.
    let mut seen_bots: Vec<String> = Vec::new();
    for l in loops {
        let agents = match plugin.list_loop_agents(&l.id).await {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(error = %e, loop_id = %l.id, "teams: could not read the loop roster");
                continue;
            }
        };
        let mut by_bot: std::collections::BTreeMap<(String, String), Vec<serde_json::Value>> =
            std::collections::BTreeMap::new();
        for a in agents {
            if a.bot_id == mine || a.bot_id.is_empty() {
                continue;
            }
            by_bot
                .entry((a.bot_id.clone(), a.bot_name.clone()))
                .or_default()
                .push(serde_json::json!({
                    "agentId": a.id,
                    "name": a.name,
                    "slug": a.slug,
                }));
        }
        for ((bot_id, bot_name), employees) in by_bot {
            if seen_bots.contains(&bot_id) {
                continue;
            }
            seen_bots.push(bot_id.clone());
            computers.push(serde_json::json!({
                "botId": bot_id,
                "botName": bot_name,
                "employees": employees,
            }));
        }
    }
    Ok(Json(serde_json::json!({ "computers": computers })))
}

/// Resolve the request's `mention` labels to member ids; labels that are
/// not members are dropped (the owner's composer only offers members).
fn mention_ids(store: &db::Store, team: &db::Team, labels: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for label in labels {
        let is_member = |id: &String| team.members.iter().any(|m| m.agent_id == *id);
        // A remote member is named by its hub id, which no local lookup would
        // find, so an id that is already a member is taken as it stands.
        let id = if is_member(label) {
            Some(label.clone())
        } else {
            tools::team::resolve_agent(store, label)
                .map(|a| a.id)
                .filter(is_member)
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
    disband(&state, &team_id).map_err(|e| to_error_response(types::NeboError::Database(e)))?;
    Ok(Json(serde_json::json!({
        "message": "Team removed"
    })))
}

/// Remove a team: the owner's delete, and a temporary team's end once its
/// outcome reached the owner. Its thread stays (see `remove_team`).
pub(crate) fn disband(state: &AppState, team_id: &str) -> Result<(), String> {
    state.store.delete_team(team_id).map_err(|e| format!("delete team: {e}"))?;
    state.hub.broadcast(tools::team::TEAM_REMOVED_EVENT, serde_json::json!({ "teamId": team_id }));
    Ok(())
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
            .create_team(
                "t-1",
                "Ops",
                "",
                &[db::TeamMember::local("chief"), db::TeamMember::local("ea")],
                "",
                None,
            )
            .unwrap();
        let ids = mention_ids(
            &s,
            &team,
            &["ea".into(), "Chief of Staff".into(), "ea".into(), "Outsider".into(), "nobody".into()],
        );
        assert_eq!(ids, vec!["ea".to_string(), "chief".to_string()]);
    }
}
