//! Where a woken turn's reply goes, and as whom it runs: the conversation
//! the session's work came from (hub check O3, Batch B13; review 5.2).
//!
//! A turn the owner starts replies where the owner wrote; a coworker's turn
//! replies to whoever messaged it. A turn woken by a notification (a helper's
//! result, a coworker's reply, an answered ask) has no input of its own to
//! say where that is, or who it is with, so every session keeps its route
//! and its seat, and the wake rail hands both to the woken turn: it
//! continues that conversation as the same party, with the same limits
//! (a woken turn inherits the seat of the session that owns it, never the
//! sender's). Both are durable (the
//! session row), because a wake survives a restart.

use serde::{Deserialize, Serialize};

use crate::chat_dispatch::CommReplyConfig;
use crate::state::AppState;

/// The conversation a session's replies go back to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ReplyRoute {
    /// A loop or phone conversation.
    Comm {
        provider: String,
        topic: String,
        conversation_id: String,
        handoff_depth: u8,
        approval_relay: bool,
        from_agent_id: String,
    },
    /// A coworker's thread: its words are recorded in the sender's thread or
    /// the team it was asked in; who hears its answer is the addressing's
    /// (`addressing`).
    Coworker(CoworkerRoute),
    /// A chat-channel conversation (Slack, Discord, Teams): the reply is
    /// posted into it, in the thread it came from.
    Channel { channel_ctx: tools::ChannelContext },
}

/// Who a coworker thread answers, and as whom its turns run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CoworkerRoute {
    /// The employee whose thread this is.
    pub to_agent_id: String,
    pub to_name: String,
    /// The sending employee ("" = the main one, or the owner in a team).
    pub from_agent_id: String,
    pub from_name: String,
    /// The sender's own record of the exchange (`None` for the main
    /// employee, whose conversation already shows it).
    pub mirror_key: Option<String>,
    /// The hop count of the message that started the thread's work.
    pub sender_depth: u8,
    /// Set for a team member's thread: the reply is posted into the team.
    pub team: Option<TeamLeg>,
    /// Whose request the thread's work serves (`coworker::seat_authority`):
    /// its turns, the message's and every one a notification wakes, run with
    /// that authority, and what the seat sends on passes it on. A route
    /// stored before there was one reads as a colleague's request.
    #[serde(default)]
    pub authority: tools::coworker::Authority,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TeamLeg {
    pub team_id: String,
    pub team_name: String,
}

impl ReplyRoute {
    pub(crate) fn comm(cfg: &CommReplyConfig) -> Self {
        ReplyRoute::Comm {
            provider: cfg.provider.clone(),
            topic: cfg.topic.clone(),
            conversation_id: cfg.conversation_id.clone(),
            handoff_depth: cfg.handoff_depth,
            approval_relay: cfg.approval_relay,
            from_agent_id: cfg.from_agent_id.clone(),
        }
    }

    /// The comm reply of a `Comm` route.
    pub(crate) fn comm_reply(&self) -> Option<CommReplyConfig> {
        match self {
            ReplyRoute::Comm {
                provider,
                topic,
                conversation_id,
                handoff_depth,
                approval_relay,
                from_agent_id,
            } => Some(CommReplyConfig {
                provider: provider.clone(),
                topic: topic.clone(),
                conversation_id: conversation_id.clone(),
                handoff_depth: *handoff_depth,
                approval_relay: *approval_relay,
                from_agent_id: from_agent_id.clone(),
            }),
            ReplyRoute::Coworker(_) | ReplyRoute::Channel { .. } => None,
        }
    }
}

/// Who a session's conversation is with: the seat its last input ran in,
/// which a turn woken there runs in too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WakeSeat {
    pub origin: tools::Origin,
    pub door: types::permissions::Door,
    /// The coworker the conversation replies to.
    pub audience: Option<String>,
    /// The restricted-run allowlist (an outside caller's, a visitor's).
    pub tool_allowlist: Option<std::collections::BTreeSet<String>>,
    /// The chat channel the conversation is in (Slack, Discord…).
    pub channel_ctx: Option<tools::ChannelContext>,
}

impl WakeSeat {
    /// The seat of a session no input ever reached (a scheduled run's):
    /// the system's own unattended work.
    pub(crate) fn system() -> Self {
        Self {
            origin: tools::Origin::System,
            door: types::permissions::Door::Chat,
            audience: None,
            tool_allowlist: None,
            channel_ctx: None,
        }
    }
}

