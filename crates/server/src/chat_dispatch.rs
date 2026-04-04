//! Unified chat dispatch — ONE way to run any chat (primary, role, channel, comm).
//!
//! Every chat entry point (WebSocket, REST, NeboLoop, cron, heartbeat) builds a
//! [`ChatConfig`] with the appropriate decorators and calls [`run_chat`]. The
//! underlying lane infrastructure, event streaming, and response handling are
//! identical for all chat types.
//!
//! All runs register in the global [`RunRegistry`](crate::run_registry::RunRegistry)
//! for visibility, cancellation, and progress tracking.

use std::collections::HashMap;
use std::sync::Arc;

use tracing::{info, warn};

use agent::lanes::make_task;
use agent::RunRequest;
use ai::StreamEventType;
use tools::Origin;

use crate::run_registry::RegisterParams;
use crate::state::AppState;

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
    pub cancel_token: tokio_util::sync::CancellationToken,
    /// Which lane to enqueue on (e.g., lanes::MAIN, lanes::COMM).
    pub lane: String,
    /// If set, sends the accumulated text response back via comm after completion.
    pub comm_reply: Option<CommReplyConfig>,
    /// Per-entity resolved config — permissions, resource grants, model, personality.
    pub entity_config: Option<crate::entity_config::ResolvedEntityConfig>,
    /// Images attached to the user's message (base64-encoded).
    pub images: Vec<ai::ImageContent>,
    /// Display name for the entity (agent name or "Nebo"). Used in RunRegistry.
    pub entity_name: String,
}

/// Configuration for sending a reply back through a communication channel.
#[derive(Clone)]
pub struct CommReplyConfig {
    pub provider: String, // "neboloop", or future: "slack", "discord"
    pub topic: String,
    pub conversation_id: String,
}

