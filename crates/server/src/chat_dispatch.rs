//! Unified chat dispatch — ONE way to run any chat (primary, role, channel, comm).
//!
//! Every chat entry point (WebSocket, REST, NeboAI, cron, heartbeat) builds a
//! [`ChatConfig`] with the appropriate decorators and calls [`run_chat`]. The
//! underlying lane infrastructure, event streaming, and response handling are
//! identical for all chat types.
//!
//! All runs register in the global [`RunRegistry`](crate::run_registry::RunRegistry)
//! for visibility, cancellation, and progress tracking.

use std::collections::HashMap;
use std::sync::Arc;

use tracing::{info, warn};

use agent::RunRequest;
use agent::lanes::make_task;
use ai::StreamEventType;
use tokio::sync::mpsc;
use tools::Origin;

use crate::run_registry::RegisterParams;
use crate::state::AppState;

/// Convert markdown to HTML using pulldown-cmark.
/// Used for assistant messages so the frontend can render with `{@html}`.
pub fn md_to_html(md: &str) -> String {
    use pulldown_cmark::{Options, Parser, html};
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(md, opts);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

fn tool_activity_label(tool_name: &str) -> Option<&'static str> {
    Some(match tool_name {
        "bash"    => "running a command",
        "grep"    => "searching files",
        "glob"    => "finding files",
        "read"    => "reading a file",
        "write"   => "writing a file",
        "edit"    => "editing a file",

        "web"     => "searching the web",
        "browser" => "reading a page",
        "bot"     => "thinking it through",
        "desktop" => "using the desktop",
        "event"   => "checking the schedule",
        "loop"    => "sending a message",

        "os"      => "checking the workspace",
        _         => return None,
    })
}

/// Past-tense counterpart, sent on the result phase: collapsed work lines
/// report OUTCOMES ("Ran a command"), not in-progress activity. One source
/// for every client — web and mobile render these verbatim.
fn tool_outcome_label(tool_name: &str) -> Option<&'static str> {
    Some(match tool_name {
        "bash"    => "Ran a command",
        "grep"    => "Searched files",
        "glob"    => "Found files",
        "read"    => "Read a file",
        "write"   => "Wrote a file",
        "edit"    => "Edited a file",

        "web"     => "Searched the web",
        "browser" => "Read a page",
        "bot"     => "Thought it through",
        "desktop" => "Used the desktop",
        "event"   => "Checked the schedule",
        "loop"    => "Sent a message",

        "os"      => "Checked the workspace",
        _         => return None,
    })
}

/// Honest fallback for a tool we don't have nice copy for: name it as-is
/// ("using tool_search" / "Used tool_search") rather than vague filler.
fn humanize_tool_name(tool_name: &str) -> (String, String) {
    let n = tool_name.replace('_', " ");
    (format!("using {n}"), format!("Used {n}"))
}

/// Verb forms for STRAP actions: (gerund for the live activity label,
/// past tense for the outcome label).
fn strap_verb(action: &str) -> Option<(&'static str, &'static str)> {
    Some(match action {
        "create" | "add" | "insert" => ("creating", "Created"),
        "read" | "get" | "view" | "fetch" => ("reading", "Read"),
        "list" | "ls" => ("listing", "Listed"),
        "search" | "find" | "query" | "glob" | "grep" => ("searching", "Searched"),
        "update" | "edit" | "set" | "patch" | "rename" | "move" => ("updating", "Updated"),
        "delete" | "remove" | "clear" => ("deleting", "Deleted"),
        "send" | "post" | "reply" | "dm" => ("sending", "Sent"),
        "run" | "exec" | "execute" | "shell" => ("running", "Ran"),
        "write" | "save" => ("writing", "Wrote"),
        "download" => ("downloading", "Downloaded"),
        "upload" => ("uploading", "Uploaded"),
        "open" | "launch" | "start" => ("opening", "Opened"),
        "stop" | "close" | "kill" => ("stopping", "Stopped"),
        "check" | "status" | "verify" => ("checking", "Checked"),
        _ => return None,
    })
}

/// Humanize a tool call from its STRAP signature — `os(resource: file,
/// action: read)` reads as "reading a file" / "Read a file", which says far
/// more than the bare domain-tool name ("os"). MCP tools (`mcp__slug__tool`)
/// humanize from their slug + tool name. Falls back to the name-only maps.
/// Returns (activity gerund phrase, past-tense outcome).
fn humanize_tool_call(tool_name: &str, input: &serde_json::Value) -> (String, String) {
    // MCP: mcp__github__create_issue → "using GitHub (create issue)".
    if let Some(rest) = tool_name.strip_prefix("mcp__") {
        if let Some((slug, tool)) = rest.split_once("__") {
            let tool_h = tool.replace('_', " ");
            return (
                format!("using {slug} ({tool_h})"),
                format!("Used {slug}: {tool_h}"),
            );
        }
    }
    // STRAP: toolName(resource, action, …).
    let resource = input.get("resource").and_then(|v| v.as_str());
    let action = input.get("action").and_then(|v| v.as_str());
    if let (Some(resource), Some(action)) = (resource, action) {
        let noun = resource.replace('_', " ");
        if let Some((gerund, past)) = strap_verb(action) {
            return (format!("{gerund} {noun}"), format!("{past} {noun}"));
        }
        // Unknown verb: show the signature honestly rather than guessing.
        return (
            format!("running {action} on {noun}"),
            format!("Ran {action} on {noun}"),
        );
    }
    match (tool_activity_label(tool_name), tool_outcome_label(tool_name)) {
        (Some(a), Some(o)) => (a.to_string(), o.to_string()),
        // Unknown tool (e.g. tool_search, a skill, a delegate) — name it
        // honestly instead of "working" / "Did a step".
        _ => humanize_tool_name(tool_name),
    }
}

/// True when a streamed text chunk is an orchestrator progress heartbeat —
/// the transient `"\n_Working on: ..._\n"` / `"\n_Working..._\n"` status the
/// sub-agent runner emits every 30s (`orchestrator.rs`). It's a "still alive"
/// signal for the live activity indicator, never real content, so it must not
/// land in a comm reply or a channel's final response.
pub(crate) fn is_progress_heartbeat(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with("_Working") && trimmed.ends_with('_')
}

/// Remove orchestrator progress-heartbeat lines from a finalized response so
/// the noise never appears in the message that replaces the streamed bubble.
/// Operates line-wise; all non-heartbeat content (including blank lines) is
/// preserved.
pub(crate) fn strip_progress_heartbeats(s: &str) -> String {
    s.lines()
        .filter(|line| !is_progress_heartbeat(line))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
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
    /// For @mention routing: the agent that originated the mention.
    /// When set, all WS broadcast payloads include "originAgentId" so the
    /// frontend can route delegate events back to the originating thread.
    pub origin_agent_id: Option<String>,
    /// Injected as a system-role message after the user prompt — visible to the
    /// LLM but not rendered in the frontend. Used for @mention routing context.
    pub mention_context: Option<String>,
    /// Tool scope name from agent.json. Restricts sidecar tools/skills/plugins
    /// to those declared in the named scope.
    pub tool_scope: Option<String>,
    /// When true, agent presents a plan before executing tool calls (Plan Mode).
    pub plan_mode: bool,
    /// Channel context (Slack/Discord/etc.) when this run was triggered by an
    /// inbound channel message. Propagated to `ToolContext.channel` so plugin
    /// uploads target the right destination. None for web UI / scheduled runs.
    pub channel_ctx: Option<tools::ChannelContext>,
}

/// Configuration for sending a reply back through a communication channel.
#[derive(Clone)]
pub struct CommReplyConfig {
    pub provider: String, // "neboai", or future: "slack", "discord"
    pub topic: String,
    pub conversation_id: String,
}

