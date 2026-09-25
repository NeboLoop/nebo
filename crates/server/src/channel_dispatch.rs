//! ChannelDispatcher implementation — bridges channel_loop to run_chat_events.
//!
//! Implements `agent::ChannelDispatcher` by calling the unified chat dispatch
//! pipeline. Channel messages go through the same path as web UI chat: session
//! management, memory, steering, streaming, tool calls — the full agent.
//!
//! File uploads do NOT flow through this dispatcher. Each channel plugin owns
//! its upload mechanism via a `upload` CLI subcommand that uses the plugin's
//! existing API client and auth — see `docs/publishers-guide/channel-plugins.md`.

use std::future::Future;
use std::pin::Pin;

use ai::StreamEventType;
use tracing::warn;

use crate::state::AppState;

/// Server-side implementation of `agent::ChannelDispatcher`.
///
/// Captures `AppState` and calls `run_chat_events()` for each inbound channel
/// message, collecting the full response text.
pub struct ChannelDispatchImpl {
    state: AppState,
}

impl ChannelDispatchImpl {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// Server-side implementation of [`tools::CodeInstaller`] — routes any marketplace code
/// through the canonical `codes::handle_code` pathway (the same one this dispatcher and
/// the WS code-install flow use). Injected into `hire_employee` so
/// `hire_employee(code:…)` installs AND cascades every
/// artifact type (skills, plugins, agents, apps, collections) correctly.
pub struct CodeInstallerImpl {
    state: AppState,
}

impl CodeInstallerImpl {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl tools::CodeInstaller for CodeInstallerImpl {
    fn install<'a>(
        &'a self,
        code: &'a str,
        by: tools::InstalledBy,
    ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            match crate::codes::detect_code(code) {
                Some((code_type, validated)) => {
                    crate::codes::handle_code_text(&self.state, code_type, validated, by).await
                }
                None => format!(
                    "'{code}' is not a valid install code — expected PREFIX-XXXX-XXXX \
                     (e.g. SKIL-/PLUG-/AGNT-/APPS-/COLL-)."
                ),
            }
        })
    }
}

impl agent::ChannelDispatcher for ChannelDispatchImpl {
    fn dispatch<'a>(
        &'a self,
        agent_id: &'a str,
        session_key: &'a str,
        channel_ctx: tools::ChannelContext,
        prompt: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, String>> + Send + 'a>> {
        Box::pin(async move {
            // Intercept install codes before they reach the agent
            if let Some((code_type, code)) = crate::codes::detect_code(prompt) {
                // A code posted in a channel is not the owner's own act.
                let response = crate::codes::handle_code_text(
                    &self.state,
                    code_type,
                    code,
                    tools::InstalledBy::Other,
                )
                .await;
                return Ok(Some(response));
            }

            let entity_config =
                crate::entity_config::resolve_for_chat(&self.state.store, "agent", agent_id);

            let channel_kind = channel_ctx.kind.clone();
            let cancel_token = tokio_util::sync::CancellationToken::new();
            let config = crate::chat_dispatch::ChatConfig {
                session_key: session_key.to_string(),
                prompt: prompt.to_string(),
                user_id: String::new(),
                channel: channel_kind.clone(),
                // A Slack/Discord/Teams interlocutor is a third party typing
                // into someone else's product — the same standing as any other
                // inbound loop traffic, which `comm_origin` already gives to
                // everyone who is not the owner on their own personal loop.
                // As `User` this path skipped the deny wall, kept Full Access,
                // and could raise an approval modal on the owner's desktop, so
                // anyone who could post in a connected channel had the standing
                // of the owner typing locally. The taint seed below was already
                // calling this input untrusted; the origin now agrees with it.
                origin: tools::Origin::Comm,
                door: types::permissions::Door::Chat,
                agent_id: agent_id.to_string(),
                cancel_token: cancel_token.clone(),
                lane: types::constants::lanes::COMM.to_string(),
                comm_reply: None,
                entity_config,
                images: vec![],
                attachments: vec![],
                entity_name: String::new(),
                origin_agent_id: None,
                mention_context: None,
                tool_scope: None,
                plan_mode: false,
                channel_ctx: Some(channel_ctx),
                handoff_depth: 0,
                // Remote channel interlocutors (Slack/Discord) are untrusted
                // input — seed the run's taint accordingly.
                seed_taint: vec![types::provenance::ProvenanceClass::Channel],
                tool_allowlist: None,
                hidden_prompt: false,
                coworker: None,
                audience: None,
                cwd: None,
                model_override: None,
            };
            let channel = channel_kind.as_str();

            let rx = crate::chat_dispatch::run_chat_events(&self.state, config)
                .await
                .map_err(|e| format!("channel dispatch error: {}", e))?;

            // A message the running turn took is answered by that turn's
            // reply: nothing is posted for it, as in the app (A34).
            let reply = collect_channel_reply(rx, &cancel_token, agent_id, channel, None).await;
            Ok((!reply.queued).then_some(reply.text))
        })
    }
}

