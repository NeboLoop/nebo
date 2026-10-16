//! Teams — the server leg of a team post. A team is a LOCAL object
//! (`db::Team`, thread `team:<id>`); posting into one:
//!
//! 1. appends the post to the team's local thread and broadcasts it — the
//!    ONE record of the team's conversation,
//! 2. asks the members who act on it through the ONE coworker rail
//!    (`coworker::send_coworker_message` with a team leg): each runs in its
//!    seat for the team, briefed with the conversation read from the team
//!    thread, and its reply comes back here as a post. A member not asked is
//!    sent nothing — no copy of the post lands in any of its threads, so its
//!    own chat stays its conversation with the owner,
//! 3. forwards it to the team's hub channel when the team is mirrored —
//!    best effort, never on the critical path.
//!
//! Who a post asks to act is `addressing::who_is_addressed` — the ONE
//! routing model for team posts and messages between employees: the
//! owner's plain post goes to the lead, @Name to that member, @everyone to
//! every member, and the lead, answering, hands steps off by name. Nothing
//! else asks anyone: a member's reply is its answer, and it goes to whoever
//! asked (and into the thread), never back as a new question. Each member
//! answers each post once (`addressing::turn_ended`).

use std::future::Future;
use std::pin::Pin;

use tools::coworker::{Authority, CoworkerMessage, TeamDelivery, TeamPost, TeamPostReceipt};

use crate::addressing::{self, NoLead, Room, Said};
use crate::state::AppState;
use types::provenance::ProvenanceClass;

/// Who wrote a team row, for `record`. Everything that happens on this Nebo
/// is `Local`: the owner (empty id) or one of this Nebo's employees, whose
/// name is looked up here. A post that arrived through the team's hub mirror
/// is `Hub`: its author lives on another Nebo, so only the hub message knows
/// the name and whether it was a person or an employee.
pub(crate) enum TeamSender<'a> {
    Local(&'a str),
    Hub { agent_id: &'a str, name: &'a str, from: crate::HubSender },
}