/// Single entry point for all chat dispatch.
///
/// Callers configure behavior via [`ChatConfig`] decorators. Every run is
/// automatically registered in the global [`RunRegistry`] for visibility and
/// cancellation — no opt-in required.
/// Shared dispatch preamble for both run entrypoints — `run_chat` (broadcast
/// sink) and `run_chat_events` (returned-channel sink). Resolves the agent
/// display name, derives the entity id + origin label, and registers the run in
/// the global RunRegistry. This identical setup lived in both functions and had
/// started to drift; keeping it here is the one canonical path (CODE_AUDITOR
/// Rule 8). Returns the display name (for outbound comm) and the run handle.
async fn register_run(
    state: &AppState,
    config: &ChatConfig,
) -> (String, crate::run_registry::RunHandle) {
    let agent_display_name = if !config.entity_name.is_empty() {
        config.entity_name.clone()
    } else if !config.agent_id.is_empty() {
        let registry = state.agent_registry.read().await;
        registry
            .get(&config.agent_id)
            .map(|r| r.name.clone())
            .unwrap_or_default()
    } else {
        state
            .store
            .get_agent_profile()
            .ok()
            .flatten()
            .map(|p| p.name)
            .unwrap_or_else(|| "Nebo".to_string())
    };

    let entity_id = if !config.agent_id.is_empty() {
        config.agent_id.clone()
    } else {
        "main".to_string()
    };
    let origin_label = format!("{:?}", config.origin).to_lowercase();

    let run_handle = state
        .run_registry
        .register(RegisterParams {
            session_key: config.session_key.clone(),
            entity_id,
            entity_name: agent_display_name.clone(),
            origin: origin_label,
            channel: config.channel.clone(),
            cancel_token: config.cancel_token.clone(),
            parent_run_id: None,
        })
        .await;

    (agent_display_name, run_handle)
}

