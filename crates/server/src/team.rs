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

/// Who wrote a team row, for `record`. Everything that happens on this Nebo
/// is `Local`: the owner (empty id) or one of this Nebo's employees, whose
/// name is looked up here. A post that arrived through the team's hub mirror
/// is `Hub`: its author lives on another Nebo, so only the hub message knows
/// the name and whether it was a person or an employee.
pub(crate) enum TeamSender<'a> {
    Local(&'a str),
    Hub { agent_id: &'a str, name: &'a str, is_agent: bool },
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
        let text = post.text.trim();
        if text.is_empty() {
            return Err("text required".to_string());
        }
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
            TeamSender::Local(&post.from_agent_id),
            &text,
            &attachments_json,
            &post.provenance,
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
        // delays the members; best effort either way).
        mirror_to_hub(&state, &team, &text, &sender_name, &post, &remote_asks).await;

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

        let mut asked: Vec<String> = Vec::new();
        for m in &remote_asks {
            let name = if m.name.is_empty() { m.agent_id.clone() } else { m.name.clone() };
            asked.push(name);
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
                sender_session_key: db::team_thread_key(&team.id),
                to: member_id.clone(),
                text: text.clone(),
                requester_scope: String::new(),
                handoff_depth: post.handoff_depth,
                provenance: post.provenance.clone(),
                team: Some(TeamDelivery {
                    team_id: team.id.clone(),
                    post_id: message.id.clone(),
                    reply_to: post.reply_to.clone(),
                    authority: authority.clone(),
                }),
                conversation: None,
                // The delivery carries the post's authority, decided above.
                owners_turn: None,
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
/// the team's thread and broadcast `team_message`, so every open team view
/// (desktop and mobile) shows it live. The ONE writer of a team row. `post`
/// calls it before the fan-out; a turn spoken in the team thread's voice
/// mode calls it alone, because the lead already answered out loud and a
/// fan-out would ask it the same thing again in text; the hub-mirror feed
/// calls it for every message the mirrored channel delivers; a member that
/// takes a post it was asked to act on acknowledges through it
/// (`coworker::OwnerForward::acknowledge`). Returns the
/// row and the sender's display name ("Owner" for the owner of this Nebo).
/// `provenance` is the posting run's: it rides the row as metadata, for the
/// members briefed with the post and the gates, never in the words.
pub(crate) fn record(
    state: &AppState,
    team: &db::Team,
    sender: TeamSender<'_>,
    text: &str,
    attachments: &serde_json::Value,
    provenance: &[types::provenance::ProvenanceClass],
) -> Result<(db::TeamMessage, String), String> {
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
        TeamSender::Hub { agent_id, name, is_agent } => {
            (agent_id, name.to_string(), if is_agent { "assistant" } else { "user" })
        }
    };
    let message = state
        .store
        .append_team_message(team, role, text, &sender_name, from_agent_id, attachments, provenance)
        .map_err(|e| format!("record team post: {e}"))?;
    state.hub.broadcast(
        tools::team::TEAM_MESSAGE_EVENT,
        serde_json::json!({
            "teamId": team.id,
            "messageId": message.id,
            "from": sender_name,
            "fromAgentId": from_agent_id,
            "senderName": sender_name,
            "role": role,
            "text": text,
            "attachments": attachments,
        }),
    );
    Ok((message, sender_name))
}

/// Forward a post to the team's hub channel, if the team is mirrored and
/// the hub is connected. Local mention tokens become hub tokens for
/// employees the hub knows; failures are logged, never returned.
async fn mirror_to_hub(
    state: &AppState,
    team: &db::Team,
    text: &str,
    sender_name: &str,
    post: &TeamPost,
    remote_asks: &[&db::TeamMember],
) {
    let Some(channel_id) = team.hub_channel_id.as_deref().filter(|c| !c.is_empty()) else {
        return;
    };
    let Some(plugin) = state.comm_manager.active_plugin().await else {
        return;
    };
    if !plugin.is_connected() {
        return;
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
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("senderName".to_string(), sender_name.to_string());
    if post.from_agent_id.is_empty() {
        metadata.insert("role".to_string(), "user".to_string());
    } else {
        metadata.insert("senderKind".to_string(), "agent".to_string());
        metadata.insert("fromAgentName".to_string(), sender_name.to_string());
        if post.handoff_depth > 0 {
            metadata.insert("handoffDepth".to_string(), post.handoff_depth.to_string());
        }
    }
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
        human_injected: post.from_agent_id.is_empty(),
        human_id: None,
        task_id: None,
        correlation_id: None,
        task_status: None,
        artifacts: Vec::new(),
        error: None,
        attachments: Vec::new(),
    };
    if let Err(e) = state.comm_manager.send(msg).await {
        tracing::warn!(error = %e, team = %team.id, "team: hub mirror send failed (team stays local)");
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
        };
        assert_eq!(tools::team::lead_of(&team("m2")), Some("m2"));
        assert_eq!(tools::team::lead_of(&team("")), None);
        assert_eq!(tools::team::lead_of(&team("gone")), None, "a lead that left the team counts as no lead");
    }
}
