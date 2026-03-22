//! Unified chat dispatch — ONE way to run any chat (primary, role, channel, comm).
//!
//! Every chat entry point (WebSocket, REST, NeboLoop) builds a [`ChatConfig`] with
//! the appropriate decorators and calls [`run_chat`]. The underlying lane infrastructure,
//! event streaming, and response handling are identical for all chat types.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use agent::lanes::make_task;
use agent::RunRequest;
use ai::StreamEventType;
use tools::Origin;

use crate::state::AppState;

/// An active agent run with its cancellation token and start time.
pub struct ActiveRun {
    pub token: CancellationToken,
    pub started_at: std::time::Instant,
}

/// Tracks active agent runs so they can be cancelled (e.g., from WebSocket).
pub type ActiveRuns = Arc<Mutex<HashMap<String, ActiveRun>>>;

/// Guard that removes a session from active_runs on drop (panic-safe cleanup).
struct ActiveRunGuard {
    active_runs: Option<ActiveRuns>,
    session_id: String,
}

impl Drop for ActiveRunGuard {
    fn drop(&mut self) {
        if let Some(ref runs) = self.active_runs {
            if let Ok(mut guard) = runs.try_lock() {
                guard.remove(&self.session_id);
            }
        }
    }
}

/// Configuration for a chat run — decorators that customize behavior
/// without changing the underlying execution flow.
pub struct ChatConfig {
    pub session_key: String,
    pub prompt: String,
    pub system: String,
    pub user_id: String,
    pub channel: String,
    pub origin: Origin,
    pub role_id: String,
    pub cancel_token: CancellationToken,
    /// Which lane to enqueue on (e.g., lanes::MAIN, lanes::COMM).
    pub lane: String,
    /// If set, sends the accumulated text response back via comm after completion.
    pub comm_reply: Option<CommReplyConfig>,
    /// Per-entity resolved config — permissions, resource grants, model, personality.
    pub entity_config: Option<crate::entity_config::ResolvedEntityConfig>,
    /// Images attached to the user's message (base64-encoded).
    pub images: Vec<ai::ImageContent>,
}

/// Configuration for sending a reply back through a communication channel.
#[derive(Clone)]
pub struct CommReplyConfig {
    pub topic: String,
    pub conversation_id: String,
}

