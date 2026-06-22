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

            let mut rx = crate::chat_dispatch::run_chat_events(&self.state, config)
                .await
                .map_err(|e| format!("channel dispatch error: {}", e))?;

            // Collect all text events into the full response,
            // filtering out internal status/progress notifications.
            let mut full_response = String::new();
            while let Some(event) = rx.recv().await {
                match event.event_type {
                    StreamEventType::Text => {
                        // Skip orchestrator progress notifications — the
                        // "_Working on: ..._" heartbeat (shared predicate) and
                        // the background-task notice are status, not content.
                        if crate::chat_dispatch::is_progress_heartbeat(&event.text) {
                            continue;
                        }
                        if event.text.trim() == "Working on this in the background..." {
                            continue;
                        }
                        full_response.push_str(&event.text);
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
                    _ => {} // ToolCall, ToolResult, Usage, Stop — skip
                }
            }

            // Clean up any residual empty lines from filtered status messages
            let cleaned = full_response
                .lines()
                .filter(|line| !line.trim().is_empty() || full_response.matches('\n').count() < 20)
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string();

            Ok(cleaned)
        })
    }
}