pub async fn run_chat(state: &AppState, config: ChatConfig) {
    let hub = state.hub.clone();
    let runner = state.runner.clone();
    let janus_usage = state.janus_usage.clone();
    let presence_tracker = state.presence.clone();
    let proactive_inbox = state.proactive_inbox.clone();
    let cleanup_tools = state.tools.clone();
    let plugin_store = state.plugin_store.clone();
    let pending_comm_asks = state.pending_comm_asks.clone();
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

    let sid = config.session_key.clone();
    let agent_id = config.agent_id.clone();
    let cancel_token = config.cancel_token.clone();
    let lane = config.lane.clone();
    // Owned AppState clone moved into the run task so the background title
    // generator can propagate the generated title to the loop (chats/sync).
    let task_state = state.clone();

    // Resolve display name + register the run (shared with run_chat_events).
    let (agent_display_name, run_handle) = register_run(state, &config).await;

    // Destructure config fields before moving into closure
    let prompt = config.prompt;
    let system = config.system;
    let user_id = config.user_id;
    let channel = config.channel;
    let origin = config.origin;
    let comm_reply = config.comm_reply;
    let entity_cfg = config.entity_config;
    let images = config.images;
    let origin_agent_id = config.origin_agent_id;
    let mention_context = config.mention_context;
    let tool_scope = config.tool_scope;
    let plan_mode = config.plan_mode;

    // Broadcast chat_created so frontend can track new conversations
    {
        let mut created_payload = serde_json::json!({
            "session_id": sid,
            "channel": channel,
            "agentId": agent_id,
        });
        if let Some(ref oid) = origin_agent_id {
            created_payload["originAgentId"] = serde_json::Value::String(oid.clone());
        }
        hub.broadcast("chat_created", created_payload);
    }

    let fairness_key = agent_id.clone();
    let mut lane_task = make_task(&lane, format!("chat:{}", sid), async move {
        // RunHandle auto-unregisters from RunRegistry on drop (panic-safe).
        let _run_handle = run_handle;

        // Send initial typing indicator for NeboLoop conversations
        if let Some(ref cm) = comm_manager {
            if let Some(ref cr) = comm_reply {
                let _ = cm.send_typing(&cr.conversation_id, true, None).await;
            }
        }

        // One id per user turn (this run): the frontend groups every stream
        // event carrying the same turn_id — text, tool, thinking — into a
        // single turn container with blocks finalized in place.
        let turn_id = uuid::Uuid::new_v4().to_string();

        // Helper: build a WS broadcast payload with session_id, agentId,
        // turn_id, and optional originAgentId (for @mention delegate routing).
        macro_rules! ws_payload {
            ($($key:tt : $val:expr),* $(,)?) => {{
                let mut v = serde_json::json!({
                    "session_id": sid,
                    "agentId": agent_id,
                    "turn_id": turn_id,
                    $($key: $val),*
                });
                if let Some(ref oid) = origin_agent_id {
                    v["originAgentId"] = serde_json::Value::String(oid.clone());
                }
                v
            }};
        }

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
        let allowed_paths = entity_cfg
            .as_ref()
            .map(|ec| ec.allowed_paths.clone())
            .unwrap_or_default();

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
            mention_context,
            tool_scope,
            plan_mode,
            // run_chat generates the chat title itself (broadcasts + pushes it
            // to the loop), so skip the runner-side generator to avoid the race.
            skip_title_gen: true,
            ..Default::default()
        };

        match runner.run(req).await {
            Ok(mut rx) => {
                let mut full_response = String::new();
                let mut text_buffer = String::new();
                let mut last_flush = tokio::time::Instant::now();
                // Tight coalesce window so text streams in small, token-smooth chunks
                // (ChatGPT/Claude feel) rather than arriving a sentence at a time. Applies
                // to both the local app and the loop/comm channel so they stream identically.
                const COALESCE_MS: u64 = 25;

                // Comm streaming: send chunks to NeboAI as they arrive.
                // Timer starts on first token, not loop init (LLM latency would
                // cause the first token to flush immediately otherwise).
                let mut comm_buffer = String::new();
                let mut last_comm_flush: Option<tokio::time::Instant> = None;
                let mut comm_streamed = false;
                let mut needs_separator = false;
                // File artifacts produced by tools during this run, collected from
                // ToolResult events. Only populated for comm replies; attached to the
                // final loop/DM reply so generated files (images, reports) reach the
                // channel Slack-style. Deduped by source URL/path.
                let mut comm_file_artifacts: Vec<String> = Vec::new();
                // Run-produced media (images/files) referenced as /api/v1/files/<name>
                // URLs, streamed to the LOCAL app on chat_complete so it renders inline.
                let mut app_file_artifacts: Vec<String> = Vec::new();
                // Match the local-chat streaming cadence (COALESCE_MS) so loop
                // replies stream token-smooth like ChatGPT/Claude instead of
                // arriving in half-second chunks. The chunks are ephemeral
                // fanout frames, so a tight window is cheap.
                const COMM_COALESCE_MS: u64 = COALESCE_MS;
                // Per-segment stream id. A "segment" is the prose between two tool
                // rounds. Each segment is flushed as its OWN persisted Message
                // (not an empty final — the gateway doesn't persist Stream frames,
                // so an empty final loses the reply on reload). tool_activity for a
                // round is tagged with the NEXT segment's id, so the client renders
                // "Used N tools" above the prose that follows it — interleaved,
                // exactly like the desktop, and durable.
                let mut comm_stream_id = uuid::Uuid::new_v4().to_string();
                // Accumulated text of the CURRENT segment (cleared at each tool
                // round when the segment is flushed as a Message).
                let mut comm_segment = String::new();

                // Keepalive: refresh the remote "is typing…" indicator on a
                // timer, independent of stream-event flow. Without this, any
                // silent gap in the event stream lets the loop/DM typing signal
                // lapse so the remote appears stalled — most importantly during
                // a provider 502 being retried: the runner's transient/retryable
                // retries sleep 2s and emit NOTHING to the stream until they
                // succeed or exhaust, so a flapping gateway leaves the remote
                // dark for the whole window even though the run is still going
                // and will complete. The desktop is unaffected (it holds its
                // last rendered state across the gap). 4s stays well inside any
                // reasonable client-side typing TTL and bridges the 2s retry
                // steps. No-op when there's no comm reply (local-only chat).
                let mut comm_keepalive = tokio::time::interval(std::time::Duration::from_secs(4));
                comm_keepalive.tick().await; // consume the immediate first tick

                loop {
                    let event = tokio::select! {
                        _ = cancel_token.cancelled() => {
                            // Flush remaining buffer before cancellation
                            if !text_buffer.is_empty() {
                                hub.broadcast("chat_stream", ws_payload!(
                                    "content": &text_buffer,
                                ));
                                text_buffer.clear();
                            }
                            hub.broadcast("chat_cancelled", ws_payload!());
                            if let Some(ref cm) = comm_manager {
                                if let Some(ref cr) = comm_reply {
                                    let _ = cm.send_typing(&cr.conversation_id, false, None).await;
                                }
                            }
                            break;
                        }
                        _ = comm_keepalive.tick() => {
                            // Bridge silent gaps (e.g. a 502 being retried): keep
                            // the remote typing indicator alive and mark the run
                            // active so neither the indicator nor stale-run cleanup
                            // lapses while the run is mid-flight with no events.
                            _run_handle.touch();
                            if let Some(ref cm) = comm_manager {
                                if let Some(ref cr) = comm_reply {
                                    let _ = cm.send_typing(&cr.conversation_id, true, None).await;
                                }
                            }
                            continue;
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
                            if needs_separator
                                && !full_response.is_empty()
                                && !full_response.ends_with(|c: char| c.is_whitespace())
                                && !event.text.starts_with(|c: char| c.is_whitespace())
                            {
                                full_response.push_str("\n\n");
                                text_buffer.push_str("\n\n");
                                if comm_reply.is_some() {
                                    comm_buffer.push_str("\n\n");
                                }
                            }
                            needs_separator = false;
                            full_response.push_str(&event.text);
                            text_buffer.push_str(&event.text);
                            if last_flush.elapsed().as_millis() as u64 >= COALESCE_MS {
                                hub.broadcast(
                                    "chat_stream",
                                    ws_payload!(
                                        "content": &text_buffer,
                                    ),
                                );
                                text_buffer.clear();
                                last_flush = tokio::time::Instant::now();
                            }
                            // Stream chunks to comm channel via provider.
                            // Skip orchestrator progress heartbeats — the loop
                            // gets a live activity signal via send_typing()
                            // instead, without the "_Working on:_" spam.
                            if comm_reply.is_some() && !is_progress_heartbeat(&event.text) {
                                comm_buffer.push_str(&event.text);
                                // Accumulate the current segment's full text (the
                                // persisted Message body, flushed at the next tool
                                // round or turn end).
                                comm_segment.push_str(&event.text);
                                // Flush the FIRST chunk immediately so the loop
                                // paints the opening text the instant the model
                                // produces it (matches the local-chat feel);
                                // coalesce subsequent chunks at COMM_COALESCE_MS.
                                let should_flush = match last_comm_flush {
                                    None => true,
                                    Some(t) => t.elapsed().as_millis() as u64 >= COMM_COALESCE_MS,
                                };
                                if should_flush {
                                    if let Some(cfg) = &comm_reply {
                                        send_comm_msg(
                                            cfg,
                                            &comm_manager,
                                            &channel_providers,
                                            comm::CommMessageType::Stream,
                                            comm_stream_id.clone(),
                                            comm_buffer.clone(),
                                            std::collections::HashMap::new(),
                                            &agent_display_name,
                                        )
                                        .await;
                                        comm_streamed = true;
                                        comm_buffer.clear();
                                        last_comm_flush = Some(tokio::time::Instant::now());
                                    }
                                }
                            }
                        }
                        StreamEventType::Thinking => {
                            hub.broadcast(
                                "thinking",
                                ws_payload!(
                                    "content": event.text,
                                ),
                            );
                            if let Some(ref cm) = comm_manager {
                                if let Some(ref cr) = comm_reply {
                                    let _ = cm.send_typing(&cr.conversation_id, true, Some("thinking")).await;
                                }
                            }
                        }
                        StreamEventType::ToolCall => {
                            // Flush pending text (web + loop) before the tool event so the
                            // reply text shows BEFORE the tool runs, instead of sticking
                            // mid-word while a slow tool/delegation executes.
                            if !text_buffer.is_empty() {
                                hub.broadcast(
                                    "chat_stream",
                                    ws_payload!(
                                        "content": &text_buffer,
                                    ),
                                );
                                text_buffer.clear();
                                last_flush = tokio::time::Instant::now();
                            }
                            if let Some(cfg) = &comm_reply {
                                // Close the current prose segment: persist it as
                                // its OWN Message (durable; the gateway drops Stream
                                // frames), then rotate the stream id so this round's
                                // tool_activity tags the NEXT segment and renders
                                // above the prose that follows — interleaved like
                                // the desktop. Only fires when the segment has
                                // prose, so consecutive tool calls in one round
                                // don't each rotate.
                                if !comm_segment.trim().is_empty() {
                                    let mut seg_meta = std::collections::HashMap::new();
                                    if !agent_display_name.is_empty() {
                                        seg_meta.insert("senderName".to_string(), agent_display_name.clone());
                                    }
                                    send_comm_msg(
                                        cfg,
                                        &comm_manager,
                                        &channel_providers,
                                        comm::CommMessageType::Message,
                                        comm_stream_id.clone(),
                                        strip_progress_heartbeats(&comm_segment),
                                        seg_meta,
                                        &agent_display_name,
                                    )
                                    .await;
                                    comm_streamed = true;
                                    comm_segment.clear();
                                    comm_buffer.clear();
                                    comm_stream_id = uuid::Uuid::new_v4().to_string();
                                    last_comm_flush = Some(tokio::time::Instant::now());
                                }
                            }
                            if let Some(ref tc) = event.tool_call {
                                hub.broadcast(
                                    "tool_start",
                                    ws_payload!(
                                        "tool_id": tc.id,
                                        "tool": tc.name,
                                        "input": tc.input,
                                    ),
                                );
                                // Mirror the tool event to the loop so it shows live
                                // activity (and renders "Used N tools"), like the local app.
                                let (activity, _) = humanize_tool_call(&tc.name, &tc.input);
                                if let Some(cfg) = &comm_reply {
                                    let request = tc.input.to_string();
                                    send_comm_tool_activity(
                                        cfg,
                                        &comm_manager,
                                        &channel_providers,
                                        &comm_stream_id,
                                        &agent_display_name,
                                        "start",
                                        &tc.name,
                                        &tc.id,
                                        activity.clone(),
                                        Some(request.as_str()),
                                        None,
                                        None,
                                    )
                                    .await;
                                }
                                if let Some(ref cm) = comm_manager {
                                    if let Some(ref cr) = comm_reply {
                                        let _ = cm.send_typing(&cr.conversation_id, true, Some(&activity)).await;
                                    }
                                }
                            }
                        }
                        StreamEventType::ToolResult => {
                            let tool_name = event
                                .tool_call
                                .as_ref()
                                .map(|tc| tc.name.as_str())
                                .unwrap_or("");
                            let tool_id = event
                                .tool_call
                                .as_ref()
                                .map(|tc| tc.id.as_str())
                                .unwrap_or("");
                            hub.broadcast(
                                "tool_result",
                                ws_payload!(
                                    "tool_id": tool_id,
                                    "tool_name": tool_name,
                                    "result": event.text,
                                    "is_error": event.error.is_some(),
                                ),
                            );
                            // Mirror the tool result to the loop (char-safe, capped
                            // well under the 32KB frame) so the "Used N tools"
                            // timeline can show Request/Response like the local app.
                            if let Some(cfg) = &comm_reply {
                                let response: String = event.text.trim().chars().take(4000).collect();
                                let outcome = event
                                    .tool_call
                                    .as_ref()
                                    .map(|tc| humanize_tool_call(&tc.name, &tc.input).1)
                                    .unwrap_or_else(|| {
                                        tool_outcome_label(tool_name)
                                            .map(|s| s.to_string())
                                            .unwrap_or_else(|| humanize_tool_name(tool_name).1)
                                    });
                                send_comm_tool_activity(
                                    cfg,
                                    &comm_manager,
                                    &channel_providers,
                                    &comm_stream_id,
                                    &agent_display_name,
                                    "result",
                                    tool_name,
                                    tool_id,
                                    response,
                                    None,
                                    Some(event.error.is_some()),
                                    Some(outcome),
                                )
                                .await;
                            }
                            // Collect run-produced media: persist to <data_dir>/files and
                            // reference by /api/v1/files/<name> — for the LOCAL app (always,
                            // rendered inline) and comm replies (when replying to a channel;
                            // resolve_comm_attachments maps the same /api/v1/files prefix).
                            if event.error.is_none() {
                                if let Some(url) = &event.image_url {
                                    if let Some(app_url) = to_app_artifact_url(url) {
                                        if !app_file_artifacts.contains(&app_url) {
                                            app_file_artifacts.push(app_url.clone());
                                        }
                                        if comm_reply.is_some()
                                            && !comm_file_artifacts.contains(&app_url)
                                        {
                                            comm_file_artifacts.push(app_url);
                                        }
                                    }
                                }
                            }
                            needs_separator = true;
                        }
                        StreamEventType::Error => {
                            hub.broadcast(
                                "chat_error",
                                ws_payload!(
                                    "error": event.error.unwrap_or_default(),
                                ),
                            );
                        }
                        StreamEventType::Usage => {
                            if let Some(ref usage) = event.usage {
                                hub.broadcast(
                                    "usage",
                                    ws_payload!(
                                        "input_tokens": usage.input_tokens,
                                        "output_tokens": usage.output_tokens,
                                        "cache_read_input_tokens": usage.cache_read_input_tokens,
                                        "cache_creation_input_tokens": usage.cache_creation_input_tokens,
                                        "overhead_tokens": usage.overhead_tokens,
                                    ),
                                );
                            }
                        }
                        StreamEventType::ApprovalRequest => {
                            if let Some(ref tc) = event.tool_call {
                                hub.broadcast(
                                    "approval_request",
                                    serde_json::json!({
                                        "session_id": sid,
                                        "request_id": tc.id,
                                        "tool": tc.name,
                                        "input": tc.input,
                                    }),
                                );
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
                            // Forward the question to the loop/channel
                            // conversation — without this the run blocks on a
                            // question the remote user never sees. The next
                            // inbound message in this conversation resolves it
                            // (see the pending-ask check in handle_comm_message).
                            if let Some(cfg) = &comm_reply {
                                pending_comm_asks
                                    .lock()
                                    .await
                                    .insert(sid.to_string(), request_id.to_string());
                                let mut meta = HashMap::new();
                                meta.insert("kind".to_string(), "ask".to_string());
                                meta.insert("request_id".to_string(), request_id.to_string());
                                if !agent_display_name.is_empty() {
                                    meta.insert("senderName".to_string(), agent_display_name.clone());
                                }
                                if let Some(w) = &event.widgets {
                                    meta.insert("widgets".to_string(), w.to_string());
                                }
                                send_comm_msg(
                                    cfg,
                                    &comm_manager,
                                    &channel_providers,
                                    comm::CommMessageType::Message,
                                    uuid::Uuid::new_v4().to_string(),
                                    event.text.clone(),
                                    meta,
                                    &agent_display_name,
                                )
                                .await;
                                // The run is now waiting on the USER — clear the
                                // typing indicator or the loop shows "thinking…"
                                // until they answer. Resumed activity re-sets it.
                                if let Some(ref cm) = comm_manager {
                                    let _ = cm
                                        .send_typing(&cfg.conversation_id, false, None)
                                        .await;
                                }
                            }
                        }
                        StreamEventType::PlanApproval => {
                            let request_id = event.error.as_deref().unwrap_or("");
                            let mut payload = ws_payload!(
                                "request_id": request_id,
                                "plan": event.text,
                            );
                            if let Some(tools) = &event.widgets {
                                payload["tools"] = tools.clone();
                            }
                            hub.broadcast("plan_approval", payload);
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
                                        weekly_limit_credits: rl.weekly_limit_credits.unwrap_or(0),
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
                                        budget_credits_cents: rl.budget_credits_cents.unwrap_or(0),
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
                                hub.broadcast(
                                    "quota_warning",
                                    ws_payload!(
                                        "message": event.text,
                                    ),
                                );
                            }
                        }
                        StreamEventType::SubagentStart => {
                            let mut payload = ws_payload!();
                            if let Some(ref w) = event.widgets {
                                for (k, v) in w.as_object().into_iter().flatten() {
                                    payload[k] = v.clone();
                                }
                            }
                            hub.broadcast("subagent_start", payload);
                        }
                        StreamEventType::SubagentProgress => {
                            let mut payload = ws_payload!();
                            if let Some(ref w) = event.widgets {
                                for (k, v) in w.as_object().into_iter().flatten() {
                                    payload[k] = v.clone();
                                }
                            }
                            hub.broadcast("subagent_progress", payload);
                        }
                        StreamEventType::SubagentComplete => {
                            let mut payload = ws_payload!();
                            if let Some(ref w) = event.widgets {
                                for (k, v) in w.as_object().into_iter().flatten() {
                                    payload[k] = v.clone();
                                }
                            }
                            hub.broadcast("subagent_complete", payload);
                        }
                        StreamEventType::ToolSummary => {
                            hub.broadcast(
                                "tool_summary",
                                ws_payload!(
                                    "summary": &event.text,
                                ),
                            );
                            needs_separator = true;
                        }
                        StreamEventType::Done => {}
                    }
                }

                // Flush any remaining coalesced text
                if !text_buffer.is_empty() {
                    hub.broadcast(
                        "chat_stream",
                        ws_payload!(
                            "content": &text_buffer,
                        ),
                    );
                }

                // Clear typing indicator for NeboLoop conversations
                if let Some(ref cm) = comm_manager {
                    if let Some(ref cr) = comm_reply {
                        let _ = cm.send_typing(&cr.conversation_id, false, None).await;
                    }
                }

                // Send final comm reply — flush remaining stream buffer + complete message
                if let Some(reply_config) = &comm_reply {
                    // Strip markdown image references to local files — the actual
                    // images are delivered as comm attachments, so these would just
                    // render as broken links in the web frontend.
                    if !comm_file_artifacts.is_empty() {
                        strip_local_image_markdown(&mut comm_segment);
                    }
                    // Send the LAST prose segment as the final Message. Earlier
                    // segments were already persisted at their tool rounds, so this
                    // carries only the closing prose — re-sending full_response here
                    // would duplicate them. (full_response still drives the local
                    // web reply above; this branch is the comm/loop mirror only.)
                    if !comm_segment.trim().is_empty() {
                        // Build metadata with agent name for all outbound messages
                        let mut reply_meta = std::collections::HashMap::new();
                        if !agent_display_name.is_empty() {
                            reply_meta.insert("senderName".to_string(), agent_display_name.clone());
                        }
                        tracing::info!(
                            target: "neboai_identity",
                            agent_id = %agent_id,
                            agent_display_name = %agent_display_name,
                            reply_topic = %reply_config.topic,
                            reply_conv = %reply_config.conversation_id,
                            response_len = full_response.len(),
                            "RESPONSE: agent reply — attaching ONLY senderName (no agent id on the wire)"
                        );

                        // Resolve run-produced file artifacts to uploaded attachments
                        // (best-effort: a failed upload is logged and skipped, never
                        // blocks the text reply). Only the neboai provider supports
                        // uploads today.
                        let reply_attachments = if reply_config.provider == "neboai" {
                            resolve_comm_attachments(&comm_manager, &plugin_store, &comm_file_artifacts).await
                        } else {
                            Vec::new()
                        };

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
                                attachments: vec![],
                            };
                            if let Err(e) = send_to_channel(
                                &reply_config.provider,
                                &comm_manager,
                                &channel_providers,
                                chunk,
                            )
                            .await
                            {
                                warn!(error = %e, "failed to send comm stream flush");
                            }
                            comm_streamed = true;
                        }

                        // Final segment Message. Each segment (this one + any
                        // flushed at tool rounds) is a durable Message — the
                        // gateway drops Stream frames, so these are what survive a
                        // reload. Tool rounds tagged the following segment's id, so
                        // the client renders "Used N tools" above the prose that
                        // followed them: interleaved like the desktop, and durable.
                        info!(
                            topic = %reply_config.topic,
                            conv_id = %reply_config.conversation_id,
                            segment_len = comm_segment.len(),
                            attachment_count = reply_attachments.len(),
                            streamed = comm_streamed,
                            "sending comm reply (final segment message)"
                        );
                        let reply = comm::CommMessage {
                            id: comm_stream_id.clone(),
                            from: String::new(),
                            to: String::new(),
                            topic: reply_config.topic.clone(),
                            conversation_id: reply_config.conversation_id.clone(),
                            msg_type: comm::CommMessageType::Message,
                            content: strip_progress_heartbeats(&comm_segment),
                            metadata: reply_meta,
                            timestamp: 0,
                            human_injected: false,
                            human_id: None,
                            task_id: None,
                            correlation_id: None,
                            task_status: None,
                            artifacts: vec![],
                            error: None,
                            attachments: reply_attachments,
                        };
                        if let Err(e) = send_to_channel(
                            &reply_config.provider,
                            &comm_manager,
                            &channel_providers,
                            reply,
                        )
                        .await
                        {
                            warn!(error = %e, "failed to send comm reply");
                        }
                    } else {
                        // No text response. Still deliver any run-produced files as
                        // an attachments-only message so the channel gets the artifact.
                        let reply_attachments = if reply_config.provider == "neboai" {
                            resolve_comm_attachments(&comm_manager, &plugin_store, &comm_file_artifacts).await
                        } else {
                            Vec::new()
                        };
                        if reply_attachments.is_empty() {
                            warn!(
                                topic = %reply_config.topic,
                                conv_id = %reply_config.conversation_id,
                                "comm reply skipped: empty response from agent"
                            );
                        } else {
                            let mut reply_meta = std::collections::HashMap::new();
                            if !agent_display_name.is_empty() {
                                reply_meta.insert(
                                    "senderName".to_string(),
                                    agent_display_name.clone(),
                                );
                            }
                            info!(
                                topic = %reply_config.topic,
                                conv_id = %reply_config.conversation_id,
                                attachment_count = reply_attachments.len(),
                                "sending comm attachments (no text response)"
                            );
                            let attach_msg = comm::CommMessage {
                                id: uuid::Uuid::new_v4().to_string(),
                                from: String::new(),
                                to: String::new(),
                                topic: reply_config.topic.clone(),
                                conversation_id: reply_config.conversation_id.clone(),
                                msg_type: comm::CommMessageType::Message,
                                content: String::new(),
                                metadata: reply_meta,
                                timestamp: 0,
                                human_injected: false,
                                human_id: None,
                                task_id: None,
                                correlation_id: None,
                                task_status: None,
                                artifacts: vec![],
                                error: None,
                                attachments: reply_attachments,
                            };
                            if let Err(e) = send_to_channel(
                                &reply_config.provider,
                                &comm_manager,
                                &channel_providers,
                                attach_msg,
                            )
                            .await
                            {
                                warn!(error = %e, "failed to send comm attachments");
                            }
                        }
                    }
                }

                // Resolve the active chat so run-produced documents can be
                // versioned + persisted under a stable container.
                let chat_id_for_artifacts = runner
                    .sessions()
                    .resolve_session_id_by_key(&sid)
                    .ok()
                    .map(|session_id| runner.sessions().active_chat_id(&session_id));

                // Version each run-produced DOCUMENT (non-media) into its
                // append-only chain and emit structured artifact objects; media
                // stays a plain URL string (rendered inline). Mixed array — the
                // frontend splits by type. Falls back to the bare URL on any error.
                let chat_artifacts: Vec<serde_json::Value> = match &chat_id_for_artifacts {
                    Some(chat_id) if !app_file_artifacts.is_empty() => {
                        let message_id = runner
                            .store()
                            .latest_assistant_message_id(chat_id)
                            .ok()
                            .flatten();
                        version_app_artifacts(
                            runner.store(),
                            chat_id,
                            message_id.as_deref(),
                            &app_file_artifacts,
                        )
                    }
                    _ => app_file_artifacts
                        .iter()
                        .map(|u| serde_json::Value::String(u.clone()))
                        .collect(),
                };

                // Always send chat_complete (with any run-produced artifacts so
                // the app renders them). Carries NO message content: streamed
                // blocks finalize in place on the frontend — a final payload that
                // re-carried the turn text is what caused segments to render twice.
                hub.broadcast(
                    "chat_complete",
                    ws_payload!(
                        "artifacts": &chat_artifacts,
                    ),
                );

                // Persist the artifacts onto the turn's final assistant message so
                // Work items + their version chain survive history reload (the live
                // event above is the only other carrier).
                if !chat_artifacts.is_empty() {
                    if let Some(chat_id) = &chat_id_for_artifacts {
                        if let Err(e) = runner
                            .store()
                            .attach_artifacts_to_latest_assistant_message(chat_id, &chat_artifacts)
                        {
                            warn!(error = %e, chat_id = %chat_id, "failed to persist artifacts on message");
                        }
                    }
                }

                // Auto-generate a descriptive chat title in the background
                let title_runner = runner.clone();
                let title_hub = hub.clone();
                let title_state = task_state.clone();
                let title_sid = sid.clone();
                tokio::spawn(async move {
                    if let Err(e) = generate_chat_title_if_needed(
                        &title_runner,
                        &title_hub,
                        &title_state,
                        &title_sid,
                    )
                    .await
                    {
                        tracing::debug!(error = %e, "chat title generation skipped");
                    }
                });
            }
            Err(e) => {
                warn!(error = %e, "agent run failed");
                hub.broadcast(
                    "chat_error",
                    ws_payload!(
                        "error": e.to_string(),
                    ),
                );
                hub.broadcast("chat_complete", ws_payload!());
            }
        }

        // Clean up browser tabs for this session.
        // The extension tracks tabs by session UUID, not session_key, so resolve it.
        let browser_session_id = runner
            .store()
            .get_session_by_name(&sid)
            .ok()
            .flatten()
            .map(|s| s.id);
        let cleanup_id = browser_session_id.as_deref().unwrap_or(&sid);
        cleanup_tools.close_browser_session(cleanup_id).await;

        // RunHandle unregisters from RunRegistry on drop (including panics)
        drop(_run_handle);

        Ok(())
    });
    lane_task.fairness_key = Some(fairness_key);

    state.lanes.enqueue_async(&lane, lane_task);
}

