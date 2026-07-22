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
/// the WS code-install flow use). Injected into the agent's `registry` install action so
/// `agent(resource:"registry", action:"install", code:…)` installs AND cascades every
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
    fn install<'a>(&'a self, code: &'a str) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            match crate::codes::detect_code(code) {
                Some((code_type, validated)) => {
                    crate::codes::handle_code_text(&self.state, code_type, validated).await
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
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            // Intercept install codes before they reach the agent
            if let Some((code_type, code)) = crate::codes::detect_code(prompt) {
                let response = crate::codes::handle_code_text(&self.state, code_type, code).await;
                return Ok(response);
            }

            let entity_config =
                crate::entity_config::resolve_for_chat(&self.state.store, "agent", agent_id);

            let channel_kind = channel_ctx.kind.clone();
            let cancel_token = tokio_util::sync::CancellationToken::new();
            let config = crate::chat_dispatch::ChatConfig {
                session_key: session_key.to_string(),
                prompt: prompt.to_string(),
                system: String::new(),
                user_id: String::new(),
                channel: channel_kind.clone(),
                origin: tools::Origin::User,
                agent_id: agent_id.to_string(),
                cancel_token: cancel_token.clone(),
                lane: types::constants::lanes::COMM.to_string(),
                comm_reply: None,
                entity_config,
                images: vec![],
                entity_name: String::new(),
                origin_agent_id: None,
                mention_context: None,
                tool_scope: None,
                plan_mode: false,
                channel_ctx: Some(channel_ctx),
            };
            let channel = channel_kind.as_str();

            let rx = crate::chat_dispatch::run_chat_events(&self.state, config)
                .await
                .map_err(|e| format!("channel dispatch error: {}", e))?;

            Ok(collect_channel_reply(rx, &cancel_token, agent_id, channel).await)
        })
    }
}

/// Drain a chat run's event stream into the channel reply text, filtering out
/// internal status/progress notifications.
///
/// Reply accumulation is gated by [`crate::chat_dispatch::reply_fragment`]:
/// only `Text` events contribute. `ControlNotice` (spiral backstop, circuit
/// breaker, terminal tool error) is run-control status and is ignored by type
/// — it must never land in a customer channel as prose.
pub(crate) async fn collect_channel_reply(
    mut rx: tokio::sync::mpsc::Receiver<ai::StreamEvent>,
    cancel_token: &tokio_util::sync::CancellationToken,
    agent_id: &str,
    channel: &str,
) -> String {
    let mut full_response = String::new();
    // Channels have no status banner — the reply is the only surface. Keep the
    // last control-notice status line as a FALLBACK so a run that terminates
    // before producing any prose doesn't answer with silence (which reads as
    // the bot ignoring the user). It is used only when the reply is empty.
    let mut last_control_notice: Option<String> = None;
    while let Some(event) = rx.recv().await {
        if let Some(frag) = crate::chat_dispatch::reply_fragment(&event) {
            // Skip orchestrator progress notifications — the
            // "_Working on: ..._" heartbeat (shared predicate) and
            // the background-task notice are status, not content.
            if crate::chat_dispatch::is_progress_heartbeat(frag) {
                continue;
            }
            if frag.trim() == "Working on this in the background..." {
                continue;
            }
            full_response.push_str(frag);
            continue;
        }
        match event.event_type {
            StreamEventType::ControlNotice => {
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
            _ => {} // ToolCall, ToolResult, Usage, Done — skip
        }
    }

    // Clean up any residual empty lines from filtered status messages
    let reply = full_response
        .lines()
        .filter(|line| !line.trim().is_empty() || full_response.matches('\n').count() < 20)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    // Empty-reply fallback: surface the terminal status line rather than
    // silence. Real prose always wins — the notice never mixes into it.
    if reply.is_empty() {
        if let Some(notice) = last_control_notice {
            return notice.trim().to_string();
        }
    }
    reply
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
            "Stopped: 'web(search)' was called 8 times this turn without progress.",
            "repeated_tool_calls",
        ))
        .await
        .unwrap();
        tx.send(ai::StreamEvent::text("\nTwo listings match your filters."))
            .await
            .unwrap();
        tx.send(ai::StreamEvent::done()).await.unwrap();
        drop(tx);

        let reply = collect_channel_reply(rx, &cancel, "agent-1", "slack").await;
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

        let reply = collect_channel_reply(rx, &cancel, "agent-1", "slack").await;
        assert_eq!(
            reply,
            "I couldn't reach gws — reconnect this account in Settings, then ask me again."
        );
    }
}