/// Post into a team. Boxed because a member's reply is itself a post (the
/// rail's collector calls back here).
pub(crate) fn post(
    state: AppState,
    post: TeamPost,
) -> Pin<Box<dyn Future<Output = Result<TeamPostReceipt, String>> + Send>> {
    Box::pin(async move {
        let team = state
            .store
            .get_team(&post.team_id)
            .map_err(|e| format!("load team: {e}"))?
            .ok_or_else(|| format!("No team with id {}", post.team_id))?;
        // A post into one of the team's projects is recorded in that
        // project's thread, and the members it asks work it there.
        let project = match post.project_id.as_deref() {
            Some(id) => Some(
                state
                    .store
                    .get_project(id)
                    .map_err(|e| format!("load project: {e}"))?
                    .filter(|p| p.team_id == team.id)
                    .ok_or_else(|| format!("No project {id} in team {}", team.name))?,
            ),
            None => None,
        };
        let text = post.text.trim();
        if text.is_empty() {
            return Err("text required".to_string());
        }
        // The owner writing in a done project reopens it: the lead takes it
        // up again in the same thread, with all of its context.
        let project = match project {
            Some(p) if p.state == "done" && post.by_owner => {
                let reopened = state
                    .store
                    .set_project_state(&p.id, "live", chrono::Utc::now().timestamp())
                    .map_err(|e| format!("reopen project: {e}"))?
                    .unwrap_or(p);
                crate::handlers::teams::announce_project(&state, &reopened);
                Some(reopened)
            }
            other => other,
        };
        let roster = tools::team::member_roster(&state.store, &team);
        let mut text = tools::team::normalize_mentions(text, &roster);
        // Attachments: the ONE helper a direct chat uses — files are saved
        // locally and noted in the text ("[Attached: … saved at …]"), so the
        // record, the hub mirror, and every member's delivery all carry them.
        // ponytail: the rail delivers text only, so images reach members as
        // saved paths (they can open them), not as vision content yet.
        if !post.attachments.is_empty() {
            let _images = crate::process_comm_attachments(&state, &post.attachments, &mut text).await;
        }
        let attachments_json = serde_json::to_value(&post.attachments).unwrap_or_default();

        // Who acts is decided once, before anything is recorded or sent: a
        // post nobody would take is refused whole, and the hub mirror has to
        // carry the asks for members on other machines. How the post is said
        // — deliberately, or in the course of answering what addressed its
        // author — is read from the conversation it was posted from.
        let member_ids = tools::team::member_ids(&team);
        let mut named = post.mention.clone();
        for id in tools::team::mentioned_members(&text, &member_ids) {
            if !named.contains(&id) {
                named.push(id);
            }
        }
        let said = if post.from_agent_id.is_empty() {
            Said::Deliberate
        } else {
            addressing::said_in(&state, post.reply_to.as_deref().unwrap_or_default(), &team.id)
        };
        let act = addressing::who_is_addressed(&addressing::Post {
            author: &post.from_agent_id,
            said: &said,
            room: Some(Room { lead: tools::team::lead_of(&team), members: &member_ids }),
            named: &named,
            everyone: tools::team::mentions_everyone(&text),
        })
        .map_err(|NoLead| {
            let names: Vec<String> = roster.iter().map(|(_, name)| format!("@{name}")).collect();
            format!(
                "Team \"{}\" has no lead, so a post that names nobody has nobody to take it; it was \
                 NOT sent. Name who should act ({}), write @everyone to ask the whole team, or tell \
                 the owner the team needs a lead (update_team with lead).",
                team.name,
                names.join(", ")
            )
        })?;

        // 1. The record: the team's own thread.
        let (message, sender_name) = record(
            &state,
            &team,
            project.as_ref(),
            TeamSender::Local(&post.from_agent_id),
            &text,
            &attachments_json,
            &post.provenance,
            None,
        )?;

        // A member on another computer is reached the only way it can be:
        // through the team's hub channel, addressed by name so that machine's
        // own dispatch runs it. There is no second rail — the far Nebo treats
        // this exactly like any other channel message that names one of its
        // employees.
        let remote_asks: Vec<&db::TeamMember> = team
            .members
            .iter()
            .filter(|m| !m.is_local() && act.contains(&m.agent_id))
            .collect();

        // 3. The optional hub mirror (before fan-out so a slow hub never
        // delays the members; best effort either way). The hub channel
        // mirrors the team's own thread; a project's posts stay on this
        // computer, so a member on another one is not asked in a project.
        let mirrored = match project {
            None => mirror_to_hub(&state, &team, &text, &sender_name, &post, &remote_asks).await,
            Some(_) => false,
        };

        // Whose request the post is: the owner's own when he typed it; one
        // of his passed on when the member posting (its reply, or a post
        // through the tool) is working on his post; its own otherwise.
        let authority = if post.by_owner {
            Authority::Owner { request: message.id.clone() }
        } else {
            crate::coworker::seat_authority(
                &state,
                post.reply_to.as_deref().unwrap_or_default(),
                &post.from_agent_id,
                post.owners_turn.as_deref(),
            )
        };

        // A member on another computer was asked only when the post went
        // out to it: with no hub, or no channel, nobody is told it was.
        let mut asked: Vec<String> = Vec::new();
        if mirrored {
            for m in &remote_asks {
                let name = roster
                    .iter()
                    .find(|(id, _)| *id == m.agent_id)
                    .map(|(_, name)| name.clone())
                    .unwrap_or_else(|| m.agent_id.clone());
                asked.push(name);
            }
        }

        // 2. The asks over the local rail. A remote member is not on it — it
        // was asked through the hub above. A member not asked is sent
        // nothing: the post is in the team thread, which is where its
        // briefing is read from the next time it is asked.
        for (member_id, member_name) in &roster {
            if *member_id == post.from_agent_id || !act.contains(member_id) {
                continue;
            }
            if team
                .members
                .iter()
                .any(|m| m.agent_id == *member_id && !m.is_local())
            {
                continue;
            }
            let msg = CoworkerMessage {
                from_agent_id: post.from_agent_id.clone(),
                sender_session_key: project
                    .as_ref()
                    .map(|p| p.thread_session_key.clone())
                    .unwrap_or_else(|| db::team_thread_key(&team.id)),
                to: member_id.clone(),
                text: text.clone(),
                requester_scope: String::new(),
                handoff_depth: post.handoff_depth,
                provenance: post.provenance.clone(),
                team: Some(TeamDelivery {
                    team_id: team.id.clone(),
                    project_id: project.as_ref().map(|p| p.id.clone()),
                    post_id: message.id.clone(),
                    reply_to: post.reply_to.clone(),
                    authority: authority.clone(),
                }),
                conversation: None,
                // The delivery carries the post's authority, decided above.
                owners_turn: None,
                sender_run_id: None,
                // Every member asked runs no looser than the post's run.
                narrow: post.narrow,
            };
            match crate::coworker::send_coworker_message(state.clone(), msg).await {
                Ok(_) => asked.push(member_name.clone()),
                Err(e) => tracing::warn!(
                    error = %e,
                    team = %team.id,
                    member = %member_id,
                    "team: delivery to member failed"
                ),
            }
        }

        Ok(TeamPostReceipt {
            team_id: team.id,
            team_name: team.name,
            message_id: message.id,
            asked,
        })
    })
}