/// Run chat through the canonical agent dispatch path and return raw stream events.
///
/// This is for API transports such as app SSE/REST that need direct access to
/// stream events while preserving the agent lens: lane queueing, RunRegistry,
/// persona, permissions, model preference, memory/session behavior, presence,
/// and proactive inbox.
pub async fn run_chat_events(
    state: &AppState,
    config: ChatConfig,
) -> Result<mpsc::Receiver<ai::StreamEvent>, types::NeboError> {
    let runner = state.runner.clone();
    let presence_tracker = state.presence.clone();
    let proactive_inbox = state.proactive_inbox.clone();
    let cleanup_tools = state.tools.clone();

    let sid = config.session_key.clone();
    let agent_id = config.agent_id.clone();
    let cancel_token = config.cancel_token.clone();
    let lane = config.lane.clone();

    // Resolve display name + register the run (shared with run_chat).
    let (_agent_display_name, run_handle) = register_run(state, &config).await;

    let (permissions, resource_grants, model_preference, personality_snippet) =
        if let Some(ref ec) = config.entity_config {
            (
                Some(ec.permissions.clone()),
                Some(ec.resource_grants.clone()),
                ec.model_preference.clone(),
                ec.personality_snippet.clone(),
            )
        } else {
            (None, None, None, None)
        };
    let allowed_paths = config
        .entity_config
        .as_ref()
        .map(|ec| ec.allowed_paths.clone())
        .unwrap_or_default();

    let progress = agent::RunProgress {
        run_id: run_handle.run_id.clone(),
        iteration_count: run_handle.iteration_count.clone(),
        tool_call_count: run_handle.tool_call_count.clone(),
        current_tool: run_handle.current_tool.clone(),
    };

    let req = RunRequest {
        session_key: sid.clone(),
        prompt: config.prompt,
        system: config.system,
        user_id: config.user_id,
        channel: config.channel,
        origin: config.origin,
        cancel_token: cancel_token.clone(),
        agent_id: agent_id.clone(),
        permissions,
        resource_grants,
        model_preference,
        personality_snippet,
        images: config.images,
        allowed_paths,
        presence_tracker: Some(presence_tracker),
        proactive_inbox: Some(proactive_inbox),
        progress: Some(progress),
        mention_context: config.mention_context,
        tool_scope: config.tool_scope,
        channel_ctx: config.channel_ctx,
        ..Default::default()
    };

    let (tx, rx) = mpsc::channel(64);
    let fairness_key = agent_id.clone();
    let mut lane_task = make_task(&lane, format!("chat:{}", sid), async move {
        let _run_handle = run_handle;
        match runner.run(req).await {
            Ok(mut events) => loop {
                let event = tokio::select! {
                    _ = cancel_token.cancelled() => break,
                    ev = events.recv() => match ev {
                        Some(e) => e,
                        None => break,
                    }
                };
                _run_handle.touch();
                if tx.send(event).await.is_err() {
                    break;
                }
            },
            Err(e) => {
                let _ = tx
                    .send(ai::StreamEvent {
                        event_type: ai::StreamEventType::Error,
                        text: String::new(),
                        tool_call: None,
                        error: Some(e.to_string()),
                        usage: None,
                        rate_limit: None,
                        widgets: None,
                        provider_metadata: None,
                        stop_reason: None,
                        image_url: None,
                    })
                    .await;
            }
        }

        let browser_session_id = runner
            .store()
            .get_session_by_name(&sid)
            .ok()
            .flatten()
            .map(|s| s.id);
        let cleanup_id = browser_session_id.as_deref().unwrap_or(&sid);
        cleanup_tools.close_browser_session(cleanup_id).await;
        drop(_run_handle);
        Ok(())
    });
    lane_task.fairness_key = Some(fairness_key);

    state.lanes.enqueue_async(&lane, lane_task);
    Ok(rx)
}