/// Post `text` into a chat-channel conversation (a Slack channel or thread)
/// through the employee's running bridge, as `op: "post"`: the employee
/// speaks on its own, with no inbound message to answer. The one way a
/// reply reaches a channel outside the inbound dispatch: a scheduled job
/// bound to a channel, and a turn woken there by a notification.
pub(crate) async fn post_to_channel(
    state: &AppState,
    agent_id: &str,
    ctx: &tools::ChannelContext,
    text: String,
) -> Result<(), String> {
    let key = tools::channel_bridge_key(agent_id, &ctx.kind);
    let Some(handle) = state.channel_bridges.read().await.get(&key).cloned() else {
        return Err(format!(
            "channel bridge `{key}` not running — enable {} for agent {} in Settings → Channels",
            ctx.kind, agent_id
        ));
    };
    let mut op = serde_json::Map::new();
    op.insert("op".into(), serde_json::Value::String("post".into()));
    op.insert("channel".into(), serde_json::Value::String(ctx.channel_id.clone()));
    if let Some(ts) = &ctx.thread_ts {
        op.insert("thread_ts".into(), serde_json::Value::String(ts.clone()));
    }
    op.insert("text".into(), serde_json::Value::String(text));
    handle
        .stdin_tx
        .send(serde_json::Value::Object(op))
        .await
        .map_err(|e| format!("bridge send: {e}"))
}

/// A turn in a chat-channel conversation that no inbound message started
/// (a notification woke it): run it as the channel's inbound turns run and
/// post its reply into the conversation (B14).
pub(crate) async fn answer_in_channel(state: &AppState, config: crate::chat_dispatch::ChatConfig, ctx: tools::ChannelContext) {
    let (agent_id, cancel_token, session_key) =
        (config.agent_id.clone(), config.cancel_token.clone(), config.session_key.clone());
    let rx = match crate::chat_dispatch::run_chat_events(state, config).await {
        Ok(rx) => rx,
        Err(e) => {
            warn!(session = %session_key, error = %e, "channel turn not started");
            return;
        }
    };
    let reply = collect_channel_reply(rx, &cancel_token, &agent_id, &ctx.kind, None).await;
    if reply.queued || reply.text.is_empty() {
        return;
    }
    if let Err(e) = post_to_channel(state, &agent_id, &ctx, reply.text).await {
        warn!(session = %session_key, error = %e, "channel reply not posted");
    }
}

/// Drain a chat run's event stream into the channel reply text, filtering out
/// internal status/progress notifications.
///
/// Reply accumulation is gated by [`crate::chat_dispatch::reply_fragment`]:
/// only `Text` events contribute. `ControlNotice` (step or spending limit,
/// terminal tool error) is run-control status and is ignored by type
/// — it must never land in a customer channel as prose.
///
/// A drained run's reply.
pub(crate) struct ChannelReply {
    pub text: String,
    /// Engine-stamped provenance of the run, so the coworker rail can label
    /// a tainted reply.
    pub provenance: Vec<types::provenance::ProvenanceClass>,
    /// The input went into a turn already running in that session: `text` is
    /// the busy line, not a reply; the running turn answers it.
    pub queued: bool,
}

