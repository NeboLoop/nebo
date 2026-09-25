//! Teams — the server leg of a team post. A team is a LOCAL object
//! (`db::Team`, thread `team:<id>`); posting into one:
//!
//! 1. appends the post to the team's local thread and broadcasts it,
//! 2. fans it out to every member through the ONE coworker rail
//!    (`coworker::send_coworker_message` with a team leg): everyone receives
//!    the post; the members asked to act run and their replies come back
//!    here as posts,
//! 3. forwards it to the team's hub channel when the team is mirrored —
//!    best effort, never on the critical path.
//!
//! Turn-taking (the chatter-storm guard) is `act_targets`: the team lead
//! manages the room. A post carrying mentions asks only the mentioned
//! members to act; the owner's unaddressed post goes to the lead alone,
//! who answers the owner and hands steps to teammates by mention; only an
//! explicit @everyone (from the owner or the lead) asks the whole team; a
//! reply asks nobody unless it mentions someone. A team with no lead falls
//! back to everyone once, so nothing is ever left unanswered.
//! Every delivery carries the depth of the post that caused it, and the
//! rail's existing depth limit refuses past it.

use std::future::Future;
use std::pin::Pin;

use tools::coworker::{CoworkerMessage, TeamDelivery, TeamPost, TeamPostReceipt};

use crate::state::AppState;

/// Which members a post asks to act. Pure — the whole turn-taking policy.
///
/// - mentions present: the mentioned members (that are in the team), never
///   the poster itself;
/// - `everyone` (an explicit @everyone) from the owner (`from_agent_id`
///   empty) or the lead: every other member, once — even in a reply, since
///   the lead runs the room;
/// - no mentions, a deliberate post (`is_reply` false) by the owner: the
///   lead alone; with no lead on record, every member once;
/// - anything else — the lead's own unaddressed post, a reply, another
///   member's post: nobody. The lead delegates by mention, not by posting.
pub(crate) fn act_targets(
    from_agent_id: &str,
    organizer_agent_id: &str,
    members: &[String],
    mentioned: &[String],
    is_reply: bool,
    everyone: bool,
) -> Vec<String> {
    let others = || -> Vec<String> {
        members
            .iter()
            .filter(|m| m.as_str() != from_agent_id)
            .cloned()
            .collect()
    };
    let from_owner = from_agent_id.is_empty();
    let from_lead = !organizer_agent_id.is_empty() && from_agent_id == organizer_agent_id;
    if everyone && (from_owner || from_lead) {
        return others();
    }
    if !mentioned.is_empty() {
        let mut out: Vec<String> = Vec::new();
        for m in mentioned {
            if members.contains(m) && m != from_agent_id && !out.contains(m) {
                out.push(m.clone());
            }
        }
        return out;
    }
    if !from_owner || is_reply {
        return Vec::new();
    }
    match lead_for_unaddressed(organizer_agent_id, members) {
        Some(lead) => vec![lead],
        None => others(),
    }
}