/// Resolve run-produced file artifacts (local `/api/v1/files/<name>` URLs or
/// `<data_dir>/files/...` paths) to uploaded comm attachments.
///
/// Best-effort: each artifact that can't be read or uploaded is logged and
/// skipped so the text reply is never blocked. Returns the successfully
/// uploaded attachments in input order.
async fn resolve_comm_attachments(
    comm_manager: &Option<Arc<comm::PluginManager>>,
    plugin_store: &Arc<napp::plugin::PluginStore>,
    artifacts: &[String],
) -> Vec<comm::wire::Attachment> {
    let mut out = Vec::new();
    if artifacts.is_empty() {
        return out;
    }
    let Some(mgr) = comm_manager.as_ref() else {
        return out;
    };

    // Files are served from <data_dir>/files/ (see handlers::files::serve_file).
    let files_dir = match config::data_dir() {
        Ok(d) => d.join("files"),
        Err(e) => {
            warn!(error = %e, "cannot resolve data dir for comm attachments");
            return out;
        }
    };

    for artifact in artifacts {
        // Map the artifact reference to a local path under <data_dir>/files/.
        let path = if let Some(rel) = artifact.strip_prefix("/api/v1/files/") {
            files_dir.join(rel)
        } else if artifact.starts_with("http://") || artifact.starts_with("https://") {
            // Remote URL we didn't produce locally — skip (can't read bytes).
            warn!(url = %artifact, "skipping remote artifact for comm attachment");
            continue;
        } else {
            std::path::PathBuf::from(artifact)
        };

        let data = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to read run artifact for attachment");
                continue;
            }
        };

        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        let mime = mime_from_extension(&path);

        match mgr.upload_file(&filename, &mime, data).await {
            Ok(att) => out.push(att),
            Err(e) => {
                warn!(filename = %filename, error = %e, "failed to upload run artifact attachment");
            }
        }

        // Decks can't render in a browser — upload the PDF preview alongside
        // (same cached conversion the local Work panel uses). The web pairs
        // "<name>.preview.pdf" to its deck and hides it from cards.
        let is_deck = matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("pptx" | "ppt")
        );
        if is_deck {
            match crate::handlers::files::ensure_pptx_preview(plugin_store, &path, &files_dir)
                .await
            {
                Ok(cache) => match tokio::fs::read(&cache).await {
                    Ok(pdf) => {
                        let preview_name = format!("{filename}.preview.pdf");
                        if let Err(e) = mgr
                            .upload_file(&preview_name, "application/pdf", pdf)
                            .await
                            .map(|att| out.push(att))
                        {
                            warn!(filename = %preview_name, error = %e, "failed to upload deck preview");
                        }
                    }
                    Err(e) => warn!(error = %e, "failed to read deck preview cache"),
                },
                Err(e) => {
                    // Best-effort: the deck still ships; the web shows a
                    // download card instead of a rendered preview.
                    warn!(filename = %filename, error = %e, "deck preview generation skipped");
                }
            }
        }
    }
    out
}