/// The record of one post — step 1 of `post` on its own: append the row to
/// the team's thread, or to one of its projects' threads (`project`), and
/// broadcast it (`team_message` for the team's own thread, `project_message`
/// for a project's, so an app that knows only team threads never shows a
/// project's row in the team chat), so every open view (desktop and mobile)
/// shows it live. The ONE writer of a team row and of a project row. `post`
/// calls it before the fan-out; a turn spoken in the team thread's voice
/// mode calls it alone, because the lead already answered out loud and a
/// fan-out would ask it the same thing again in text; the hub-mirror feed
/// calls it for every message the mirrored channel delivers; a member that
/// takes a post it was asked to act on acknowledges through it
/// (`coworker::OwnerForward::acknowledge`). Returns the
/// row and the sender's display name ("Owner" for the owner of this Nebo).
/// `provenance` is the posting run's: it rides the row as metadata, for the
/// members briefed with the post and the gates, never in the words. `card`
/// is set on the lead's line that hands a project's card out
/// (`card_lines`): the row and its event carry it, so the app shows the
/// line as the card.
pub(crate) fn record(
    state: &AppState,
    team: &db::Team,
    project: Option<&db::Project>,
    sender: TeamSender<'_>,
    text: &str,
    attachments: &serde_json::Value,
    provenance: &[types::provenance::ProvenanceClass],
    card: Option<&db::TeamCard>,
) -> Result<(db::TeamMessage, String), String> {
    let untrusted = matches!(sender, TeamSender::Hub { from, .. } if from != crate::HubSender::Owner);
    let (from_agent_id, sender_name, role) = match sender {
        TeamSender::Local("") => ("", "Owner".to_string(), "user"),
        TeamSender::Local(id) => (
            id,
            state
                .store
                .get_agent(id)
                .ok()
                .flatten()
                .map(|a| a.name)
                .unwrap_or_else(|| id.to_string()),
            "assistant",
        ),
        TeamSender::Hub { agent_id, name, from } => {
            let (name, role) = hub_row_sender(name, from);
            (agent_id, name, role)
        }
    };
    let mut provenance = provenance.to_vec();
    if untrusted && !provenance.contains(&ProvenanceClass::Channel) {
        provenance.push(ProvenanceClass::Channel);
    }
    let message = state
        .store
        .append_team_message(team, project, role, text, &sender_name, from_agent_id, attachments, &provenance, card)
        .map_err(|e| format!("record team post: {e}"))?;
    let thread = project.map(|p| p.thread_session_key.clone()).unwrap_or_else(|| db::team_thread_key(&team.id));
    let mut payload = serde_json::json!({
        "teamId": team.id,
        "messageId": message.id,
        "from": sender_name,
        "fromAgentId": from_agent_id,
        "senderName": sender_name,
        "role": role,
        "text": text,
        "attachments": attachments,
        // A member's post the owner has not read yet: the sidebar's dot
        // (an app with the thread open marks it read).
        "unread": state.store.conversation_unread(&thread).unwrap_or(false),
    });
    if let Some(c) = card {
        payload["card"] = serde_json::json!(c);
    }
    let event = match project {
        Some(p) => {
            payload["projectId"] = serde_json::json!(p.id);
            tools::team::PROJECT_MESSAGE_EVENT
        }
        None => tools::team::TEAM_MESSAGE_EVENT,
    };
    state.hub.broadcast(event, payload);
    Ok((message, sender_name))
}

/// Longest wrap-up the done notice carries; the project's thread has it whole.
const WRAP_UP_NOTICE_CHARS: usize = 1_500;

/// A done project's wrap-up, the lead's words when its run's outcome came
/// back (`engine::finish_temporary_work` closed the project), delivered
/// once per close: to the owner's Inbox, loud, linking to the project's
/// thread, and to his phone.
pub(crate) fn wrap_up(state: &AppState, project: &db::Project, seat: &str, heard_through: Option<&str>, words: &str) {
    let words = words.trim();
    if words.is_empty() || project.state != "done" || !heard_the_outcome(state, project, seat, heard_through) {
        return;
    }
    let id = format!("project:{}:{}", project.id, project.closed_at.unwrap_or_default());
    let user = state.store.ensure_local_user_id().unwrap_or_default();
    if matches!(state.store.get_notification(&id, &user), Ok(Some(_))) {
        return;
    }
    let title = format!("{} is done", project.title);
    let link = tools::owner_notify::link::project(&project.team_id, &project.id);
    let body: String = words.chars().take(WRAP_UP_NOTICE_CHARS).collect();
    let n = tools::owner_notify::OwnerNotification {
        id: &id,
        kind: "info",
        title: &title,
        body: Some(&body),
        action_url: Some(&link),
        agent_id: Some(&project.lead_agent_id),
        loud: true,
        link_label: Some("Project"),
        ..Default::default()
    };
    if !tools::owner_notify::emit(&state.store, Some(&|ev, payload| state.hub.broadcast(ev, payload)), &n) {
        tracing::warn!(project = %project.id, "project: the done notice was not stored");
        return;
    }
    crate::codes::push_inbox(state, n.hub_item(serde_json::json!({})));
}