/// The ONE member an owner's unaddressed post goes to: the lead, when one is
/// on record and on the team. The single rule behind both ways of talking to
/// a team — a typed post (`act_targets`) and a voice call opened from the
/// team thread. `None` = the team has no lead: a typed post then reaches
/// every member once, so nothing is left unanswered; a call, which needs one
/// speaker, refuses instead.
pub(crate) fn lead_for_unaddressed(organizer_agent_id: &str, members: &[String]) -> Option<String> {
    (!organizer_agent_id.is_empty() && members.iter().any(|m| m == organizer_agent_id))
        .then(|| organizer_agent_id.to_string())
}

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

        // 1. The record: the team's own thread.
        let (message, sender_name) = record(
            &state,
            &team,
            TeamSender::Local(&post.from_agent_id),
            &text,
            &attachments_json,
        )?;

        // Who acts is decided once, before anything is sent, because the hub
        // mirror has to carry the asks for members on other machines.
        let member_ids = tools::team::member_ids(&team);
        let mut mentioned = post.mention.clone();
        for id in tools::team::mentioned_members(&text, &member_ids) {
            if !mentioned.contains(&id) {
                mentioned.push(id);
            }
        }
        let act = act_targets(
            &post.from_agent_id,
            &team.organizer_agent_id,
            &member_ids,
            &mentioned,
            post.is_reply,
            tools::team::mentions_everyone(&text),
        );

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

        let mut asked: Vec<String> = Vec::new();
        for m in &remote_asks {
            let name = if m.name.is_empty() { m.agent_id.clone() } else { m.name.clone() };
            asked.push(name);
        }

        // 2. Fan-out over the local rail. A remote member is not on it — it
        // was asked through the hub above.
        for (member_id, member_name) in &roster {
            if *member_id == post.from_agent_id {
                continue;
            }
            if team
                .members
                .iter()
                .any(|m| m.agent_id == *member_id && !m.is_local())
            {
                continue;
            }
            let act_now = act.contains(member_id);
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
                    act: act_now,
                    reply_to: post.reply_to.clone(),
                }),
            };
            match crate::coworker::send_coworker_message(state.clone(), msg).await {
                Ok(_) => {
                    if act_now {
                        asked.push(member_name.clone());
                    }
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    team = %team.id,
                    member = %member_id,
                    act = act_now,
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
/// calls it for every message the mirrored channel delivers. Returns the
/// row and the sender's display name ("Owner" for the owner of this Nebo).
pub(crate) fn record(
    state: &AppState,
    team: &db::Team,
    sender: TeamSender<'_>,
    text: &str,
    attachments: &serde_json::Value,
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
        .append_team_message(team, role, text, &sender_name, from_agent_id, attachments)
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
    use super::{act_targets, lead_for_unaddressed};

    fn members(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("m{i}")).collect()
    }

    /// Simulate the fan-out policy end to end: one post, then every member
    /// asked to act replies (with no mentions), each reply itself a post
    /// flagged as a reply. Returns the number of runs until nobody is asked.
    fn simulate(first_from: &str, lead: &str, members: &[String], mentioned: &[String], everyone: bool) -> usize {
        let mut runs = 0usize;
        let mut queue: Vec<(String, Vec<String>, bool, bool)> =
            vec![(first_from.to_string(), mentioned.to_vec(), false, everyone)];
        let mut guard = 0usize;
        while let Some((from, mentions, is_reply, all)) = queue.pop() {
            guard += 1;
            assert!(guard < 1_000, "fan-out did not converge");
            for target in act_targets(&from, lead, members, &mentions, is_reply, all) {
                runs += 1;
                queue.push((target, Vec::new(), true, false));
            }
        }
        runs
    }

    /// The owner's unaddressed post reaches the lead alone; the lead's reply
    /// asks nobody; a five-member team runs exactly once.
    #[test]
    fn owner_post_goes_to_the_lead_alone() {
        let team = members(5);
        assert_eq!(act_targets("", "m1", &team, &[], false, false), vec!["m1".to_string()]);
        assert_eq!(simulate("", "m1", &team, &[], false), 1);
        // The lead posting without addressing anyone asks nobody: it
        // delegates by mention, not by posting.
        assert!(act_targets("m1", "m1", &team, &[], false, false).is_empty());
        // A non-lead member's unaddressed post asks nobody either.
        assert!(act_targets("m3", "m1", &team, &[], false, false).is_empty());
    }

    /// @everyone from the owner or the lead asks the whole team once — and
    /// still does not loop, because replies never re-open the floor.
    #[test]
    fn everyone_asks_the_whole_team_once() {
        let team = members(5);
        assert_eq!(act_targets("", "m1", &team, &[], false, true), team);
        assert_eq!(simulate("", "m1", &team, &[], true), 5);
        assert_eq!(act_targets("m1", "m1", &team, &[], true, true).len(), 4);
        // A member who is not the lead cannot summon everyone.
        assert!(act_targets("m3", "m1", &team, &[], false, true).is_empty());
    }

    /// Mentions narrow the ask to the mentioned members only, whoever posts;
    /// a member never asks itself; strangers are ignored.
    #[test]
    fn mentions_ask_only_the_mentioned() {
        let team = members(5);
        assert_eq!(
            act_targets("", "m1", &team, &["m2".into(), "m4".into()], false, false),
            vec!["m2".to_string(), "m4".to_string()]
        );
        assert_eq!(
            act_targets("m3", "m1", &team, &["m3".into(), "m5".into(), "zed".into()], true, false),
            vec!["m5".to_string()]
        );
        assert_eq!(simulate("m3", "m1", &team, &["m5".into()], false), 1);
    }

    /// With no lead on record the owner still reaches everyone once, so a
    /// team is never left unanswered.
    #[test]
    fn owner_reaches_everyone_without_a_lead() {
        let team = members(3);
        assert_eq!(act_targets("", "", &team, &[], false, false), team);
        assert!(act_targets("m2", "", &team, &[], false, false).is_empty());
    }

    /// The lead is ONE rule for text and voice: the member on record, if it
    /// is on the team; nobody when there is no lead or the lead left the
    /// team — and then the typed post fans out to everyone (above) while a
    /// call refuses, never picks a member.
    #[test]
    fn the_lead_is_one_rule_for_text_and_voice() {
        let team = members(3);
        assert_eq!(lead_for_unaddressed("m2", &team).as_deref(), Some("m2"));
        assert_eq!(lead_for_unaddressed("", &team), None);
        assert_eq!(lead_for_unaddressed("gone", &team), None);
        assert_eq!(
            act_targets("", "gone", &team, &[], false, false),
            team,
            "a lead that left the team counts as no lead"
        );
    }
}