/// Infer a MIME type from a file extension for outbound comm attachments.
fn mime_from_extension(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "html" => "text/html",
        "docx" => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Map a run-produced `image_url` to a URL the LOCAL app can render and comm replies
/// can attach: `data:` URIs are persisted to `<data_dir>/files/`, local files are
/// located (or copied) under that dir, and both are referenced as `/api/v1/files/<name>`
/// (served by `handlers::files::serve_file`). `http(s)` URLs pass through; unservable
/// refs return None.
fn to_app_artifact_url(image_url: &str) -> Option<String> {
    if image_url.starts_with("http://") || image_url.starts_with("https://") {
        return Some(image_url.to_string());
    }
    if image_url.starts_with("/api/v1/files/") {
        return Some(image_url.to_string());
    }
    let abs_path = if image_url.starts_with("data:") {
        save_data_uri_to_file(image_url)?
    } else {
        image_url.to_string()
    };
    let files_dir = config::data_dir().ok()?.join("files");
    let p = std::path::Path::new(&abs_path);
    if let Ok(rel) = p.strip_prefix(&files_dir) {
        return Some(format!("/api/v1/files/{}", rel.to_string_lossy()));
    }
    // Local file outside <data_dir>/files — copy it in so serve_file can reach it.
    let _ = std::fs::create_dir_all(&files_dir);
    let filename = p.file_name()?.to_string_lossy().to_string();
    let dest = files_dir.join(&filename);
    std::fs::copy(p, &dest).ok()?;
    Some(format!("/api/v1/files/{}", filename))
}

/// Media (image/video) artifacts render inline and are never versioned.
const MEDIA_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "mp4", "webm", "mov"];

