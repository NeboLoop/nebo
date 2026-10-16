//! Teams — the platform API over the ONE team core (`tools::team::create`
//! for creation, `crate::team::post` for posting). A team is a local object
//! with its own thread; the hub, when present, is only a mirror.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};

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
    /// "temporary": assembled for one piece of work, disbanded once its
    /// outcome reaches the owner (it needs a lead). Absent or "saved": stays.
    #[serde(default)]
    pub lifetime: Option<String>,
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
    /// What doesn't change for the team (brand, rules, who does what); absent = unchanged.
    pub standing: Option<String>,
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

/// PUT /teams/{teamId} — rename, re-mission, or change members (`edit_team`).
/// The rules (two-member floor, organizer stays) live in `tools::team::update`.
/// `changed` is false when the team already was what the request asks for
/// (every member named already on it, the same lead): nothing was written
/// and nothing is announced.
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
    let updated = tools::team::update(
        &state.store,
        &team_id,
        body.name.as_deref(),
        body.mission.as_deref(),
        members.as_deref(),
        organizer.as_deref(),
        body.standing.as_deref(),
    )
    .map_err(|e| to_error_response(types::NeboError::Validation(e)))?;

    if updated.changed {
        state.hub.broadcast(
            tools::team::TEAM_UPDATED_EVENT,
            serde_json::json!({ "team": updated.team }),
        );
    }
    Ok(Json(serde_json::json!({ "team": updated.team, "changed": updated.changed })))
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
    let lifetime = match body.lifetime.as_deref() {
        None | Some("saved") => tools::Lifetime::Saved,
        Some("temporary") => tools::Lifetime::Temporary { report_to: String::new() },
        Some(other) => {
            return Err(to_error_response(types::NeboError::Validation(format!(
                "lifetime is \"temporary\" or \"saved\", not {other:?}"
            ))));
        }
    };
    let team = tools::team::create(
        comm.as_ref(),
        &state.store,
        &body.name,
        &body.mission,
        &members,
        &organizer,
        &lifetime,
    )
    .await
    .map_err(|e| to_error_response(types::NeboError::Validation(e)))?;

    state.hub.broadcast(
        tools::team::TEAM_CREATED_EVENT,
        serde_json::json!({ "team": team }),
    );

    Ok(Json(serde_json::json!({ "team": team })))
}

/// One row of the roster's team list: the team, where it sits in the
/// owner's Pinned list (null when it is not pinned), its projects with
/// their states (the row's "N live", or "done" once none is going), and the
/// newest post in any of its threads (the row's latest line).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RosterTeam {
    #[serde(flatten)]
    pub team: db::Team,
    pub pin_position: Option<usize>,
    pub projects: Vec<RosterProject>,
    pub latest: Option<db::TeamMessage>,
}

/// A project on a team's roster row: which one, and its state (`live`,
/// `paused` or `done`), kept current by `project_updated`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RosterProject {
    pub id: String,
    pub state: String,
}

/// GET /teams — the sidebar's team list, in the owner's order (his order,
/// then A to Z), each row with its place among the pins.
pub async fn list_teams(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let mut teams = state.store.list_teams().map_err(to_error_response)?;
    let order = crate::handlers::agents::roster_order(&state.store);
    let rows: Vec<(&str, &str)> = teams.iter().map(|t| (t.id.as_str(), t.name.as_str())).collect();
    let arranged = db::models::RosterOrder::arrange(&order.teams, None, &rows);
    let mut slots: Vec<Option<db::Team>> = teams.drain(..).map(Some).collect();
    let mut teams: Vec<RosterTeam> = Vec::new();
    for team in arranged.into_iter().filter_map(|i| slots[i].take()) {
        let projects = state
            .store
            .list_team_projects(&team.id)
            .map_err(to_error_response)?
            .into_iter()
            .map(|p| RosterProject { id: p.id, state: p.state })
            .collect();
        let latest = state.store.latest_team_message(&team.id).map_err(to_error_response)?;
        teams.push(RosterTeam {
            pin_position: order.pin_position(
                db::models::RosterKind::Team,
                &team.id,
                tools::team_tool::PRIMARY_AGENT_ID,
            ),
            projects,
            latest,
            team,
        });
    }
    let total = teams.len();
    // The teams with a member's post the owner has not read: the sidebar's
    // "New reply" dot. Cleared by `PUT /teams/{teamId}/read`. A project's
    // thread is listed on its own (`unreadProjects`, cleared by
    // `PUT /projects/{id}/read`), so an app that knows no projects never
    // shows a dot it has no way to clear.
    let mut unread_teams: Vec<String> = Vec::new();
    let mut unread_projects: Vec<serde_json::Value> = Vec::new();
    for key in state.store.unread_conversations().map_err(to_error_response)? {
        match db::team_thread_of(&key) {
            Some(db::TeamThread::Team(id)) => unread_teams.push(id.to_string()),
            Some(db::TeamThread::Project { team_id, project_id }) => {
                unread_projects.push(serde_json::json!({ "teamId": team_id, "projectId": project_id }))
            }
            None => {}
        }
    }
    Ok(Json(serde_json::json!({
        "teams": teams,
        "total": total,
        "unreadTeams": unread_teams,
        "unreadProjects": unread_projects,
    })))
}