/// Whether the lead's turn in `seat` heard its project's latest run end:
/// the outcome row (`engine::report_temporary_outcome`, naming the run) is
/// in what the turn read, up to `heard_through`. Its answer is the wrap-up;
/// any other answer in a done project (its plan, landing after a run that
/// was quicker than it) is not.
fn heard_the_outcome(state: &AppState, project: &db::Project, seat: &str, heard_through: Option<&str>) -> bool {
    let Some(run) = state.store.latest_project_run(&project.id).ok().flatten() else {
        return false;
    };
    let marker = format!("(run {run})");
    let sessions = state.harness.sessions();
    let Ok(sid) = sessions.resolve_session_id_by_key(seat) else {
        return false;
    };
    let rows = sessions.get_messages(&sid).unwrap_or_default();
    let read = match heard_through.and_then(|id| rows.iter().position(|m| m.id == id)) {
        Some(at) => &rows[..=at],
        None => &rows[..],
    };
    read.iter().any(|m| m.content.contains(&marker))
}


/// A project's cards as lines in its thread: each change a card goes
/// through (`workflow::cases::ProjectChange`, rung by the engine when a
/// project's step is handed to a member and when the member closes it)
/// becomes one row, written by `record` like every row, and the card's
/// hand-off goes to every open view. Handing out is the lead's line ("Script
/// Writer: hero script. Done when: …"); the close is the member's own
/// summary, or that it could not finish and why.
pub(crate) fn project_lines(state: AppState, mut rx: tokio::sync::mpsc::UnboundedReceiver<workflow::cases::ProjectChange>) {
    tokio::spawn(async move {
        while let Some(change) = rx.recv().await {
            let said = match &change {
                workflow::cases::ProjectChange::LimitReached { project_id, run_id } => paused_at_limit(&state, project_id, run_id),
                workflow::cases::ProjectChange::HeldForOk { run_id, key, assignment_id, summary } => {
                    held_for_ok(&state, run_id, key, assignment_id.as_deref(), summary)
                }
                workflow::cases::ProjectChange::Started { project_id, .. } => playbook_started(&state, project_id),
                _ => project_line(&state, &change),
            };
            if let Err(e) = said {
                tracing::warn!(error = %e, ?change, "project: line not recorded");
            }
        }
    });
}

