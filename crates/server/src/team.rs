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
//! Turn-taking (the chatter-storm guard) is `act_targets`: a post carrying
//! mentions asks only the mentioned members to act; a deliberate post with
//! no mention from the owner or the organizer opens the floor and every
//! member may answer once; a reply (any member's, the organizer's included)
//! asks nobody unless it mentions someone.
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
/// - no mentions, a deliberate post (`is_reply` false) by the owner
///   (`from_agent_id` empty) or the organizer: every other member, once;
/// - no mentions, anything else — a reply, or another member's post:
///   nobody (a reply never re-opens the floor).
pub(crate) fn act_targets(
    from_agent_id: &str,
    organizer_agent_id: &str,
    members: &[String],
    mentioned: &[String],
    is_reply: bool,
) -> Vec<String> {
    if !mentioned.is_empty() {
        let mut out: Vec<String> = Vec::new();
        for m in mentioned {
            if members.contains(m) && m != from_agent_id && !out.contains(m) {
                out.push(m.clone());
            }
        }
        return out;
    }
    let opens_floor = !is_reply
        && (from_agent_id.is_empty()
            || (!organizer_agent_id.is_empty() && from_agent_id == organizer_agent_id));
    if !opens_floor {
        return Vec::new();
    }
    members
        .iter()
        .filter(|m| m.as_str() != from_agent_id)
        .cloned()
        .collect()
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

        let (sender_name, role) = if post.from_agent_id.is_empty() {
            ("Owner".to_string(), "user")
        } else {
            (
                state
                    .store
                    .get_agent(&post.from_agent_id)
                    .ok()
                    .flatten()
                    .map(|a| a.name)
                    .unwrap_or_else(|| post.from_agent_id.clone()),
                "assistant",
            )
        };

        // 1. The record: the team's own thread.
        let message = state
            .store
            .append_team_message(&team, role, &text, &sender_name, &post.from_agent_id, &attachments_json)
            .map_err(|e| format!("record team post: {e}"))?;
        state.hub.broadcast(
            tools::team::TEAM_MESSAGE_EVENT,
            serde_json::json!({
                "teamId": team.id,
                "messageId": message.id,
                "from": sender_name,
                "fromAgentId": post.from_agent_id,
                "senderName": sender_name,
                "role": role,
                "text": text,
                "attachments": attachments_json,
            }),
        );

        // 3. The optional hub mirror (before fan-out so a slow hub never
        // delays the members; best effort either way).
        mirror_to_hub(&state, &team, &text, &sender_name, &post).await;

        // 2. Fan-out: everyone receives the post; `act` decides who runs.
        let mut mentioned = post.mention.clone();
        for id in tools::team::mentioned_members(&text, &team.member_agent_ids) {
            if !mentioned.contains(&id) {
                mentioned.push(id);
            }
        }
        let act = act_targets(
            &post.from_agent_id,
            &team.organizer_agent_id,
            &team.member_agent_ids,
            &mentioned,
            post.is_reply,
        );
        let mut asked: Vec<String> = Vec::new();
        for (member_id, member_name) in &roster {
            if *member_id == post.from_agent_id {
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
                wait: false,
                team: Some(TeamDelivery {
                    team_id: team.id.clone(),
                    act: act_now,
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

/// Forward a post to the team's hub channel, if the team is mirrored and
/// the hub is connected. Local mention tokens become hub tokens for
/// employees the hub knows; failures are logged, never returned.
async fn mirror_to_hub(state: &AppState, team: &db::Team, text: &str, sender_name: &str, post: &TeamPost) {
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
        for id in &team.member_agent_ids {
            let token = format!("<@{id}>");
            if !content.contains(&token) {
                continue;
            }
            if let Ok(Some(a)) = state.store.get_agent(id) {
                if let Some(loop_id) = a.loop_agent_id.as_deref().filter(|s| !s.is_empty()) {
                    content = content.replace(&token, &format!("<@{loop_id}>"));
                }
            }
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
    use super::act_targets;

    fn members(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("m{i}")).collect()
    }

    /// Simulate the fan-out policy end to end: one deliberate post, then
    /// every member asked to act replies (with no mentions), and each reply
    /// is itself a post flagged as a reply. Returns the number of runs
    /// (deliveries that ask a member to act) until nobody is asked any more.
    fn simulate(first_from: &str, organizer: &str, members: &[String], mentioned: &[String]) -> usize {
        let mut runs = 0usize;
        let mut queue: Vec<(String, Vec<String>, bool)> =
            vec![(first_from.to_string(), mentioned.to_vec(), false)];
        let mut guard = 0usize;
        while let Some((from, mentions, is_reply)) = queue.pop() {
            guard += 1;
            assert!(guard < 1_000, "fan-out did not converge");
            for target in act_targets(&from, organizer, members, &mentions, is_reply) {
                runs += 1;
                // The member's reply: a post from it with no mentions.
                queue.push((target, Vec::new(), true));
            }
        }
        runs
    }

    /// The addendum's guarantee: a five-member team does not loop. An owner
    /// post opens the floor, every member (the organizer included) replies
    /// exactly once, and no reply causes a further delivery that asks anyone
    /// to act.
    #[test]
    fn five_member_team_does_not_loop() {
        let team = members(5);
        assert_eq!(simulate("", "m1", &team, &[]), 5);
        // The organizer deliberately posting again re-opens the floor:
        // everyone but itself, once.
        assert_eq!(simulate("m1", "m1", &team, &[]), 4);
        // A non-organizer member's unaddressed post asks nobody; neither
        // does the organizer's REPLY.
        assert_eq!(simulate("m3", "m1", &team, &[]), 0);
        assert!(act_targets("m1", "m1", &team, &[], true).is_empty());
    }

    /// Mentions narrow the ask to the mentioned members only, whoever posts;
    /// a member never asks itself; strangers are ignored.
    #[test]
    fn mentions_ask_only_the_mentioned() {
        let team = members(5);
        assert_eq!(
            act_targets("", "m1", &team, &["m2".into(), "m4".into()], false),
            vec!["m2".to_string(), "m4".to_string()]
        );
        assert_eq!(
            act_targets("m3", "m1", &team, &["m3".into(), "m5".into(), "zed".into()], true),
            vec!["m5".to_string()]
        );
        assert_eq!(simulate("m3", "m1", &team, &["m5".into()]), 1);
    }

    /// The owner always reaches everyone and resets the floor even when the
    /// team has no organizer on record.
    #[test]
    fn owner_opens_the_floor_without_an_organizer() {
        let team = members(3);
        assert_eq!(act_targets("", "", &team, &[], false), team);
        assert!(act_targets("m2", "", &team, &[], false).is_empty());
    }
}