/// Single entry point for all chat dispatch.
///
/// Callers configure behavior via [`ChatConfig`] decorators. Optionally pass
/// [`ActiveRuns`] to enable external cancellation (WebSocket cancel messages).
pub async fn run_chat(state: &AppState, config: ChatConfig, active_runs: Option<ActiveRuns>) {
    let hub = state.hub.clone();
    let runner = state.runner.clone();
    let janus_usage = state.janus_usage.clone();
    let comm_manager = if config.comm_reply.is_some() {
        Some(state.comm_manager.clone())
    } else {
        None
    };

    let sid = config.session_key.clone();
    let role_id = config.role_id.clone();
    let cancel_token = config.cancel_token.clone();
    let lane = config.lane.clone();

    // Track for external cancellation if active_runs provided
    if let Some(ref runs) = active_runs {
        runs.lock().await.insert(sid.clone(), ActiveRun {
            token: cancel_token.clone(),
            started_at: std::time::Instant::now(),
        });
    }

    // Broadcast chat_created so frontend can track new conversations
    hub.broadcast(
        "chat_created",
        serde_json::json!({
            "session_id": sid,
            "channel": config.channel,
            "role_id": role_id,
        }),
    );

    // Destructure config fields before moving into closure
    let prompt = config.prompt;
    let system = config.system;
    let user_id = config.user_id;
    let channel = config.channel;
    let origin = config.origin;
    let comm_reply = config.comm_reply;
    let entity_cfg = config.entity_config;
    let images = config.images;

    let lane_task = make_task(&lane, format!("chat:{}", sid), async move {
        // Panic-safe cleanup: removes session from active_runs even on panic
        let _run_guard = ActiveRunGuard {
            active_runs: active_runs.clone(),
            session_id: sid.clone(),
        };

        // Extract per-entity overrides from resolved config
        let (permissions, resource_grants, model_preference, personality_snippet) =
            if let Some(ref ec) = entity_cfg {
                (
                    Some(ec.permissions.clone()),
                    Some(ec.resource_grants.clone()),
                    ec.model_preference.clone(),
                    ec.personality_snippet.clone(),
                )
            } else {
                (None, None, None, None)
            };
        let allowed_paths = entity_cfg.as_ref().map(|ec| ec.allowed_paths.clone()).unwrap_or_default();

        let req = RunRequest {
            session_key: sid.clone(),
            prompt,
            system,
            user_id,
            channel,
            origin,
            cancel_token: cancel_token.clone(),
            role_id: role_id.clone(),
            permissions,
            resource_grants,
            model_preference,
            personality_snippet,
            images,
            allowed_paths,
            ..Default::default()
        };

        match runner.run(req).await {
            Ok(mut rx) => {
                let mut full_response = String::new();
                let mut text_buffer = String::new();
                let mut last_flush = tokio::time::Instant::now();
                const COALESCE_MS: u64 = 75;

                loop {
                    let event = tokio::select! {
                        _ = cancel_token.cancelled() => {
                            // Flush remaining buffer before cancellation
                            if !text_buffer.is_empty() {
                                hub.broadcast("chat_stream", serde_json::json!({
                                    "session_id": sid,
                                    "content": &text_buffer,
                                    "role_id": role_id,
                                }));
                            }
                            hub.broadcast("chat_cancelled", serde_json::json!({
                                "session_id": sid,
                                "role_id": role_id,
                            }));
                            break;
                        }
                        ev = rx.recv() => match ev {
                            Some(e) => e,
                            None => break,
                        }
                    };

                    match event.event_type {
                        StreamEventType::Text => {
                            full_response.push_str(&event.text);
                            text_buffer.push_str(&event.text);
                            if last_flush.elapsed().as_millis() as u64 >= COALESCE_MS {
                                hub.broadcast("chat_stream", serde_json::json!({
                                    "session_id": sid,
                                    "content": &text_buffer,
                                    "role_id": role_id,
                                }));
                                text_buffer.clear();
                                last_flush = tokio::time::Instant::now();
                            }
                        }
                        StreamEventType::Thinking => {
                            hub.broadcast("thinking", serde_json::json!({
                                "session_id": sid,
                                "content": event.text,
                                "role_id": role_id,
                            }));
                        }
                        StreamEventType::ToolCall => {
                            if let Some(ref tc) = event.tool_call {
                                hub.broadcast("tool_start", serde_json::json!({
                                    "session_id": sid,
                                    "tool_id": tc.id,
                                    "tool": tc.name,
                                    "input": tc.input,
                                    "role_id": role_id,
                                }));
                            }
                        }
                        StreamEventType::ToolResult => {
                            let tool_name = event.tool_call.as_ref()
                                .map(|tc| tc.name.as_str()).unwrap_or("");
                            let tool_id = event.tool_call.as_ref()
                                .map(|tc| tc.id.as_str()).unwrap_or("");
                            hub.broadcast("tool_result", serde_json::json!({
                                "session_id": sid,
                                "tool_id": tool_id,
                                "tool_name": tool_name,
                                "result": event.text,
                                "is_error": event.error.is_some(),
                                "role_id": role_id,
                            }));
                        }
                        StreamEventType::Error => {
                            hub.broadcast("chat_error", serde_json::json!({
                                "session_id": sid,
                                "error": event.error.unwrap_or_default(),
                                "role_id": role_id,
                            }));
                        }
                        StreamEventType::Usage => {
                            if let Some(ref usage) = event.usage {
                                hub.broadcast("usage", serde_json::json!({
                                    "session_id": sid,
                                    "input_tokens": usage.input_tokens,
                                    "output_tokens": usage.output_tokens,
                                }));
                            }
                        }
                        StreamEventType::ApprovalRequest => {
                            if let Some(ref tc) = event.tool_call {
                                hub.broadcast("approval_request", serde_json::json!({
                                    "session_id": sid,
                                    "request_id": tc.id,
                                    "tool": tc.name,
                                    "input": tc.input,
                                }));
                            }
                        }
                        StreamEventType::AskRequest => {
                            let request_id = event.error.as_deref().unwrap_or("");
                            let mut payload = serde_json::json!({
                                "session_id": sid,
                                "request_id": request_id,
                                "prompt": event.text,
                            });
                            if let Some(widgets) = &event.widgets {
                                payload["widgets"] = widgets.clone();
                            }
                            hub.broadcast("ask_request", payload);
                        }
                        StreamEventType::RateLimit => {
                            if let Some(ref rl) = event.rate_limit {
                                if rl.session_limit_tokens.is_some()
                                    || rl.weekly_limit_tokens.is_some()
                                {
                                    let usage = crate::state::JanusUsage {
                                        session_limit_tokens: rl
                                            .session_limit_tokens
                                            .unwrap_or(0),
                                        session_remaining_tokens: rl
                                            .session_remaining_tokens
                                            .unwrap_or(0),
                                        session_reset_at: rl
                                            .session_reset_at
                                            .clone()
                                            .unwrap_or_default(),
                                        weekly_limit_tokens: rl
                                            .weekly_limit_tokens
                                            .unwrap_or(0),
                                        weekly_remaining_tokens: rl
                                            .weekly_remaining_tokens
                                            .unwrap_or(0),
                                        weekly_reset_at: rl
                                            .weekly_reset_at
                                            .clone()
                                            .unwrap_or_default(),
                                    };
                                    *janus_usage.write().await = Some(usage);
                                }
                            }
                        }
                        StreamEventType::Done => {}
                    }
                }

                // Flush any remaining coalesced text
                if !text_buffer.is_empty() {
                    hub.broadcast("chat_stream", serde_json::json!({
                        "session_id": sid,
                        "content": &text_buffer,
                        "role_id": role_id,
                    }));
                }

                // Send comm reply if configured
                if let Some(reply_config) = &comm_reply {
                    if let Some(comm_mgr) = &comm_manager {
                        if !full_response.is_empty() {
                            let reply = comm::CommMessage {
                                id: uuid::Uuid::new_v4().to_string(),
                                from: String::new(),
                                to: String::new(),
                                topic: reply_config.topic.clone(),
                                conversation_id: reply_config.conversation_id.clone(),
                                msg_type: comm::CommMessageType::Message,
                                content: full_response,
                                metadata: std::collections::HashMap::new(),
                                timestamp: 0,
                                human_injected: false,
                                human_id: None,
                                task_id: None,
                                correlation_id: None,
                                task_status: None,
                                artifacts: vec![],
                                error: None,
                            };
                            if let Err(e) = comm_mgr.send(reply).await {
                                warn!(error = %e, "failed to send comm reply");
                            }
                        }
                    }
                }

                // Always send chat_complete
                hub.broadcast("chat_complete", serde_json::json!({
                    "session_id": sid,
                    "role_id": role_id,
                }));
            }
            Err(e) => {
                warn!(error = %e, "agent run failed");
                hub.broadcast("chat_error", serde_json::json!({
                    "session_id": sid,
                    "error": e.to_string(),
                    "role_id": role_id,
                }));
                hub.broadcast("chat_complete", serde_json::json!({
                    "session_id": sid,
                    "role_id": role_id,
                }));
            }
        }

        // ActiveRunGuard handles cleanup on drop (including panics)
        drop(_run_guard);

        Ok(())
    });

    state.lanes.enqueue_async(&lane, lane_task);
}