const ROUTE: &str = "replyRoute";
const SEAT: &str = "wakeSeat";

/// Record `route` for session `session_key` of `user_id` (`None` clears it).
pub(crate) fn set(state: &AppState, session_key: &str, user_id: &str, route: Option<&ReplyRoute>) {
    write(state, session_key, user_id, ROUTE, route.map(|r| serde_json::to_string(r).unwrap_or_default()));
}

/// Record the seat of the input session `session_key` just received.
pub(crate) fn set_seat(state: &AppState, session_key: &str, user_id: &str, seat: &WakeSeat) {
    write(state, session_key, user_id, SEAT, Some(serde_json::to_string(seat).unwrap_or_default()));
}

fn write(state: &AppState, session_key: &str, user_id: &str, field: &str, json: Option<String>) {
    let sessions = state.harness.sessions();
    let written = sessions
        .get_or_create(session_key, user_id)
        .map_err(|e| e.to_string())
        .and_then(|s| state.store.set_session_meta(&s.id, field, json.as_deref()).map_err(|e| e.to_string()));
    if let Err(e) = written {
        tracing::warn!(session = %session_key, field, error = %e, "not recorded: a woken turn here has only the defaults");
    }
}

/// The route session `session_key` keeps, if any.
pub(crate) fn of(state: &AppState, session_key: &str) -> Option<ReplyRoute> {
    read(state, session_key, ROUTE)
}

/// The seat session `session_key` keeps, if any.
pub(crate) fn seat_of(state: &AppState, session_key: &str) -> Option<WakeSeat> {
    read(state, session_key, SEAT)
}

fn read<T: serde::de::DeserializeOwned>(state: &AppState, session_key: &str, field: &str) -> Option<T> {
    let id = state
        .harness
        .sessions()
        .resolve_session_id_by_key(session_key)
        .ok()?;
    let json = state.store.session_meta(&id, field).ok()??;
    match serde_json::from_str(&json) {
        Ok(value) => Some(value),
        Err(e) => {
            tracing::warn!(session = %session_key, field, error = %e, "unreadable session record");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A route survives the store as written, both kinds.
    #[test]
    fn a_route_round_trips() {
        let comm = ReplyRoute::Comm {
            provider: "neboai".into(),
            topic: "dm".into(),
            conversation_id: "c1".into(),
            handoff_depth: 1,
            approval_relay: true,
            from_agent_id: String::new(),
        };
        let cw = ReplyRoute::Coworker(CoworkerRoute {
            to_agent_id: "bk".into(),
            to_name: "Bookkeeper".into(),
            from_agent_id: String::new(),
            from_name: "Nebo".into(),
            mirror_key: None,
            sender_depth: 0,
            team: Some(TeamLeg {
                team_id: "t1".into(),
                team_name: "Floor".into(),
            }),
            authority: tools::coworker::Authority::OwnersRequest { request: "p1".into() },
        });
        let channel = ReplyRoute::Channel {
            channel_ctx: tools::ChannelContext { kind: "slack".into(), channel_id: "C1".into(), thread_ts: Some("1.2".into()) },
        };
        for r in [comm, cw, channel] {
            let back: ReplyRoute =
                serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
            assert_eq!(back, r);
        }
        let seat = WakeSeat {
            origin: tools::Origin::Comm,
            door: types::permissions::Door::Coworker { from: "bk".into() },
            audience: Some("bk".into()),
            tool_allowlist: Some(["read_calendar".to_string()].into()),
            channel_ctx: Some(tools::ChannelContext { kind: "slack".into(), channel_id: "C1".into(), thread_ts: None }),
        };
        let back: WakeSeat = serde_json::from_str(&serde_json::to_string(&seat).unwrap()).unwrap();
        assert_eq!(back, seat);
    }

    /// A coworker route stored before routes carried whose request they
    /// serve reads as a colleague's: nothing stored earlier gains the
    /// owner's authority.
    #[test]
    fn an_older_route_reads_as_a_colleagues_request() {
        let stored = r#"{"kind":"coworker","to_agent_id":"bk","to_name":"Bookkeeper","from_agent_id":"","from_name":"Owner","reply_to":null,"mirror_key":null,"sender_depth":0,"team":{"team_id":"t1","team_name":"Floor"}}"#;
        let ReplyRoute::Coworker(route) = serde_json::from_str::<ReplyRoute>(stored).unwrap() else {
            panic!("a coworker route");
        };
        assert_eq!(route.authority, tools::coworker::Authority::Coworker);
    }
}
