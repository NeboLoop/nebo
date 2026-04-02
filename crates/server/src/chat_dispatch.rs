//! Unified chat dispatch — ONE way to run any chat (primary, role, channel, comm).
//!
//! Every chat entry point (WebSocket, REST, NeboLoop) builds a [`ChatConfig`] with
//! the appropriate decorators and calls [`run_chat`]. The underlying lane infrastructure,
//! event streaming, and response handling are identical for all chat types.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

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
    pub agent_id: String,
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
    let presence_tracker = state.presence.clone();
    let proactive_inbox = state.proactive_inbox.clone();
    let comm_manager = if config.comm_reply.is_some() {
        Some(state.comm_manager.clone())
    } else {
        None
    };

    // Resolve agent display name for outbound comm messages
    let agent_display_name = if !config.agent_id.is_empty() {
        let registry = state.agent_registry.read().await;
        registry.get(&config.agent_id).map(|r| r.name.clone()).unwrap_or_default()
    } else {
        state.store.get_agent_profile().ok().flatten()
            .map(|p| p.name)
            .unwrap_or_default()
    };

    let sid = config.session_key.clone();
    let agent_id = config.agent_id.clone();
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
            "agentId": agent_id,
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
            agent_id: agent_id.clone(),
            permissions,
            resource_grants,
            model_preference,
            personality_snippet,
            images,
            allowed_paths,
            presence_tracker: Some(presence_tracker.clone()),
            proactive_inbox: Some(proactive_inbox.clone()),
            ..Default::default()
        };

        match runner.run(req).await {
            Ok(mut rx) => {
                let mut full_response = String::new();
                let mut text_buffer = String::new();
                let mut last_flush = tokio::time::Instant::now();
                const COALESCE_MS: u64 = 75;

                // Comm streaming: send chunks to NeboLoop as they arrive
                let mut comm_buffer = String::new();
                let mut last_comm_flush = tokio::time::Instant::now();
                let mut comm_streamed = false;
                const COMM_COALESCE_MS: u64 = 500;

                loop {
                    let event = tokio::select! {
                        _ = cancel_token.cancelled() => {
                            // Flush remaining buffer before cancellation
                            if !text_buffer.is_empty() {
                                hub.broadcast("chat_stream", serde_json::json!({
                                    "session_id": sid,
                                    "content": &text_buffer,
                                    "agentId": agent_id,
                                }));
                                text_buffer.clear();
                            }
                            hub.broadcast("chat_cancelled", serde_json::json!({
                                "session_id": sid,
                                "agentId": agent_id,
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
                                    "agentId": agent_id,
                                }));
                                text_buffer.clear();
                                last_flush = tokio::time::Instant::now();
                            }
                            // Stream chunks to NeboLoop comm channel
                            if comm_reply.is_some() {
                                comm_buffer.push_str(&event.text);
                                if last_comm_flush.elapsed().as_millis() as u64 >= COMM_COALESCE_MS {
                                    if let (Some(cfg), Some(mgr)) = (&comm_reply, &comm_manager) {
                                        let mut chunk_meta = std::collections::HashMap::new();
                                        if !agent_display_name.is_empty() {
                                            chunk_meta.insert("senderName".to_string(), agent_display_name.clone());
                                        }
                                        let chunk = comm::CommMessage {
                                            id: uuid::Uuid::new_v4().to_string(),
                                            from: String::new(),
                                            to: String::new(),
                                            topic: cfg.topic.clone(),
                                            conversation_id: cfg.conversation_id.clone(),
                                            msg_type: comm::CommMessageType::Stream,
                                            content: comm_buffer.clone(),
                                            metadata: chunk_meta,
                                            timestamp: 0,
                                            human_injected: false,
                                            human_id: None,
                                            task_id: None,
                                            correlation_id: None,
                                            task_status: None,
                                            artifacts: vec![],
                                            error: None,
                                        };
                                        if let Err(e) = mgr.send(chunk).await {
                                            warn!(error = %e, "failed to send comm stream chunk");
                                        }
                                        comm_streamed = true;
                                        comm_buffer.clear();
                                        last_comm_flush = tokio::time::Instant::now();
                                    }
                                }
                            }
                        }
                        StreamEventType::Thinking => {
                            hub.broadcast("thinking", serde_json::json!({
                                "session_id": sid,
                                "content": event.text,
                                "agentId": agent_id,
                            }));
                        }
                        StreamEventType::ToolCall => {
                            // Flush pending text before tool event to prevent fragmentation
                            if !text_buffer.is_empty() {
                                hub.broadcast("chat_stream", serde_json::json!({
                                    "session_id": sid,
                                    "content": &text_buffer,
                                    "agentId": agent_id,
                                }));
                                text_buffer.clear();
                                last_flush = tokio::time::Instant::now();
                            }
                            if let Some(ref tc) = event.tool_call {
                                hub.broadcast("tool_start", serde_json::json!({
                                    "session_id": sid,
                                    "tool_id": tc.id,
                                    "tool": tc.name,
                                    "input": tc.input,
                                    "agentId": agent_id,
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
                                "agentId": agent_id,
                            }));
                        }
                        StreamEventType::Error => {
                            hub.broadcast("chat_error", serde_json::json!({
                                "session_id": sid,
                                "error": event.error.unwrap_or_default(),
                                "agentId": agent_id,
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
                                        updated_at: chrono::Utc::now().to_rfc3339(),
                                    };
                                    *janus_usage.write().await = Some(usage);
                                }
                            }
                            // If the runner forwarded a quota warning (text is non-empty),
                            // broadcast to the frontend so it can show a warning banner.
                            if !event.text.is_empty() {
                                hub.broadcast("quota_warning", serde_json::json!({
                                    "session_id": sid,
                                    "message": event.text,
                                    "agentId": agent_id,
                                }));
                            }
                        }
                        StreamEventType::SubagentStart => {
                            let mut payload = serde_json::json!({
                                "session_id": sid,
                                "agentId": agent_id,
                            });
                            if let Some(ref w) = event.widgets {
                                for (k, v) in w.as_object().into_iter().flatten() {
                                    payload[k] = v.clone();
                                }
                            }
                            hub.broadcast("subagent_start", payload);
                        }
                        StreamEventType::SubagentProgress => {
                            let mut payload = serde_json::json!({
                                "session_id": sid,
                                "agentId": agent_id,
                            });
                            if let Some(ref w) = event.widgets {
                                for (k, v) in w.as_object().into_iter().flatten() {
                                    payload[k] = v.clone();
                                }
                            }
                            hub.broadcast("subagent_progress", payload);
                        }
                        StreamEventType::SubagentComplete => {
                            let mut payload = serde_json::json!({
                                "session_id": sid,
                                "agentId": agent_id,
                            });
                            if let Some(ref w) = event.widgets {
                                for (k, v) in w.as_object().into_iter().flatten() {
                                    payload[k] = v.clone();
                                }
                            }
                            hub.broadcast("subagent_complete", payload);
                        }
                        StreamEventType::Done => {}
                    }
                }

                // Flush any remaining coalesced text
                if !text_buffer.is_empty() {
                    hub.broadcast("chat_stream", serde_json::json!({
                        "session_id": sid,
                        "content": &text_buffer,
                        "agentId": agent_id,
                    }));
                }

                // Send final comm reply — flush remaining stream buffer + complete message
                if let Some(reply_config) = &comm_reply {
                    if let Some(comm_mgr) = &comm_manager {
                        if !full_response.is_empty() {
                            // Build metadata with agent name for all outbound messages
                            let mut reply_meta = std::collections::HashMap::new();
                            if !agent_display_name.is_empty() {
                                reply_meta.insert("senderName".to_string(), agent_display_name.clone());
                            }

                            // Flush any remaining streamed text
                            if !comm_buffer.is_empty() {
                                let chunk = comm::CommMessage {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    from: String::new(),
                                    to: String::new(),
                                    topic: reply_config.topic.clone(),
                                    conversation_id: reply_config.conversation_id.clone(),
                                    msg_type: comm::CommMessageType::Stream,
                                    content: comm_buffer,
                                    metadata: reply_meta.clone(),
                                    timestamp: 0,
                                    human_injected: false,
                                    human_id: None,
                                    task_id: None,
                                    correlation_id: None,
                                    task_status: None,
                                    artifacts: vec![],
                                    error: None,
                                };
                                if let Err(e) = comm_mgr.send(chunk).await {
                                    warn!(error = %e, "failed to send comm stream flush");
                                }
                            }

                            // If we streamed chunks, don't send the full response again
                            // (it would appear as a duplicate message in the Loop).
                            // Only send the complete message if no chunks were streamed
                            // (e.g., very short responses that finished before the first flush).
                            if !comm_streamed {
                                info!(
                                    topic = %reply_config.topic,
                                    conv_id = %reply_config.conversation_id,
                                    response_len = full_response.len(),
                                    "sending comm reply (complete, no stream)"
                                );
                                let reply = comm::CommMessage {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    from: String::new(),
                                    to: String::new(),
                                    topic: reply_config.topic.clone(),
                                    conversation_id: reply_config.conversation_id.clone(),
                                    msg_type: comm::CommMessageType::Message,
                                    content: full_response,
                                    metadata: reply_meta,
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
                        } else {
                            warn!(
                                topic = %reply_config.topic,
                                conv_id = %reply_config.conversation_id,
                                "comm reply skipped: empty response from agent"
                            );
                        }
                    }
                }

                // Always send chat_complete
                hub.broadcast("chat_complete", serde_json::json!({
                    "session_id": sid,
                    "agentId": agent_id,
                }));
            }
            Err(e) => {
                warn!(error = %e, "agent run failed");
                hub.broadcast("chat_error", serde_json::json!({
                    "session_id": sid,
                    "error": e.to_string(),
                    "agentId": agent_id,
                }));
                hub.broadcast("chat_complete", serde_json::json!({
                    "session_id": sid,
                    "agentId": agent_id,
                }));
            }
        }

        // ActiveRunGuard handles cleanup on drop (including panics)
        drop(_run_guard);

        Ok(())
    });

    state.lanes.enqueue_async(&lane, lane_task);
}