/// PUT /teams/{teamId}/read — the owner has the team's conversation open:
/// its newest post from a member is read, on every surface (`chat_read`).
pub async fn mark_team_read(
    State(state): State<AppState>,
    Path(team_id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    crate::handlers::chat::conversation_read(&state, &db::team_thread_key(&team_id)).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
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
/// owner's post always reaches everyone and re-opens the floor. A command
/// he types here (`/clear`, `/stop`, …) is run at this door and never
/// posted: `command` names it, `message` is its answer, and `messageId` is
/// the row it left in the thread, if any (the divider a clear leaves).
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
    if let Some(command) = command(&state, &team, text).await {
        let command = command.map_err(|e| to_error_response(types::NeboError::Internal(e)))?;
        return Ok(Json(serde_json::json!({
            "message": command.reply,
            "messageId": command.message_id,
            "asked": Vec::<String>::new(),
            "command": command.name,
        })));
    }
    let mention = mention_ids(&state.store, &team, &body.mention);

    let receipt = crate::team::post(state.clone(), owners_post(&team, None, text, body.attachments, mention))
        .await
        .map_err(|e| to_error_response(types::NeboError::Internal(e)))?;

    Ok(Json(serde_json::json!({
        "message": "Sent",
        "messageId": receipt.message_id,
        "asked": receipt.asked,
        "command": "",
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopTeamWorkRequest {
    /// One member's work only (a local agent id). Absent: every member's.
    #[serde(default)]
    pub agent_id: String,
    /// One helper that member started only.
    #[serde(default)]
    pub task_id: String,
}

/// POST /teams/{teamId}/stop — the team's Stop: every member's work in the
/// team's conversation, one member's (`agentId`), or one helper a member
/// started there (`agentId` + `taskId`). Never anything outside the team.
pub async fn stop_team_work(
    State(state): State<AppState>,
    Path(team_id): Path<String>,
    Json(body): Json<StopTeamWorkRequest>,
) -> HandlerResult<serde_json::Value> {
    let team = state
        .store
        .get_team(&team_id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    let target = match (body.agent_id.as_str(), body.task_id.as_str()) {
        ("", _) => Target::Everyone,
        (member, "") => Target::Member(member),
        (member, task_id) => Target::Helper { member, task_id },
    };
    let stopped = stop(&state, &team, None, StopBy::Owner, target)
        .await
        .map_err(|e| to_error_response(types::NeboError::Internal(e)))?;
    Ok(Json(serde_json::json!({
        "message": stopped_line(&stopped),
        "stopped": stopped,
    })))
}

/// GET /teams/{teamId}/working — what runs in the team's conversation now:
/// each member working in its seat for the team, and each helper a member
/// started there, with what it is doing and the chat that holds its steps.
/// Read once when the team opens; every change after arrives as the events
/// the work already sends (`tool_start`, `chat_complete`, `subagent_*`,
/// `team_activity`), never by polling.
pub async fn team_working(
    State(state): State<AppState>,
    Path(team_id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let team = state
        .store
        .get_team(&team_id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(serde_json::json!({ "working": working(&state, &team, None).await })))
}

/// What runs in a team's conversation, or in one of its projects', now:
/// each local member working in its seat there, and each helper it started.
async fn working(state: &AppState, team: &db::Team, project: Option<&db::Project>) -> Vec<serde_json::Value> {
    let sessions = state.harness.sessions();
    let chat_of = |key: &str| sessions.resolve_session_id_by_key(key).map(|sid| sessions.active_chat_id(&sid)).unwrap_or_default();
    let mut working: Vec<serde_json::Value> = Vec::new();
    for (member_id, name) in tools::team::member_roster(&state.store, team) {
        if !team.members.iter().any(|m| m.agent_id == member_id && m.is_local()) {
            continue;
        }
        let (seat, _) = crate::coworker::team_seat(&member_id, team, project);
        if let Some(run) = state.run_registry.find_by_session(&seat).await {
            working.push(serde_json::json!({
                "kind": "member",
                "agentId": member_id,
                "member": name,
                "title": name,
                "taskId": "",
                "call": run.call,
                "activity": "",
                "sessionKey": seat,
                "chatId": chat_of(&seat),
            }));
        }
        for helper in state.helpers.list(Some(&seat)).into_iter().filter(|h| h.running) {
            working.push(serde_json::json!({
                "kind": "helper",
                "agentId": member_id,
                "member": name,
                "title": helper.description,
                "taskId": helper.task_id,
                "call": helper.call,
                "activity": helper.activity,
                "sessionKey": helper.session_key,
                "chatId": chat_of(&helper.session_key),
            }));
        }
    }
    working
}

/// A command the owner typed in a team's composer, run at the door.
pub(crate) struct Command {
    /// The command, without its slash.
    pub name: &'static str,
    /// What the owner reads.
    pub reply: String,
    /// The row it left in the team thread; empty when it left none.
    pub message_id: String,
}

/// What `/help` says in a team, and what a chat-only command is answered
/// with there.
const TEAM_COMMANDS: &str = "In a team, /clear starts the conversation fresh (every message is kept), and /stop stops the team's work.";

/// The owner's commands in a team thread. `None`: the text is not one of
/// them, and is posted. A command is never posted to the team, so no member
/// reads it as a message: the chat's commands that have no meaning for a
/// team are answered with the ones that do.
pub(crate) async fn command(state: &AppState, team: &db::Team, text: &str) -> Option<Result<Command, String>> {
    let word = text.split_whitespace().next().unwrap_or_default().to_lowercase();
    let reply = |name: &'static str, reply: String| Some(Ok(Command { name, reply, message_id: String::new() }));
    match word.as_str() {
        "/clear" => Some(clear_team(state, team).await.map(|message_id| Command {
            name: "clear",
            reply: "Context cleared — fresh start.".to_string(),
            message_id,
        })),
        "/stop" | "/cancel" | "/halt" => match stop(state, team, None, StopBy::Owner, Target::Everyone).await {
            Ok(stopped) => reply("stop", stopped_line(&stopped)),
            Err(e) => Some(Err(e)),
        },
        "/new" => reply("new", "A team keeps one conversation. /clear starts it fresh; every message is kept.".to_string()),
        "/help" => reply("help", TEAM_COMMANDS.to_string()),
        "/compact" | "/model" | "/status" | "/goal" => {
            reply("other", format!("{word} works in an employee's own chat. {TEAM_COMMANDS}"))
        }
        _ => None,
    }
}

/// Who asks to stop a team's work.
pub(crate) enum StopBy<'a> {
    /// The owner: the team thread's Stop, or `/stop` there.
    Owner,
    /// An employee through `stop_team`: only the team's lead, and only while
    /// it serves the owner's own request (`coworker::seat_authority`).
    Employee { agent_id: &'a str, session_key: &'a str, owners_turn: Option<&'a str> },
}

/// What a stop stops in a team.
pub(crate) enum Target<'a> {
    /// Every member's work there.
    Everyone,
    /// One member's work there (a local agent id).
    Member(&'a str),
    /// One helper a member started there.
    Helper { member: &'a str, task_id: &'a str },
}

/// Stop the team's work: each local member's running turn in its seat for
/// the team, and the helpers that turn started, through the ONE stop path
/// (`chat_dispatch::stop_session`; a linked member's turn cancels its
/// agent's prompt with it). The members' other conversations are left
/// alone, and so is the lead that asked. The owner may stop one member's
/// work there, or one helper a member started there; an employee stops the
/// team as a whole or not at all. Returns who (or what) was stopped.
pub(crate) async fn stop(
    state: &AppState,
    team: &db::Team,
    project: Option<&db::Project>,
    by: StopBy<'_>,
    target: Target<'_>,
) -> Result<Vec<String>, String> {
    let asker = match by {
        StopBy::Owner => "",
        StopBy::Employee { agent_id, session_key, owners_turn } => {
            if tools::team::lead_of(team) != Some(agent_id) {
                return Err(format!(
                    "Only the {} team's lead can stop its work, when the owner asks. Nothing was stopped.",
                    team.name
                ));
            }
            let authority = crate::coworker::seat_authority(state, session_key, agent_id, owners_turn);
            if authority.owners_request().is_none() {
                return Err(format!(
                    "Stopping the {} team's work takes the owner's own request. Nothing was stopped; ask the owner.",
                    team.name
                ));
            }
            agent_id
        }
    };
    let roster = tools::team::member_roster(&state.store, team);
    if let Target::Helper { member, task_id } = target {
        let (seat, _) = crate::coworker::team_seat(member, team, project);
        let Some(helper) = state.helpers.list(Some(&seat)).into_iter().find(|h| h.task_id == task_id && h.running) else {
            return Ok(Vec::new());
        };
        state.helpers.stop(&seat, task_id)?;
        let name = roster.iter().find(|(id, _)| id == member).map(|(_, n)| n.as_str()).unwrap_or(member);
        return Ok(vec![format!("{name}'s helper ({})", helper.description)]);
    }
    let mut stopped: Vec<String> = Vec::new();
    // Members on other computers: the stop goes to them through the team's
    // hub channel, the one way they hear the team. A project's work is all
    // on this computer (`team::post`).
    let remote_wanted = project.is_none() && team.members.iter().any(|m| {
        !m.is_local()
            && match target {
                Target::Member(id) => m.agent_id == id,
                _ => true,
            }
    });
    if remote_wanted && crate::team::stop_on_hub(state, team).await {
        tracing::info!(team = %team.id, "team stop sent to the members on other computers");
    }
    for (member_id, name) in roster {
        let local = team.members.iter().any(|m| m.agent_id == member_id && m.is_local());
        let wanted = match target {
            Target::Member(id) => member_id == id,
            _ => true,
        };
        if !local || !wanted || member_id == asker {
            continue;
        }
        let (seat, _) = crate::coworker::team_seat(&member_id, team, project);
        if crate::chat_dispatch::stop_session(&state.store, &state.helpers, &state.run_registry, &seat).await {
            let mut payload = serde_json::json!({ "teamId": team.id, "agentId": member_id, "agentName": name, "state": "stopped" });
            let event = match project {
                Some(p) => {
                    payload["projectId"] = serde_json::json!(p.id);
                    tools::team::PROJECT_ACTIVITY_EVENT
                }
                None => tools::team::TEAM_ACTIVITY_EVENT,
            };
            state.hub.broadcast(event, payload);
            stopped.push(name);
        }
    }
    Ok(stopped)
}

/// What a stop says: who was stopped, in plain words.
pub(crate) fn stopped_line(stopped: &[String]) -> String {
    match stopped.split_last() {
        None => "Nothing was running in the team.".to_string(),
        Some((only, [])) => format!("Stopped the team's work: {only}."),
        Some((last, rest)) => format!("Stopped the team's work: {} and {last}.", rest.join(", ")),
    }
}

/// The owner's `/clear` in a team: the team's project starts over. The
/// team's work stops first; then the team thread keeps every post and shows
/// the divider where it was cleared, and each local member's seat for the
/// team is cleared the way a chat is (`checkpoint::clear`: its model starts
/// after the divider, a linked member's session is forgotten). No member is
/// briefed with a post from before it again (`coworker::team_context`).
/// Returns the divider's row id.
async fn clear_team(state: &AppState, team: &db::Team) -> Result<String, String> {
    stop(state, team, None, StopBy::Owner, Target::Everyone).await?;
    let chat = state.store.ensure_team_thread(&db::team_thread_key(&team.id), &team.name).map_err(|e| format!("open team thread: {e}"))?;
    let divider = agent::harness::compact::checkpoint::clear(&state.store, &chat).map_err(|e| format!("clear team thread: {e}"))?;
    let sessions = state.harness.sessions();
    for member in team.members.iter().filter(|m| m.is_local()) {
        let (seat, _) = crate::coworker::team_seat(&member.agent_id, team, None);
        let Ok(sid) = sessions.resolve_session_id_by_key(&seat) else {
            continue;
        };
        agent::harness::compact::checkpoint::clear(&state.store, &sessions.active_chat_id(&sid))
            .map_err(|e| format!("clear a member's seat: {e}"))?;
        let _ = state.store.reset_session_counters(&sid);
    }
    state.hub.broadcast(
        tools::team::TEAM_MESSAGE_EVENT,
        serde_json::json!({
            "teamId": team.id,
            "messageId": divider.id,
            "from": "",
            "fromAgentId": "",
            "senderName": "",
            "role": "system",
            "text": divider.content,
            "attachments": [],
            "cleared": true,
        }),
    );
    Ok(divider.id)
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
    let messages = thread_messages(&state, &db::team_thread_key(&team_id)).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "messages": messages })))
}

/// A team-style thread as the app reads it: its newest 200 rows, each with
/// its sender's name.
fn thread_messages(state: &AppState, key: &str) -> Result<Vec<db::TeamMessage>, types::NeboError> {
    Ok(state
        .store
        .list_team_messages(key, 200)?
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
        .collect())
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

// ── Projects ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectRequest {
    /// What the project is called. Empty: the start of the ask.
    #[serde(default)]
    pub title: String,
    /// What the owner asks the team to do: the project's first post.
    pub ask: String,
    /// Uploaded files (POST /files/upload metadata) riding the ask.
    #[serde(default)]
    pub attachments: Vec<comm::wire::Attachment>,
}

/// Longest title a project takes from its ask.
const PROJECT_TITLE_CHARS: usize = 60;

/// A project's title: the one given, or the ask's first line, cut at a word.
fn project_title(title: &str, ask: &str) -> String {
    let title = title.trim();
    if !title.is_empty() {
        return title.to_string();
    }
    let line = ask.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    match line.char_indices().nth(PROJECT_TITLE_CHARS) {
        None => line.to_string(),
        Some((cut, _)) => {
            let head = &line[..cut];
            let head = head.rsplit_once(' ').map(|(h, _)| h).filter(|h| !h.is_empty()).unwrap_or(head);
            format!("{}…", head.trim_end())
        }
    }
}

fn load_team(state: &AppState, team_id: &str) -> Result<db::Team, (axum::http::StatusCode, Json<crate::handlers::ErrorResponse>)> {
    state
        .store
        .get_team(team_id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))
}

/// A project and its team.
fn load_project(
    state: &AppState,
    id: &str,
) -> Result<(db::Project, db::Team), (axum::http::StatusCode, Json<crate::handlers::ErrorResponse>)> {
    let project = state
        .store
        .get_project(id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    let team = load_team(state, &project.team_id)?;
    Ok((project, team))
}

/// Tell every open view a project changed (created, or its state moved).
pub(crate) fn announce_project(state: &AppState, project: &db::Project) {
    state.hub.broadcast(tools::team::PROJECT_UPDATED_EVENT, serde_json::json!({ "project": project }));
}

/// GET /teams/{teamId}/projects — the team's projects: live and paused
/// first, newest first, then the done ones.
pub async fn list_projects(
    State(state): State<AppState>,
    Path(team_id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    load_team(&state, &team_id)?;
    let projects = state.store.list_team_projects(&team_id).map_err(to_error_response)?;
    let total = projects.len();
    Ok(Json(serde_json::json!({ "projects": projects, "total": total })))
}

/// POST /teams/{teamId}/projects — the owner's "New project": the project,
/// its own thread, and his ask as its first post, which goes to the team's
/// lead like any post that names nobody (the lead answers it or hands steps
/// to teammates by name). A project has a lead who owns it, so an
/// owner-led team names one first.
pub async fn create_project(
    State(state): State<AppState>,
    Path(team_id): Path<String>,
    Json(body): Json<CreateProjectRequest>,
) -> HandlerResult<serde_json::Value> {
    let team = load_team(&state, &team_id)?;
    let ask = body.ask.trim();
    if ask.is_empty() {
        return Err(to_error_response(types::NeboError::Validation("ask required".into())));
    }
    let Some(lead) = tools::team::lead_of(&team).map(str::to_string) else {
        return Err(to_error_response(types::NeboError::Validation(format!(
            "A project has a lead who owns it, and the {} team has none. Pick its lead first.",
            team.name
        ))));
    };
    let id = uuid::Uuid::new_v4().to_string();
    let title = project_title(&body.title, ask);
    let project = state
        .store
        .create_project(&db::NewProject {
            id: &id,
            team_id: &team.id,
            lead_agent_id: &lead,
            title: &title,
            ask,
            asker_session_key: "",
            asker_agent_id: "",
            playbook_binding: None,
        })
        .map_err(to_error_response)?;
    announce_project(&state, &project);
    let receipt = crate::team::post(state.clone(), owners_post(&team, Some(&project), ask, body.attachments, Vec::new()))
        .await
        .map_err(|e| to_error_response(types::NeboError::Internal(e)))?;
    Ok(Json(serde_json::json!({
        "project": project,
        "messageId": receipt.message_id,
        "asked": receipt.asked,
    })))
}

/// The owner's own post, typed in the app, into a team or one of its projects.
fn owners_post(
    team: &db::Team,
    project: Option<&db::Project>,
    text: &str,
    attachments: Vec<comm::wire::Attachment>,
    mention: Vec<String>,
) -> tools::coworker::TeamPost {
    tools::coworker::TeamPost {
        attachments,
        team_id: team.id.clone(),
        project_id: project.map(|p| p.id.clone()),
        from_agent_id: String::new(),
        // The owner typed it: his own request.
        by_owner: true,
        owners_turn: None,
        text: text.to_string(),
        mention,
        handoff_depth: 0,
        provenance: Vec::new(),
        // His own post from the app: no conversation's mode.
        narrow: None,
        // The owner reads the thread.
        reply_to: None,
    }
}

/// GET /projects/{id}/messages — the project's thread. Initial load only;
/// live updates arrive as `project_message` events.
pub async fn get_project_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let (project, _) = load_project(&state, &id)?;
    let messages = thread_messages(&state, &project.thread_session_key).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "messages": messages })))
}