fn artifact_ext(url: &str) -> String {
    url.rsplit('/')
        .next()
        .and_then(|name| name.rsplit('.').next())
        .map(|e| e.to_lowercase())
        .unwrap_or_default()
}

/// Work-panel kind by extension. Mirrors the frontend's `artifactsToWorkItems`.
fn artifact_kind(ext: &str) -> &'static str {
    match ext {
        "csv" | "xlsx" | "xls" => "table",
        "pptx" | "ppt" => "slides",
        "js" | "ts" | "jsx" | "tsx" | "py" | "rs" | "go" | "json" | "sh" | "css" => "code",
        _ => "document",
    }
}

/// Version each run-produced DOCUMENT into its append-only chain (a `work_documents`
/// container keyed by (chat_id, filename) + a `work_document_versions` row), copying
/// the bytes to a version-specific path so old versions stay viewable and the open
/// viewer refreshes in place. Returns structured artifact objects
/// `{ documentId, filename, kind, version, url }`. Media artifacts pass through as a
/// plain URL string (rendered inline). Any per-artifact failure degrades to the bare
/// URL so the document still appears.
fn version_app_artifacts(
    store: &Arc<db::Store>,
    chat_id: &str,
    message_id: Option<&str>,
    flat_urls: &[String],
) -> Vec<serde_json::Value> {
    use sha2::{Digest, Sha256};

    let files_dir = match config::data_dir() {
        Ok(d) => d.join("files"),
        Err(_) => {
            return flat_urls
                .iter()
                .map(|u| serde_json::Value::String(u.clone()))
                .collect();
        }
    };

    flat_urls
        .iter()
        .map(|url| {
            let bare = serde_json::Value::String(url.clone());
            let ext = artifact_ext(url);
            // Media renders inline — never versioned.
            if MEDIA_EXTS.contains(&ext.as_str()) {
                return bare;
            }
            // Only flat /api/v1/files/<name> artifacts are local + versionable.
            let Some(name) = url.strip_prefix("/api/v1/files/") else {
                return bare;
            };
            let filename = name.rsplit('/').next().unwrap_or(name).to_string();
            let flat_path = files_dir.join(name);
            let Ok(bytes) = std::fs::read(&flat_path) else {
                return bare;
            };
            let hash = hex::encode(Sha256::digest(&bytes));
            let kind = artifact_kind(&ext);

            let built = (|| -> Result<serde_json::Value, types::NeboError> {
                let doc = store.upsert_work_document(chat_id, &filename, kind)?;
                let latest = store.latest_work_version(&doc.id)?;

                // Identical content → reuse the current version (no spurious bump).
                if let Some(ref v) = latest {
                    if v.content_hash.as_deref() == Some(hash.as_str()) {
                        return Ok(serde_json::json!({
                            "documentId": doc.id,
                            "filename": filename,
                            "kind": kind,
                            "version": v.version_number,
                            "url": v.url,
                        }));
                    }
                }

                // Content-addressed blob: store the bytes ONCE keyed by hash, so a
                // revert or the same content across documents reuses one file. The
                // ext keeps serve_file's content-type detection working.
                let blob_name = if ext.is_empty() {
                    hash.clone()
                } else {
                    format!("{}.{}", hash, ext)
                };
                let rel = format!("work/blobs/{}", blob_name);
                let dest = files_dir.join(&rel);
                if !dest.exists() {
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| types::NeboError::Internal(format!("mkdir blobs dir: {e}")))?;
                    }
                    std::fs::copy(&flat_path, &dest)
                        .map_err(|e| types::NeboError::Internal(format!("copy blob: {e}")))?;
                }
                let _ = store.register_content_blob(&hash, &ext, bytes.len() as i64);
                let versioned_url = format!("/api/v1/files/{}", rel);
                let parent_id = latest.as_ref().map(|v| v.id.as_str());
                let version = store.add_work_version(
                    &doc.id,
                    parent_id,
                    &versioned_url,
                    Some(&hash),
                    None,
                    message_id,
                )?;
                Ok(serde_json::json!({
                    "documentId": doc.id,
                    "filename": filename,
                    "kind": kind,
                    "version": version.version_number,
                    "url": versioned_url,
                }))
            })();

            match built {
                Ok(obj) => obj,
                Err(e) => {
                    warn!(error = %e, url = %url, "failed to version work document; using bare url");
                    bare
                }
            }
        })
        .collect()
}

/// Save a `data:` URI (base64 image) to `<data_dir>/files/` and return the
/// local file path. Used to materialize inline screenshots as uploadable
/// attachments for comm replies.
fn save_data_uri_to_file(data_uri: &str) -> Option<String> {
    use base64::Engine;

    tracing::info!(uri_len = data_uri.len(), prefix = &data_uri[..60.min(data_uri.len())], "save_data_uri_to_file called");

    let (mime, b64) = if let Some(rest) = data_uri.strip_prefix("data:image/jpeg;base64,") {
        ("jpeg", rest)
    } else if let Some(rest) = data_uri.strip_prefix("data:image/png;base64,") {
        ("png", rest)
    } else if let Some(rest) = data_uri.strip_prefix("data:image/webp;base64,") {
        ("webp", rest)
    } else {
        return None;
    };

    let bytes = match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "failed to decode data URI for comm attachment");
            return None;
        }
    };

    let files_dir = match config::data_dir() {
        Ok(d) => d.join("files"),
        Err(_) => return None,
    };
    let _ = std::fs::create_dir_all(&files_dir);

    let filename = format!("screenshot-{}.{}", uuid::Uuid::new_v4(), mime);
    let path = files_dir.join(&filename);
    if let Err(e) = std::fs::write(&path, &bytes) {
        warn!(error = %e, path = %path.display(), "failed to save screenshot for comm attachment");
        return None;
    }

    Some(path.to_string_lossy().to_string())
}

/// Strip markdown image references that point to local file paths.
/// These render as broken images in the web frontend — the actual
/// images are delivered as comm attachments instead.
fn strip_local_image_markdown(text: &mut String) {
    while let Some(start) = text.find("![") {
        let after_alt = match text[start + 2..].find("](") {
            Some(i) => start + 2 + i + 2,
            None => break,
        };
        let end = match text[after_alt..].find(')') {
            Some(i) => after_alt + i + 1,
            None => break,
        };
        let url = &text[after_alt..end - 1];
        if url.starts_with('/') || url.starts_with("file://") {
            // Remove the entire ![alt](path) and any surrounding whitespace/newlines
            let trim_start = if start > 0 && text.as_bytes()[start - 1] == b'\n' {
                start - 1
            } else {
                start
            };
            let trim_end = if end < text.len() && text.as_bytes()[end] == b'\n' {
                end + 1
            } else {
                end
            };
            text.replace_range(trim_start..trim_end, "");
        } else {
            break;
        }
    }
}

/// Build and send one comm message (stream chunk, tool activity, …) to a reply
/// channel. The single place outbound comm messages are assembled, so the loop
/// receives the same kinds of events the local web hub does.
#[allow(clippy::too_many_arguments)]
async fn send_comm_msg(
    cfg: &CommReplyConfig,
    comm_manager: &Option<Arc<comm::PluginManager>>,
    channel_providers: &Option<
        Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn comm::ChannelProvider>>>>,
    >,
    msg_type: comm::CommMessageType,
    id: String,
    content: String,
    mut metadata: HashMap<String, String>,
    sender_name: &str,
) {
    if !sender_name.is_empty() {
        metadata.insert("senderName".to_string(), sender_name.to_string());
    }
    let msg = comm::CommMessage {
        id,
        from: String::new(),
        to: String::new(),
        topic: cfg.topic.clone(),
        conversation_id: cfg.conversation_id.clone(),
        msg_type,
        content,
        metadata,
        timestamp: 0,
        human_injected: false,
        human_id: None,
        task_id: None,
        correlation_id: None,
        task_status: None,
        artifacts: vec![],
        error: None,
        attachments: vec![],
    };
    if let Err(e) = send_to_channel(&cfg.provider, comm_manager, channel_providers, msg).await {
        warn!(error = %e, "failed to send comm message");
    }
}