/// Single entry point for all chat dispatch.
///
/// Callers configure behavior via [`ChatConfig`] decorators. Every run is
/// automatically registered in the global [`RunRegistry`] for visibility and
/// cancellation — no opt-in required.
pub async fn run_chat(state: &AppState, config: ChatConfig) {
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
    let channel_providers = if config.comm_reply.is_some() {
        Some(state.channel_providers.clone())
    } else {
        None
    };

    // Resolve agent display name for outbound comm messages
    let agent_display_name = if !config.entity_name.is_empty() {
        config.entity_name.clone()
    } else if !config.agent_id.is_empty() {
        let registry = state.agent_registry.read().await;
        registry.get(&config.agent_id).map(|r| r.name.clone()).unwrap_or_default()
    } else {
        state.store.get_agent_profile().ok().flatten()
            .map(|p| p.name)
            .unwrap_or_else(|| "Nebo".to_string())
    };

    let sid = config.session_key.clone();
    let agent_id = config.agent_id.clone();
    let cancel_token = config.cancel_token.clone();
    let lane = config.lane.clone();

    // Determine entity_id for the registry
    let entity_id = if !agent_id.is_empty() {
        agent_id.clone()
    } else {
        "main".to_string()
    };
    let origin_label = format!("{:?}", config.origin).to_lowercase();

    // Register in the global RunRegistry — ALL run paths go through here.
    let run_handle = state.run_registry.register(RegisterParams {
        session_key: sid.clone(),
        entity_id,
        entity_name: agent_display_name.clone(),
        origin: origin_label,
        channel: config.channel.clone(),
        cancel_token: cancel_token.clone(),
        parent_run_id: None,
    }).await;

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
        // RunHandle auto-unregisters from RunRegistry on drop (panic-safe).
        let _run_handle = run_handle;

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

        // Build progress tracker from RunHandle's shared Arcs
        let progress = agent::RunProgress {
            run_id: _run_handle.run_id.clone(),
            iteration_count: _run_handle.iteration_count.clone(),
            tool_call_count: _run_handle.tool_call_count.clone(),
            current_tool: _run_handle.current_tool.clone(),
        };

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
            progress: Some(progress),
            ..Default::default()
        };

        match runner.run(req).await {
            Ok(mut rx) => {
                let mut full_response = String::new();
                let mut text_buffer = String::new();
                let mut last_flush = tokio::time::Instant::now();
                const COALESCE_MS: u64 = 75;

                // Comm streaming: send chunks to NeboLoop as they arrive.
                // Timer starts on first token, not loop init (LLM latency would
                // cause the first token to flush immediately otherwise).
                let mut comm_buffer = String::new();
                let mut last_comm_flush: Option<tokio::time::Instant> = None;
                let mut comm_streamed = false;
                const COMM_COALESCE_MS: u64 = 500;
                // Stable ID across all stream chunks so the gateway coalesces them
                let comm_stream_id = uuid::Uuid::new_v4().to_string();

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

                    // Refresh activity timestamp so stale-run cleanup doesn't kill us
                    _run_handle.touch();

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
                            // Stream chunks to comm channel via provider
                            if comm_reply.is_some() {
                                comm_buffer.push_str(&event.text);
                                let flush_elapsed = last_comm_flush
                                    .get_or_insert_with(tokio::time::Instant::now)
                                    .elapsed()
                                    .as_millis() as u64;
                                if flush_elapsed >= COMM_COALESCE_MS {
                                    if let Some(cfg) = &comm_reply {
                                        let mut chunk_meta = std::collections::HashMap::new();
                                        if !agent_display_name.is_empty() {
                                            chunk_meta.insert("senderName".to_string(), agent_display_name.clone());
                                        }
                                        let chunk = comm::CommMessage {
                                            id: comm_stream_id.clone(),
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
                                        if let Err(e) = send_to_channel(&cfg.provider, &comm_manager, &channel_providers, chunk).await {
                                            warn!(error = %e, "failed to send comm stream chunk");
                                        }
                                        comm_streamed = true;
                                        comm_buffer.clear();
                                        last_comm_flush = Some(tokio::time::Instant::now());
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
                                if rl.session_limit_credits.is_some()
                                    || rl.weekly_limit_credits.is_some()
                                {
                                    let usage = crate::state::JanusUsage {
                                        session_limit_credits: rl
                                            .session_limit_credits
                                            .unwrap_or(0),
                                        session_remaining_credits: rl
                                            .session_remaining_credits
                                            .unwrap_or(0),
                                        session_reset_at: rl
                                            .session_reset_at
                                            .clone()
                                            .unwrap_or_default(),
                                        weekly_limit_credits: rl
                                            .weekly_limit_credits
                                            .unwrap_or(0),
                                        weekly_remaining_credits: rl
                                            .weekly_remaining_credits
                                            .unwrap_or(0),
                                        weekly_reset_at: rl
                                            .weekly_reset_at
                                            .clone()
                                            .unwrap_or_default(),
                                        budget_free_available: rl
                                            .budget_free_available
                                            .unwrap_or(0),
                                        budget_gift_available: rl
                                            .budget_gift_available
                                            .unwrap_or(0),
                                        budget_credits_cents: rl
                                            .budget_credits_cents
                                            .unwrap_or(0),
                                        budget_active_pool: rl
                                            .budget_active_pool
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
                    if !full_response.is_empty() {
                        // Build metadata with agent name for all outbound messages
                        let mut reply_meta = std::collections::HashMap::new();
                        if !agent_display_name.is_empty() {
                            reply_meta.insert("senderName".to_string(), agent_display_name.clone());
                        }

                        // Flush any remaining streamed text
                        if !comm_buffer.is_empty() {
                            let chunk = comm::CommMessage {
                                id: comm_stream_id.clone(),
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
                            if let Err(e) = send_to_channel(&reply_config.provider, &comm_manager, &channel_providers, chunk).await {
                                warn!(error = %e, "failed to send comm stream flush");
                            }
                            comm_streamed = true;
                        }

                        // If we streamed any chunks (during loop or final flush), don't
                        // send the full response again — it would duplicate on the Loop.
                        // Only send a complete Message if nothing was streamed at all.
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
                            if let Err(e) = send_to_channel(&reply_config.provider, &comm_manager, &channel_providers, reply).await {
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

        // RunHandle unregisters from RunRegistry on drop (including panics)
        drop(_run_handle);

        Ok(())
    });

    state.lanes.enqueue_async(&lane, lane_task);
}

/// Send a comm message through the appropriate channel provider.
/// Fast path for "neboloop" uses the comm_manager directly.
/// Other providers are looked up in the channel_providers registry.
async fn send_to_channel(
    provider: &str,
    comm_manager: &Option<Arc<comm::PluginManager>>,
    channel_providers: &Option<Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn comm::ChannelProvider>>>>>,
    msg: comm::CommMessage,
) -> Result<(), comm::CommError> {
    if provider == "neboloop" {
        if let Some(ref mgr) = *comm_manager {
            return mgr.send(msg).await;
        }
        return Err(comm::CommError::NoActivePlugin);
    }
    if let Some(ref providers_lock) = *channel_providers {
        let providers: tokio::sync::RwLockReadGuard<'_, HashMap<String, Arc<dyn comm::ChannelProvider>>> =
            providers_lock.read().await;
        if let Some(p) = providers.get(provider) {
            return p.send_response(msg).await;
        }
    }
    Err(comm::CommError::Other(format!("unknown channel provider: {}", provider)))
}