/// POST /projects/{id}/messages — the owner writes in a project's thread:
/// to its lead when he names nobody, or to the members he names.
pub async fn send_project_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SendTeamMessageRequest>,
) -> HandlerResult<serde_json::Value> {
    let (project, team) = load_project(&state, &id)?;
    let text = body.text.trim();
    if text.is_empty() {
        return Err(to_error_response(types::NeboError::Validation("text required".into())));
    }
    let mention = mention_ids(&state.store, &team, &body.mention);
    let receipt = crate::team::post(state.clone(), owners_post(&team, Some(&project), text, body.attachments, mention))
        .await
        .map_err(|e| to_error_response(types::NeboError::Internal(e)))?;
    Ok(Json(serde_json::json!({
        "message": "Sent",
        "messageId": receipt.message_id,
        "asked": receipt.asked,
    })))
}

/// PUT /projects/{id}/read — the owner has the project's thread open.
pub async fn mark_project_read(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let (project, _) = load_project(&state, &id)?;
    crate::handlers::chat::conversation_read(&state, &project.thread_session_key).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /projects/{id}/working — what runs in the project now, as
/// `GET /teams/{teamId}/working` reads it for the team's own thread.
pub async fn project_working(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let (project, team) = load_project(&state, &id)?;
    Ok(Json(serde_json::json!({ "working": working(&state, &team, Some(&project)).await })))
}

#[derive(Debug, Deserialize)]
pub struct ContinueProjectRequest {
    /// "continue" (the default) or "stop".
    #[serde(default)]
    pub answer: Option<String>,
}

/// POST /projects/{id}/continue — the owner's answer to a project paused
/// at its limit, from the project itself: the same answer as its Inbox
/// item's (`team::answer_limit`). `answered` is false when it was not
/// paused.
pub async fn continue_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ContinueProjectRequest>,
) -> HandlerResult<serde_json::Value> {
    let (project, _) = load_project(&state, &id)?;
    let answer = body.answer.as_deref().unwrap_or("continue");
    let answered = crate::team::answer_limit(&state, &project, answer, "project")
        .await
        .map_err(|e| to_error_response(types::NeboError::Validation(e)))?;
    Ok(Json(serde_json::json!({ "answered": answered })))
}

/// POST /projects/{id}/stop — the project's Stop: every member's work in
/// it, one member's (`agentId`), or one helper (`agentId` + `taskId`).
/// Never anything outside the project.
pub async fn stop_project_work(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<StopTeamWorkRequest>,
) -> HandlerResult<serde_json::Value> {
    let (project, team) = load_project(&state, &id)?;
    let target = match (body.agent_id.as_str(), body.task_id.as_str()) {
        ("", _) => Target::Everyone,
        (member, "") => Target::Member(member),
        (member, task_id) => Target::Helper { member, task_id },
    };
    let stopped = stop(&state, &team, Some(&project), StopBy::Owner, target)
        .await
        .map_err(|e| to_error_response(types::NeboError::Internal(e)))?;
    Ok(Json(serde_json::json!({
        "message": stopped_line(&stopped),
        "stopped": stopped,
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