fn project_line(state: &AppState, change: &workflow::cases::ProjectChange) -> Result<(), String> {
    use workflow::cases::ProjectChange;
    let id = match change {
        ProjectChange::Opened { assignment_id, .. } | ProjectChange::Closed { assignment_id } => assignment_id,
        ProjectChange::LimitReached { .. } | ProjectChange::HeldForOk { .. } | ProjectChange::Started { .. } => return Ok(()),
    };
    let Some(card) = state.store.get_handoff(id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    let Some(project) = card.project_id.as_deref().map(|p| state.store.get_project(p)).transpose().map_err(|e| e.to_string())?.flatten()
    else {
        return Ok(());
    };
    let Some(team) = state.store.get_team(&project.team_id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    crate::handoff::broadcast(state, card.clone());
    let member = state.store.get_agent(&card.to_agent_id).ok().flatten().map(|a| a.name).unwrap_or_else(|| card.to_agent_id.clone());
    let (from, text, handed_out) = match change {
        ProjectChange::Opened { task, done_when, .. } => (
            card.from_agent_id.as_str(),
            match done_when.trim() {
                "" => format!("{member}: {task}"),
                when => format!("{member}: {task}\nDone when: {when}"),
            },
            Some(db::TeamCard {
                handoff_id: card.id.clone(),
                agent_id: card.to_agent_id.clone(),
                task: task.clone(),
                done_when: done_when.trim().to_string(),
            }),
        ),
        ProjectChange::LimitReached { .. } | ProjectChange::HeldForOk { .. } | ProjectChange::Started { .. } => return Ok(()),
        ProjectChange::Closed { .. } => (
            card.to_agent_id.as_str(),
            match card.status.as_str() {
                "done" if !card.result.trim().is_empty() => card.result.clone(),
                "done" => "Done.".to_string(),
                _ => {
                    let why = [card.error.trim(), card.result.trim()].into_iter().find(|s| !s.is_empty()).unwrap_or("no reason was given");
                    format!("I couldn't finish this: {why}")
                }
            },
            None,
        ),
    };
    record(state, &team, Some(&project), TeamSender::Local(from), &text, &serde_json::json!([]), &[], handed_out.as_ref()).map(|_| ())
}

/// A playbook's run opened a fresh project (`workflow_manager::
/// open_playbook_project`): every open view hears of it, its lead's seat for
/// it is ready to answer there (its route: a colleague's request into the
/// project, so the wrap-up turn holds only the lead's tools and its words
/// are posted to the project's thread), and the thread opens with what the
/// playbook is for, in the lead's name.
fn playbook_started(state: &AppState, project_id: &str) -> Result<(), String> {
    let Some(project) = state.store.get_project(project_id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    let Some(team) = state.store.get_team(&project.team_id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    crate::handlers::teams::announce_project(state, &project);
    let lead_name = state.store.get_agent(&project.lead_agent_id).ok().flatten().map(|a| a.name).unwrap_or_default();
    let (seat, title) = crate::coworker::team_seat(&project.lead_agent_id, &team, Some(&project));
    crate::coworker::ensure_conversation_thread(state, &seat, &title)?;
    let route = crate::reply_route::CoworkerRoute {
        to_agent_id: project.lead_agent_id.clone(),
        to_name: lead_name,
        from_agent_id: String::new(),
        from_name: "the playbook's schedule".to_string(),
        mirror_key: None,
        sender_depth: 0,
        team: Some(crate::reply_route::TeamLeg { team_id: team.id.clone(), team_name: team.name.clone(), project_id: Some(project.id.clone()) }),
        authority: tools::coworker::Authority::Coworker,
        narrow: None,
    };
    crate::reply_route::set(state, &seat, "", Some(&crate::reply_route::ReplyRoute::Coworker(route)));
    let line = format!("Started: {}", project.ask);
    record(state, &team, Some(&project), TeamSender::Local(&project.lead_agent_id), &line, &serde_json::json!([]), &[], None).map(|_| ())
}

/// The answers a held result takes. Any other answer is a Change with that
/// text as what to change (an app that lets the owner type it sends it).
const HELD_APPROVE: &str = "approve";
const HELD_CHANGE: &str = "change";

/// A member's result held for the owner's OK (`params.hold` on its card,
/// `workflow::graph::run_expert`): the project's thread shows it held, and
/// the owner is asked in Needs you and on his phone to Approve it or have it
/// changed. The item is `held:<request key>`; the run is the key's first
/// segment.
fn held_for_ok(state: &AppState, run_id: &str, key: &str, assignment_id: Option<&str>, summary: &str) -> Result<(), String> {
    let project = state.store.workflow_run_project(run_id).map_err(|e| e.to_string())?.and_then(|p| state.store.get_project(&p).ok().flatten());
    let card = assignment_id.and_then(|a| state.store.get_handoff(a).ok().flatten());
    let member = card
        .as_ref()
        .and_then(|c| state.store.get_agent(&c.to_agent_id).ok().flatten())
        .map(|a| a.name)
        .unwrap_or_else(|| "A teammate".to_string());
    let task = card.as_ref().map(|c| c.ask.clone()).unwrap_or_default();
    if let Some(p) = project.as_ref()
        && let Some(team) = state.store.get_team(&p.team_id).map_err(|e| e.to_string())?
    {
        let from = card.as_ref().map(|c| c.to_agent_id.clone()).unwrap_or_else(|| p.lead_agent_id.clone());
        let line = format!("Held for your OK: {summary}");
        record(state, &team, Some(p), TeamSender::Local(&from), &line, &serde_json::json!([]), &[], None)?;
    }
    let id = format!("held:{key}");
    let title = match task.trim() {
        "" => format!("{member}'s result is held for your OK"),
        task => format!("{member}: {task}"),
    };
    let link = match project.as_ref() {
        Some(p) => tools::owner_notify::link::project(&p.team_id, &p.id),
        None => String::new(),
    };
    let options = [
        db::models::ItemOption { label: "Approve".into(), value: HELD_APPROVE.into() },
        db::models::ItemOption { label: "Change".into(), value: HELD_CHANGE.into() },
    ];
    let lead = project.as_ref().map(|p| p.lead_agent_id.clone());
    let n = tools::owner_notify::OwnerNotification {
        id: &id,
        kind: "info",
        title: &title,
        body: Some(summary),
        action_url: (!link.is_empty()).then_some(link.as_str()),
        agent_id: lead.as_deref(),
        loud: true,
        needs_you: true,
        options: &options,
        link_label: project.as_ref().map(|_| "Project"),
        ..Default::default()
    };
    if tools::owner_notify::emit(&state.store, Some(&|ev, payload| state.hub.broadcast(ev, payload)), &n) {
        crate::codes::push_inbox(state, n.hub_item(serde_json::json!({})));
    }
    Ok(())
}

/// The owner's review of a held result (`held:<request key>` in the Inbox):
/// the ONE way it is answered. Approve makes it the step's output and the
/// run goes on; Change (or any other words, which say what to change) sends
/// it back to the same member as the next round, held again when it
/// returns. The item is settled and the project's thread says which.
pub(crate) fn answer_held(state: &AppState, key: &str, answer: &str, via: &str) -> Result<(), String> {
    let run_id = key.split(':').next().unwrap_or_default();
    let review = match answer.trim() {
        HELD_APPROVE => workflow::expert::Review::Approve,
        HELD_CHANGE => workflow::expert::Review::Change(String::new()),
        note => workflow::expert::Review::Change(note.to_string()),
    };
    workflow::expert::record_review(&state.store, run_id, key, &review).map_err(|e| e.to_string())?;
    let label = match &review {
        workflow::expert::Review::Approve => "Approved",
        workflow::expert::Review::Change(_) => "Change",
    };
    tools::owner_notify::settle(&state.store, Some(&|ev, p| state.hub.broadcast(ev, p)), &format!("held:{key}"), label, Some(via));
    let project = state.store.workflow_run_project(run_id).map_err(|e| e.to_string())?.and_then(|p| state.store.get_project(&p).ok().flatten());
    if let Some(p) = project
        && let Some(team) = state.store.get_team(&p.team_id).map_err(|e| e.to_string())?
    {
        let line = match &review {
            workflow::expert::Review::Approve => "Approved.".to_string(),
            workflow::expert::Review::Change(note) if note.is_empty() => "Change it, please.".to_string(),
            workflow::expert::Review::Change(note) => format!("Change it, please: {note}"),
        };
        record(state, &team, Some(&p), TeamSender::Local(""), &line, &serde_json::json!([]), &[], None)?;
    }
    Ok(())
}

/// The answers a project paused at its limit takes.
const LIMIT_CONTINUE: &str = "continue";
const LIMIT_STOP: &str = "stop";

/// The Inbox item that asks the owner about a project's run paused at its
/// limit (`limit:<project>:<run>`), and the project and run it names.
fn limit_item_id(project_id: &str, run_id: &str) -> String {
    format!("limit:{project_id}:{run_id}")
}

/// A project's run paused at its limit (`workflow::graph::park_at_limit`):
/// the project's thread says so, and the owner is asked, in Needs you and on
/// his phone, whether it goes on (Continue) or ends (Stop). Nothing more is
/// handed out until he answers; work already handed out finishes.
fn paused_at_limit(state: &AppState, project_id: &str, run_id: &str) -> Result<(), String> {
    let Some(project) = state.store.get_project(project_id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    let Some(team) = state.store.get_team(&project.team_id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    crate::handlers::teams::announce_project(state, &project);
    let (steps, cents) = project.limits();
    let line = format!("Paused: this project reached its limit of {steps} steps / ${:.2}. Continue or Stop?", cents as f64 / 100.0);
    record(state, &team, Some(&project), TeamSender::Local(&project.lead_agent_id), &line, &serde_json::json!([]), &[], None)?;
    let id = limit_item_id(&project.id, run_id);
    let title = format!("{} is paused at its limit", project.title);
    let link = tools::owner_notify::link::project(&project.team_id, &project.id);
    let options = [
        db::models::ItemOption { label: "Continue".into(), value: LIMIT_CONTINUE.into() },
        db::models::ItemOption { label: "Stop".into(), value: LIMIT_STOP.into() },
    ];
    let n = tools::owner_notify::OwnerNotification {
        id: &id,
        kind: "info",
        title: &title,
        body: Some(&line),
        action_url: Some(&link),
        agent_id: Some(&project.lead_agent_id),
        loud: true,
        needs_you: true,
        options: &options,
        link_label: Some("Project"),
        ..Default::default()
    };
    if tools::owner_notify::emit(&state.store, Some(&|ev, payload| state.hub.broadcast(ev, payload)), &n) {
        crate::codes::push_inbox(state, n.hub_item(serde_json::json!({})));
    }
    Ok(())
}

/// The owner's answer to a project paused at its limit, from the Inbox
/// (`limit:` items) or the project's own Continue (`POST
/// /projects/{id}/continue`): the ONE way it is answered. Continue gives the
/// project as much room again (its limit raised by the default allowance
/// past what it used) and wakes its run, which hands out its next card;
/// Stop ends the run, and the project closes with its lead's wrap-up.
/// `false` when the project is not paused.
pub(crate) async fn answer_limit(state: &AppState, project: &db::Project, answer: &str, via: &str) -> Result<bool, String> {
    if project.state != "paused" {
        return Ok(false);
    }
    let Some(run_id) = state.store.latest_project_run(&project.id).map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    let label = match answer {
        LIMIT_CONTINUE => "Continue",
        LIMIT_STOP => "Stop",
        other => return Err(format!("a paused project takes continue or stop, not {other:?}")),
    };
    tools::owner_notify::settle(&state.store, Some(&|ev, p| state.hub.broadcast(ev, p)), &limit_item_id(&project.id, &run_id), label, Some(via));
    let now = chrono::Utc::now().timestamp();
    if answer == LIMIT_STOP {
        state.workflow_manager.cancel_run(&run_id).await?;
        return Ok(true);
    }
    let used = state.store.project_usage(project).map_err(|e| e.to_string())?;
    let steps = used.turns + db::PROJECT_LIMIT_STEPS;
    let cents = used.spent_microcents / 1_000_000 + db::PROJECT_LIMIT_SPEND_CENTS;
    state.store.raise_project_limits(&project.id, steps, cents).map_err(|e| e.to_string())?;
    if let Some(live) = state.store.set_project_state(&project.id, "live", now).map_err(|e| e.to_string())? {
        crate::handlers::teams::announce_project(state, &live);
    }
    state
        .store
        .engine_enqueue_event(&db::NewEvent {
            kind: workflow::expert::LIMIT_KIND,
            target_type: "run",
            target_id: &workflow::expert::limit_target(&run_id),
            payload: "continue",
            idem_key: &format!("limit-continue:{run_id}:{steps}"),
            durable: true,
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// A post that came through the hub as the team's record shows it: its
/// name and its row's role. Who it is from is decided once, by
/// `crate::hub_sender` (the seam on trusting the hub). Only the owner's own
/// post reads as the owner's; anything else is channel content (`record`
/// marks it, so a member briefed with it reads it as such and its run
/// carries the taint) and never reads as the owner — one naming itself
/// "Owner", or nobody, is "Someone on another computer".
fn hub_row_sender(name: &str, from: crate::HubSender) -> (String, &'static str) {
    if from == crate::HubSender::Owner {
        return (crate::coworker::OWNER.to_string(), "user");
    }
    let name = name.trim();
    let name = if name.is_empty() || name.eq_ignore_ascii_case(crate::coworker::OWNER) {
        "Someone on another computer".to_string()
    } else {
        name.to_string()
    };
    (name, if from == crate::HubSender::Agent { "assistant" } else { "user" })
}

/// Forward a post to the team's hub channel, if the team is mirrored and
/// the hub is connected. Local mention tokens become hub tokens for
/// employees the hub knows. Returns whether it went out; a failure is
/// logged.
async fn mirror_to_hub(
    state: &AppState,
    team: &db::Team,
    text: &str,
    sender_name: &str,
    post: &TeamPost,
    remote_asks: &[&db::TeamMember],
) -> bool {
    let Some(channel_id) = team.hub_channel_id.as_deref().filter(|c| !c.is_empty()) else {
        return false;
    };
    let Some(plugin) = state.comm_manager.active_plugin().await else {
        return false;
    };
    if !plugin.is_connected() {
        return false;
    }
    let mut content = text.to_string();
    if content.contains("<@") {
        // A LOCAL member is addressed locally in the team thread, so its token
        // has to be translated into the identity the hub knows. A remote
        // member's id is already a hub id and passes through untouched.
        for m in team.members.iter().filter(|m| m.is_local()) {
            let token = format!("<@{}>", m.agent_id);
            if !content.contains(&token) {
                continue;
            }
            if let Ok(Some(a)) = state.store.get_agent(&m.agent_id) {
                if let Some(loop_id) = a.loop_agent_id.as_deref().filter(|s| !s.is_empty()) {
                    content = content.replace(&token, &format!("<@{loop_id}>"));
                }
            }
        }
    }
    // The asks for members on other machines. The far Nebo answers a channel
    // message that names one of its employees, so naming them here IS the
    // dispatch — appended rather than woven in, so the owner's own words reach
    // that machine unchanged.
    for m in remote_asks {
        let token = format!("<@{}>", m.agent_id);
        if !content.contains(&token) {
            content.push_str(&format!(" {token}"));
        }
    }
    let hub_id = crate::chat_dispatch::hub_agent_id(&state.store, &post.from_agent_id);
    let metadata = mirror_metadata(post, sender_name, hub_id);
    let msg = comm::CommMessage {
        id: uuid::Uuid::new_v4().to_string(),
        from: String::new(),
        to: String::new(),
        topic: channel_id.to_string(),
        conversation_id: channel_id.to_string(),
        msg_type: comm::CommMessageType::LoopChannel,
        content,
        metadata,
        timestamp: 0,
        human_injected: post.by_owner,
        human_id: None,
        task_id: None,
        correlation_id: None,
        task_status: None,
        artifacts: Vec::new(),
        error: None,
        attachments: Vec::new(),
    };
    match state.comm_manager.send(msg).await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(error = %e, team = %team.id, "team: hub mirror send failed (team stays local)");
            false
        }
    }
}

/// What a team post carries to the hub about who wrote it. The owner's own
/// post (`by_owner`, set only by the team door he typed it in) carries the
/// owner-relay mark; anything else — an employee's post, the main employee's
/// — is the bot's (`senderKind: agent`), whatever its words say.
fn mirror_metadata(post: &TeamPost, sender_name: &str, hub_id: Option<String>) -> std::collections::HashMap<String, String> {
    if post.by_owner {
        return crate::chat_dispatch::owner_relay_metadata();
    }
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("senderName".to_string(), sender_name.to_string());
    metadata.insert("senderKind".to_string(), "agent".to_string());
    // No fromAgentId: this bot's own deliveries without one are its echo
    // (`handle_comm_message`), so the post is not recorded twice.
    if !post.from_agent_id.is_empty() {
        metadata.insert("fromAgentName".to_string(), sender_name.to_string());
    }
    if let Some(hub_id) = hub_id {
        metadata.insert("agentId".to_string(), hub_id);
    }
    if post.handoff_depth > 0 {
        metadata.insert("handoffDepth".to_string(), post.handoff_depth.to_string());
    }
    metadata
}

/// The owner's Stop for a team's members on other computers: one stop to
/// the team's hub channel, which each of those Nebos takes as a stop of
/// every employee's work in that channel (`try_handle_channel_control`).
/// Returns whether it went out.
pub(crate) async fn stop_on_hub(state: &AppState, team: &db::Team) -> bool {
    let Some(channel_id) = team.hub_channel_id.as_deref().filter(|c| !c.is_empty()) else {
        return false;
    };
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("kind".to_string(), "stop".to_string());
    metadata.insert("senderKind".to_string(), "agent".to_string());
    let msg = comm::CommMessage {
        id: uuid::Uuid::new_v4().to_string(),
        from: String::new(),
        to: String::new(),
        topic: channel_id.to_string(),
        conversation_id: channel_id.to_string(),
        msg_type: comm::CommMessageType::LoopChannel,
        content: "/stop".to_string(),
        metadata,
        timestamp: 0,
        human_injected: false,
        human_id: None,
        task_id: None,
        correlation_id: None,
        task_status: None,
        artifacts: Vec::new(),
        error: None,
        attachments: Vec::new(),
    };
    match state.comm_manager.send(msg).await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(error = %e, team = %team.id, "team: the stop did not reach the hub");
            false
        }
    }
}

#[cfg(test)]
mod hub_sender_tests {
    use crate::HubSender;

    fn post(from: &str, by_owner: bool) -> tools::coworker::TeamPost {
        tools::coworker::TeamPost {
            attachments: vec![],
            team_id: "t1".into(),
            project_id: None,
            from_agent_id: from.into(),
            by_owner,
            owners_turn: None,
            text: "hi".into(),
            mention: vec![],
            handoff_depth: 1,
            provenance: vec![],
            narrow: None,
            reply_to: None,
        }
    }

    /// Only the owner's own team post carries the owner-relay mark to the
    /// hub; an employee's post, or the main employee's, never carries it or
    /// role=user.
    #[test]
    fn only_the_owners_post_is_relayed_as_his() {
        let owners = super::mirror_metadata(&post("", true), "Owner", None);
        assert_eq!(owners.get("relayedForOwner").map(String::as_str), Some("true"));
        for (from, name) in [("emp-1", "Pam"), ("", "Nebo")] {
            let m = super::mirror_metadata(&post(from, false), name, Some("hub-pam".into()));
            assert!(m.get("relayedForOwner").is_none() && m.get("role").is_none(), "{m:?}");
            assert_eq!(m.get("senderKind").map(String::as_str), Some("agent"));
        }
    }

    /// A hub row reads as the owner only when the seam says it is his;
    /// otherwise never, whatever it calls itself.
    #[test]
    fn a_hub_row_never_reads_as_the_owner_unless_it_is_his() {
        assert_eq!(super::hub_row_sender("Owner", HubSender::Person), ("Someone on another computer".to_string(), "user"));
        assert_eq!(super::hub_row_sender(" owner ", HubSender::Agent).0, "Someone on another computer");
        assert_eq!(super::hub_row_sender("", HubSender::Agent).0, "Someone on another computer");
        assert_eq!(super::hub_row_sender("Scout", HubSender::Agent), ("Scout".to_string(), "assistant"));
        assert_eq!(super::hub_row_sender("Alma", HubSender::Owner), ("Owner".to_string(), "user"));
    }
}

#[cfg(test)]
mod tests {
    /// The lead is ONE rule for text, voice and the roster: the member on
    /// record, if it is on the team; nobody when there is no lead or the
    /// lead left the team.
    #[test]
    fn the_lead_is_one_rule() {
        let team = |lead: &str| db::Team {
            id: "t".into(),
            name: "T".into(),
            mission: String::new(),
            members: ["m1", "m2", "m3"].into_iter().map(db::TeamMember::local).collect(),
            organizer_agent_id: lead.into(),
            hub_channel_id: None,
            created_at: 0,
            standing: String::new(),
        };
        assert_eq!(tools::team::lead_of(&team("m2")), Some("m2"));
        assert_eq!(tools::team::lead_of(&team("")), None);
        assert_eq!(tools::team::lead_of(&team("gone")), None, "a lead that left the team counts as no lead");
    }
}