/// Forward a tool event (`phase` = "start" | "result") to a reply channel,
/// tagged with the response's `stream_id` so the loop can group it under the
/// reply as a "Used N tools" timeline — mirroring the local app.
#[allow(clippy::too_many_arguments)]
async fn send_comm_tool_activity(
    cfg: &CommReplyConfig,
    comm_manager: &Option<Arc<comm::PluginManager>>,
    channel_providers: &Option<
        Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn comm::ChannelProvider>>>>,
    >,
    stream_id: &str,
    sender_name: &str,
    phase: &str,
    tool: &str,
    tool_id: &str,
    content: String,
    request: Option<&str>,
    is_error: Option<bool>,
    outcome: Option<String>,
) {
    let mut metadata = HashMap::new();
    metadata.insert("phase".to_string(), phase.to_string());
    metadata.insert("tool".to_string(), tool.to_string());
    metadata.insert("tool_id".to_string(), tool_id.to_string());
    metadata.insert("stream_id".to_string(), stream_id.to_string());
    // start carries the request (tool input); result carries the error flag —
    // the loop renders these as the local app's Request/Response + Result chip.
    if let Some(req) = request {
        metadata.insert("request".to_string(), req.chars().take(2000).collect());
    }
    if let Some(err) = is_error {
        metadata.insert("is_error".to_string(), err.to_string());
    }
    // The result phase carries the past-tense outcome label — collapsed work
    // lines report what WAS DONE ("Read a file"), not effort.
    if let Some(outcome) = outcome {
        metadata.insert("outcome".to_string(), outcome);
    }
    send_comm_msg(
        cfg,
        comm_manager,
        channel_providers,
        comm::CommMessageType::ToolActivity,
        uuid::Uuid::new_v4().to_string(),
        content,
        metadata,
        sender_name,
    )
    .await;
}

/// Send a comm message through the appropriate channel provider.
/// Fast path for "neboai" uses the comm_manager directly.
/// Other providers are looked up in the channel_providers registry.
async fn send_to_channel(
    provider: &str,
    comm_manager: &Option<Arc<comm::PluginManager>>,
    channel_providers: &Option<
        Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn comm::ChannelProvider>>>>,
    >,
    msg: comm::CommMessage,
) -> Result<(), comm::CommError> {
    if provider == "neboai" {
        if let Some(ref mgr) = *comm_manager {
            return mgr.send(msg).await;
        }
        return Err(comm::CommError::NoActivePlugin);
    }
    if let Some(ref providers_lock) = *channel_providers {
        let providers: tokio::sync::RwLockReadGuard<
            '_,
            HashMap<String, Arc<dyn comm::ChannelProvider>>,
        > = providers_lock.read().await;
        if let Some(p) = providers.get(provider) {
            return p.send_response(msg).await;
        }
    }
    Err(comm::CommError::Other(format!(
        "unknown channel provider: {}",
        provider
    )))
}

/// Generate a descriptive chat title from the conversation's first messages.
/// Spawned as a background task after chat_complete — failures are non-fatal.
async fn generate_chat_title_if_needed(
    runner: &Arc<agent::Runner>,
    hub: &Arc<crate::handlers::ws::ClientHub>,
    state: &AppState,
    session_key: &str,
) -> Result<(), types::NeboError> {
    let store = runner.store();

    // Resolve session key → internal session ID → active chat ID
    let session_id = runner.sessions().resolve_session_id_by_key(session_key)?;
    let chat_id = runner.sessions().active_chat_id(&session_id);

    let chat = store
        .get_chat(&chat_id)?
        .ok_or(types::NeboError::NotFound)?;

    // Need a user+assistant exchange to name from.
    let messages = store.get_recent_chat_messages(&chat_id, 8)?;
    if messages.len() < 2 {
        return Ok(());
    }

    // Skip chats the user explicitly renamed — never clobber a chosen name (mirrors
    // Claude desktop's explicit-title check instead of matching default strings).
    if chat.title_custom {
        return Ok(());
    }
    // Trigger by USER-MESSAGE COUNT — language-independent. Name on the first
    // exchange, then re-refine ONCE at the third user turn over the fuller
    // conversation (Claude desktop's count-1 / count-3 behavior). Fires at most
    // twice and never depends on a per-locale default-title string ("New Chat" vs
    // the loop's "New chat" vs "Nuevo chat" / "新しいチャット").
    let user_turns = messages.iter().filter(|m| m.role == "user").count();
    if user_turns != 1 && user_turns != 3 {
        return Ok(());
    }

    // Build a compact transcript. Use more of the conversation on the count-3
    // refinement so the regenerated title reflects fuller context. Char-safe
    // truncation — content may be non-ASCII.
    let take_n = if user_turns >= 3 { 8 } else { 4 };
    let transcript: String = messages
        .iter()
        .take(take_n)
        .map(|m| {
            let snippet: String = m.content.chars().take(200).collect();
            format!("{}: {}", m.role, snippet)
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Generate the title via the one shared title-LLM primitive
    // (summarizer::generate_session_title) — the same call the runner's background
    // path uses, so there's a single title-generation implementation. The compact
    // transcript is the prompt; the gating (count 1/3), store write, broadcast and
    // loop-push stay here (server concerns). Empty model = provider default.
    let Some(new_title) =
        agent::summarizer::generate_session_title(&runner.providers(), &transcript, "").await
    else {
        return Ok(());
    };

    store.update_chat_title(&chat_id, &new_title, false)?;

    hub.broadcast(
        "chat_title_updated",
        serde_json::json!({
            "chatId": chat_id,
            "title": new_title,
        }),
    );

    info!(chat_id = %chat_id, title = %new_title, "auto-generated chat title");

    // Propagate the new title to the loop so a remote (NeboLoop) reader sees the
    // chat get named just like the desktop does — without waiting for the next
    // reconnect-time reconcile. local_agent_id is the `agent:<id>:` segment of
    // the session key ("assistant" for the primary companion). No-op for
    // non-agent sessions or agents not registered on the loop.
    if let Some(local_agent_id) = session_key
        .strip_prefix("agent:")
        .and_then(|s| s.split(':').next())
    {
        crate::codes::push_chat_title_to_loop(state, local_agent_id, &chat_id, &new_title).await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_progress_heartbeat, strip_progress_heartbeats};

    #[test]
    fn detects_orchestrator_heartbeats() {
        // The exact shapes orchestrator.rs emits (wrapped in surrounding newlines).
        assert!(is_progress_heartbeat("\n_Working on: agent_\n"));
        assert!(is_progress_heartbeat("\n_Working on: task: spawn_\n"));
        assert!(is_progress_heartbeat("_Working..._"));
    }

    #[test]
    fn leaves_real_content_alone() {
        assert!(!is_progress_heartbeat("Working on the report now."));
        assert!(!is_progress_heartbeat("_emphasis_ in the reply"));
        assert!(!is_progress_heartbeat("Here are the diligence scores: 85/90."));
    }

    #[test]
    fn strips_only_heartbeat_lines_preserving_content() {
        let input =
            "Here are the results.\n\n_Working on: agent_\nComposite score: 87.22\n_Working..._";
        assert_eq!(
            strip_progress_heartbeats(input),
            "Here are the results.\n\nComposite score: 87.22"
        );
    }
}