/// `owner`: when the run happens on the LOCAL machine for the local owner
/// (coworker messages), approval and ask requests are forwarded to the owner's
/// frontend (same broadcasts `run_chat` emits) and the run parks on its
/// existing oneshot until the owner answers. When `None` (remote channels —
/// Slack/Discord — with no approval surface), an approval request cancels the
/// run with an honest notice, as before.
pub(crate) async fn collect_channel_reply(
    mut rx: tokio::sync::mpsc::Receiver<ai::StreamEvent>,
    cancel_token: &tokio_util::sync::CancellationToken,
    agent_id: &str,
    channel: &str,
    owner: Option<&crate::coworker::OwnerForward<'_>>,
) -> ChannelReply {
    let mut full_response = String::new();
    let mut queued = false;
    // Channels have no status banner — the reply is the only surface. Keep the
    // last control-notice status line as a FALLBACK so a run that terminates
    // before producing any prose doesn't answer with silence (which reads as
    // the bot ignoring the user). It is used only when the reply is empty.
    let mut last_control_notice: Option<String> = None;
    // Engine-stamped provenance of the run (Done events) — returned to the
    // caller so the coworker rail can label tainted replies.
    let mut reply_provenance: Vec<types::provenance::ProvenanceClass> = Vec::new();
    while let Some(event) = rx.recv().await {
        if let Some(frag) = crate::chat_dispatch::reply_fragment(&event) {
            full_response.push_str(frag);
            continue;
        }
        match event.event_type {
            StreamEventType::ControlNotice => {
                if event.stop_reason.as_deref() == Some(agent::harness::session_gate::QUEUED_INTO_RUNNING_TURN) {
                    queued = true;
                }
                if !event.text.trim().is_empty() {
                    last_control_notice = Some(event.text.clone());
                }
            }
            StreamEventType::Error => {
                warn!(
                    agent_id,
                    channel,
                    error = %event.text,
                    "channel chat error"
                );
            }
            StreamEventType::ApprovalRequest => {
                if let Some(fw) = owner {
                    // Local owner has a real approval surface: forward and let
                    // the run park on its oneshot until they answer.
                    if let Some(ref tc) = event.tool_call {
                        fw.forward_approval(tc);
                    }
                    continue;
                }
                warn!(
                    agent_id,
                    channel,
                    "channel chat requested approval; cancelling because channel dispatch has no approval UI"
                );
                cancel_token.cancel();
                if full_response.trim().is_empty() {
                    full_response.push_str(
                        "I need an approval before I can do that, but this channel can't show approval prompts. Enable Full Access or continue in the Nebo app.",
                    );
                }
                break;
            }
            StreamEventType::AskRequest => {
                if let Some(fw) = owner {
                    fw.forward_ask(&event).await;
                }
                // Remote channels: unchanged (asks are relayed by the comm
                // pipeline where configured, not by this collector).
            }
            StreamEventType::Done => {
                if let Some(p) = &event.provenance {
                    reply_provenance = p.clone();
                }
            }
            _ => {} // ToolCall, ToolResult, Usage — skip
        }
    }

    let reply = full_response.trim().to_string();

    // Empty-reply fallback: surface the terminal status line rather than
    // silence. Real prose always wins — the notice never mixes into it.
    let text = match last_control_notice {
        Some(notice) if reply.is_empty() => notice.trim().to_string(),
        _ => reply,
    };
    ChannelReply { text, provenance: reply_provenance, queued }
}

#[cfg(test)]
mod tests {
    use super::collect_channel_reply;

    /// A run emitting Text + ControlNotice must produce a reply containing
    /// ONLY the Text — the spiral/circuit-breaker notice leaked verbatim into
    /// a customer Slack channel when it was emitted as plain Text.
    #[tokio::test]
    async fn channel_reply_ignores_control_notices() {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let cancel = tokio_util::sync::CancellationToken::new();
        tx.send(ai::StreamEvent::text("Here is what I found so far."))
            .await
            .unwrap();
        tx.send(ai::StreamEvent::control_notice(
            "Stopped after 100 steps, the most one turn takes.",
            "max_steps",
        ))
        .await
        .unwrap();
        tx.send(ai::StreamEvent::text("\nTwo listings match your filters."))
            .await
            .unwrap();
        tx.send(ai::StreamEvent::done()).await.unwrap();
        drop(tx);

        let reply = collect_channel_reply(rx, &cancel, "agent-1", "slack", None).await.text;
        assert_eq!(
            reply,
            "Here is what I found so far.\nTwo listings match your filters."
        );
        assert!(!reply.contains("Stopped"));
        assert!(!cancel.is_cancelled());
    }

    /// A run that terminates with NO prose must not answer a channel with
    /// silence — the last control-notice status line serves as the fallback
    /// reply (channels have no status banner; the reply is the only surface).
    #[tokio::test]
    async fn channel_reply_falls_back_to_notice_when_empty() {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let cancel = tokio_util::sync::CancellationToken::new();
        tx.send(ai::StreamEvent::control_notice(
            "I couldn't reach gws — reconnect this account in Settings, then ask me again.",
            "terminal_tool_error",
        ))
        .await
        .unwrap();
        tx.send(ai::StreamEvent::done()).await.unwrap();
        drop(tx);

        let reply = collect_channel_reply(rx, &cancel, "agent-1", "slack", None).await;
        assert!(!reply.queued);
        let reply = reply.text;
        assert_eq!(
            reply,
            "I couldn't reach gws — reconnect this account in Settings, then ask me again."
        );
    }

    /// Input that went into a turn already running is not answered by the
    /// busy line: the collector says it was queued, so nobody reads that
    /// line as the reply.
    #[tokio::test]
    async fn a_queued_input_is_not_a_reply() {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let cancel = tokio_util::sync::CancellationToken::new();
        tx.send(ai::StreamEvent::control_notice(
            "Got it. I'll pick this up at my next step.",
            agent::harness::session_gate::QUEUED_INTO_RUNNING_TURN,
        ))
        .await
        .unwrap();
        tx.send(ai::StreamEvent::done()).await.unwrap();
        drop(tx);
        assert!(collect_channel_reply(rx, &cancel, "agent-1", "coworker", None).await.queued);
    }
}
