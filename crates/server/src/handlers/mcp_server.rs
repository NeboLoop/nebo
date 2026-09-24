use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use agent::RunRequest;
use ai::StreamEventType;
use tools::Origin;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<serde_json::Value>,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

impl JsonRpcResponse {
    fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Option<serde_json::Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

/// POST /agent/mcp — JSON-RPC 2.0 handler for CLI provider tool access.
pub async fn agent_mcp_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let req: JsonRpcRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::OK,
                axum::Json(JsonRpcResponse::error(
                    None,
                    -32700,
                    format!("Parse error: {}", e),
                )),
            );
        }
    };

    info!(method = %req.method, "MCP request");

    let resp = match req.method.as_str() {
        "initialize" => JsonRpcResponse::success(
            req.id,
            serde_json::json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "nebo",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),

        "notifications/initialized" => {
            // Client acknowledgment — no response needed for notifications,
            // but since we're HTTP, return empty success
            JsonRpcResponse::success(req.id, serde_json::json!({}))
        }

        "tools/list" => {
            let tool_defs = state.tools.list().await;
            let mut tools: Vec<serde_json::Value> = tool_defs
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.input_schema,
                    })
                })
                .collect();
            tools.extend(service_tools());
            JsonRpcResponse::success(req.id, serde_json::json!({ "tools": tools }))
        }

        "tools/call" => {
            let name = req
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let arguments = req
                .params
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::json!({}));

            if name.is_empty() {
                JsonRpcResponse::error(req.id, -32602, "Missing tool name")
            } else if name == "nebo" {
                // Service tool — chat, sessions, events
                info!(tool = "nebo", "MCP service tool call");
                let (text, is_error) = execute_nebo_tool(&state, &arguments).await;
                let content = serde_json::json!([{ "type": "text", "text": text }]);
                JsonRpcResponse::success(
                    req.id,
                    serde_json::json!({ "content": content, "isError": is_error }),
                )
            } else {
                // A CLI provider's own tool call carries the credential its
                // run issued; anything else is an outside MCP client.
                let credential = headers
                    .get(agent::tool_credentials::HEADER)
                    .and_then(|v| v.to_str().ok());
                let grant = credential.map(|t| state.tool_credentials.grant(t));
                info!(tool = %name, run = credential.is_some(), "MCP tool call");
                let result = match grant {
                    Some(None) => tools::ToolResult::error(
                        "This run has ended, so its tool access has too. Nothing was run.",
                    ),
                    Some(Some(grant)) => {
                        call_tool(&state.store, &state.tools, Some(&grant), name, arguments).await
                    }
                    None => call_tool(&state.store, &state.tools, None, name, arguments).await,
                };

                let content = serde_json::json!([{
                    "type": "text",
                    "text": result.content,
                }]);

                JsonRpcResponse::success(
                    req.id,
                    serde_json::json!({
                        "content": content,
                        "isError": result.is_error,
                    }),
                )
            }
        }

        _ => {
            warn!(method = %req.method, "unknown MCP method");
            JsonRpcResponse::error(req.id, -32601, format!("Method not found: {}", req.method))
        }
    };

    (StatusCode::OK, axum::Json(resp))
}

/// Run one `tools/call` that arrived over `/agent/mcp`.
///
/// `run` is the run a CLI provider's call carries the credential of: the call
/// executes as that run — its context, its employee's rules, its approval
/// card — exactly as the runner's own tool call would. Without one the caller
/// is an outside MCP client (Claude Desktop, Cursor): `Origin::Mcp`, under
/// the rules of the main assistant resolved the way a chat run resolves them,
/// with nobody to ask, so what would ask is refused as for any unattended run.
/// Either way the call passes the runner's permission gate, then the registry.
async fn call_tool(
    store: &std::sync::Arc<db::Store>,
    tools: &tools::Registry,
    run: Option<&agent::RunGrant>,
    name: &str,
    arguments: serde_json::Value,
) -> tools::ToolResult {
    let outside;
    let run = match run {
        Some(run) => run,
        None => {
            outside = outside_client(store);
            &outside
        }
    };
    let call = ai::ToolCall {
        // The approval card is keyed by the call id.
        id: format!("mcp-{}", uuid::Uuid::new_v4().simple()),
        name: name.to_string(),
        input: tools.normalize_input(name, arguments).await,
    };
    let mut blocked = vec![None];
    let gate = agent::gate_tool_calls(
        &agent::GateRun {
            tools,
            store,
            agent_id: &run.agent_id,
            session_id: &run.ctx.session_id,
            session_key: &run.ctx.session_key,
            origin: run.ctx.origin,
            full_access: run.ctx.full_access,
            entity_permissions: run.ctx.entity_permissions.as_ref(),
            operation_policy: run.ctx.operation_policy.as_ref(),
            approval: run.approval.as_ref().map(|door| agent::ApprovalDoor {
                channels: &door.channels,
                tx: &door.tx,
                cancel_token: &door.cancel_token,
            }),
            approval_relay: run.approval_relay,
            workflow_mode: run.workflow_mode.as_ref(),
            sessions: run.sessions.as_ref(),
        },
        std::slice::from_ref(&call),
        &mut blocked,
    )
    .await;
    if let Some((_, refused)) = blocked.pop().flatten() {
        return refused;
    }
    let ctx = tools::ToolContext {
        approved_categories: gate.approved_categories,
        ..run.ctx.clone()
    };
    tools.execute(&ctx, &call.name, call.input).await
}

/// An outside MCP client's standing: the main assistant's rules, as a chat
/// run resolves them, and no approval door.
fn outside_client(store: &db::Store) -> agent::RunGrant {
    let rules = crate::entity_config::resolve_for_chat(store, "main", "main");
    let (permissions, resource_grants, _, _, allowed_paths, operation_policy) =
        crate::chat_dispatch::entity_run_params(rules.as_ref());
    agent::RunGrant {
        ctx: tools::ToolContext {
            origin: Origin::Mcp,
            user_id: "mcp-client".into(),
            session_key: "mcp".into(),
            entity_permissions: permissions,
            operation_policy,
            resource_grants,
            allowed_paths,
            full_access: crate::chat_dispatch::resolve_full_access(store),
            ..Default::default()
        },
        agent_id: String::new(),
        approval: None,
        approval_relay: false,
        workflow_mode: None,
        sessions: None,
    }
}

// ── nebo service tool ────────────────────────────────────────────────

/// Returns the MCP-only `nebo` service tool definition.
fn service_tools() -> Vec<serde_json::Value> {
    vec![serde_json::json!({
        "name": "nebo",
        "description": "Chat with nebo's agent and manage sessions. \
            For skills/workflows/agents use the existing skill(), work(), agent() tools.\n\n\
            Chat:\n  nebo(resource: \"chat\", action: \"send\", message: \"...\")\n  \
            nebo(resource: \"chat\", action: \"send\", message: \"...\", session_id: \"debug\")\n\n\
            Events:\n  nebo(action: \"emit\", source: \"my.event\")\n\n\
            Sessions:\n  nebo(resource: \"sessions\", action: \"list\")\n  \
            nebo(resource: \"my-session\", action: \"history\")\n  \
            nebo(resource: \"my-session\", action: \"reset\")",
        "inputSchema": {
            "type": "object",
            "properties": {
                "resource": { "type": "string", "description": "Target: 'chat', 'sessions', or a session id" },
                "action": { "type": "string", "description": "send, list, history, reset, emit" },
                "message": { "type": "string", "description": "Chat message (for action: send)" },
                "session_id": { "type": "string", "description": "Session ID for chat continuity (default: mcp-default)" },
                "timeout_secs": { "type": "integer", "description": "Max wait seconds for chat (default: 300, max: 600)" },
                "source": { "type": "string", "description": "Event source (for action: emit)" },
                "payload": { "type": "object", "description": "Event payload (for action: emit)" }
            },
            "required": ["action"]
        }
    })]
}

/// Dispatch a `nebo` service tool call by resource + action.
async fn execute_nebo_tool(state: &AppState, input: &serde_json::Value) -> (String, bool) {
    let action = input["action"].as_str().unwrap_or("");
    let resource = input["resource"].as_str().unwrap_or("");

    match (resource, action) {
        ("chat", "send") => handle_chat_send(state, input).await,
        (_, "emit") => handle_emit(state, input).await,
        ("sessions", "list") => handle_sessions_list(state).await,
        (id, "history") if !id.is_empty() => handle_session_history(state, id).await,
        (id, "reset") if !id.is_empty() => handle_session_reset(state, id).await,
        _ => (
            format!(
                "Unknown nebo action '{}' on resource '{}'",
                action, resource
            ),
            true,
        ),
    }
}

/// Send a chat message to nebo's agent and collect the full response.
async fn handle_chat_send(state: &AppState, input: &serde_json::Value) -> (String, bool) {
    let message = match input["message"].as_str() {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => return ("Missing 'message' parameter".into(), true),
    };

    let session_id = input["session_id"]
        .as_str()
        .unwrap_or("mcp-default")
        .to_string();
    let session_key = format!("mcp-{}", session_id);

    let timeout_secs = input["timeout_secs"].as_u64().unwrap_or(300).min(600);

    let cancel_token = CancellationToken::new();

    // Timeout watchdog
    let ct = cancel_token.clone();
    let watchdog = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(timeout_secs)).await;
        ct.cancel();
    });

    let req = RunRequest {
        session_key,
        prompt: message,
        channel: "mcp".into(),
        // External MCP clients (Claude Desktop, Cursor) are Origin::Mcp:
        // Autonomous-class (HITL asks blocked — nobody sees our modal from
        // another app) and subject to the Mcp origin deny list. Tagging them
        // User gave an external client the same trust as our own UI (TD: this
        // was the gap flagged in the 2026-07-23 execution-path audit).
        origin: Origin::Mcp,
        cancel_token: cancel_token.clone(),
        ..Default::default()
    };

    let mut rx = match state.runner.run(req).await {
        Ok(rx) => rx,
        Err(e) => {
            watchdog.abort();
            return (format!("Runner error: {}", e), true);
        }
    };

    let mut response = String::new();
    let mut tools_used: Vec<String> = Vec::new();
    let mut had_error = false;

    loop {
        let event = tokio::select! {
            _ = cancel_token.cancelled() => {
                if response.is_empty() {
                    response.push_str("[Timed out]");
                } else {
                    response.push_str("\n\n[Timed out]");
                }
                had_error = true;
                break;
            }
            ev = rx.recv() => match ev {
                Some(e) => e,
                None => break,
            }
        };

        match event.event_type {
            StreamEventType::Text => {
                response.push_str(&event.text);
            }
            StreamEventType::ToolCall => {
                if let Some(ref tc) = event.tool_call {
                    tools_used.push(tc.name.clone());
                }
            }
            StreamEventType::ApprovalRequest => {
                // Auto-approve all tool calls from MCP (once — don't persist a grant)
                if let Some(ref tc) = event.tool_call {
                    let mut channels = state.approval_channels.lock().await;
                    if let Some(tx) = channels.remove(&tc.id) {
                        let _ = tx.send("once".to_string());
                    }
                }
            }
            StreamEventType::AskRequest => {
                // Auto-answer ask requests with a default
                let request_id = event.error.as_deref().unwrap_or("");
                if !request_id.is_empty() {
                    crate::chat_dispatch::answer_ask(state, request_id, "yes".into()).await;
                }
            }
            StreamEventType::Error => {
                if let Some(ref err) = event.error {
                    response.push_str(&format!("\n[Error: {}]", err));
                    had_error = true;
                }
            }
            StreamEventType::Done => break,
            _ => {} // Thinking, Usage, RateLimit, ToolResult — skip
        }
    }

    watchdog.abort();

    // Append tool usage summary if any tools were called
    if !tools_used.is_empty() {
        response.push_str(&format!("\n\n[Tools used: {}]", tools_used.join(", ")));
    }

    (response, had_error)
}

/// Emit an event to the event bus.
async fn handle_emit(state: &AppState, input: &serde_json::Value) -> (String, bool) {
    let source = match input["source"].as_str() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return ("Missing 'source' parameter".into(), true),
    };
    let payload = input["payload"].clone();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    state.event_bus.emit(tools::Event {
        source: source.clone(),
        payload,
        origin: "mcp".into(),
        timestamp,
    });

    (format!("Event '{}' emitted", source), false)
}

/// List all agent sessions.
async fn handle_sessions_list(state: &AppState) -> (String, bool) {
    match state.runner.sessions().list_sessions("agent") {
        Ok(sessions) => {
            let json = serde_json::to_string_pretty(&sessions).unwrap_or_default();
            (json, false)
        }
        Err(e) => (format!("Failed to list sessions: {}", e), true),
    }
}

/// Get message history for a session.
async fn handle_session_history(state: &AppState, session_id: &str) -> (String, bool) {
    let key = if session_id.starts_with("mcp-") {
        session_id.to_string()
    } else {
        format!("mcp-{}", session_id)
    };

    match state.runner.sessions().get_messages(&key) {
        Ok(messages) => {
            let json = serde_json::to_string_pretty(&messages).unwrap_or_default();
            (json, false)
        }
        Err(e) => (format!("Failed to get history: {}", e), true),
    }
}

/// Reset (clear) a session's history.
async fn handle_session_reset(state: &AppState, session_id: &str) -> (String, bool) {
    let key = if session_id.starts_with("mcp-") {
        session_id.to_string()
    } else {
        format!("mcp-{}", session_id)
    };

    match state.runner.sessions().reset(&key) {
        Ok(_) => (format!("Session '{}' reset", session_id), false),
        Err(e) => (format!("Failed to reset session: {}", e), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tools::registry::DynTool;
    use tools::{ToolContext, ToolResult};

    /// A tool that reports whether it ran, declaring the rule key and
    /// capability the gates read.
    struct Probe {
        name: &'static str,
        operation: Option<&'static str>,
        key: &'static str,
        capability: Option<&'static str>,
    }
    impl Probe {
        /// A web call: the Web capability.
        fn web() -> Self {
            Self { name: "web", operation: None, key: "fetch_url", capability: Some("web") }
        }
        /// A shell command: `run_command`, the Shell capability.
        fn shell() -> Self {
            Self { name: "os", operation: None, key: "run_command", capability: Some("shell") }
        }
    }
    impl DynTool for Probe {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> String {
            String::new()
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn rule_key(&self, _input: &serde_json::Value) -> String {
            self.key.to_string()
        }
        fn capability(&self, _input: &serde_json::Value) -> Option<&'static str> {
            self.capability
        }
        fn operation_performed(&self, _input: &serde_json::Value) -> Option<String> {
            self.operation.map(str::to_string)
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a ToolContext,
            _input: serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
            Box::pin(async { ToolResult::ok("RAN") })
        }
    }

    async fn setup(probe: Probe, rules: serde_json::Value) -> (tempfile::TempDir, Arc<db::Store>, tools::Registry) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(dir.path().join("t.db").to_str().unwrap()).unwrap());
        store.upsert_entity_config("main", "main", &rules).unwrap();
        let registry = tools::Registry::new(tools::Policy::default());
        registry.register(Box::new(probe)).await;
        (dir, store, registry)
    }

    #[tokio::test]
    async fn a_capability_the_employee_lacks_is_refused() {
        let (_d, store, registry) = setup(
            Probe::web(),
            serde_json::json!({ "permissions": r#"{"web":false}"# }),
        )
        .await;
        let r = call_tool(&store, &registry, None, "web", serde_json::json!({})).await;
        assert!(r.is_error, "ran without the web capability: {}", r.content);
        assert!(r.content.starts_with("PERMISSION_REQUIRED:web"), "{}", r.content);
    }

    #[tokio::test]
    async fn a_capability_the_employee_has_runs() {
        let (_d, store, registry) = setup(
            Probe::web(),
            serde_json::json!({ "permissions": r#"{"web":true}"# }),
        )
        .await;
        let r = call_tool(&store, &registry, None, "web", serde_json::json!({})).await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.content, "RAN");
    }

    #[tokio::test]
    async fn a_blocked_operation_is_refused() {
        let (_d, store, registry) = setup(
            Probe { name: "plugin", operation: Some("payments.charge"), key: "plugin__payments", capability: None },
            serde_json::json!({ "operationPolicy": r#"{"operations":{"payments.charge":"blocked"}}"# }),
        )
        .await;
        let r = call_tool(&store, &registry, None, "plugin", serde_json::json!({})).await;
        assert!(r.is_error, "a Blocked operation ran: {}", r.content);
        assert!(r.content.contains("Blocked"), "{}", r.content);
    }

    // An outside client is an MCP client, whatever any run is doing.
    #[tokio::test]
    async fn an_outside_client_is_an_mcp_client() {
        let (_d, store, registry) = setup(Probe::shell(), serde_json::json!({})).await;
        let r = call_tool(&store, &registry, None, "os", shell_call()).await;
        assert!(r.is_error, "shell ran for an MCP client: {}", r.content);
        assert!(r.content.contains("not permitted"), "{}", r.content);
    }

    fn shell_call() -> serde_json::Value {
        serde_json::json!({ "resource": "shell", "action": "exec", "command": "true" })
    }

    /// A chat run on the CLI provider, as the runner issues it: the owner's
    /// own chat (Origin::User) for an employee with the given rules.
    fn cli_run(permissions: &[(&str, bool)], door: Option<agent::tool_credentials::OwnedApprovalDoor>) -> agent::RunGrant {
        agent::RunGrant {
            ctx: ToolContext {
                origin: Origin::User,
                session_key: "agent:emp-1:web".into(),
                session_id: "s-1".into(),
                entity_permissions: Some(permissions.iter().map(|(k, v)| (k.to_string(), *v)).collect()),
                ..Default::default()
            },
            agent_id: "emp-1".into(),
            approval: door,
            approval_relay: false,
            workflow_mode: None,
            sessions: None,
        }
    }

    // Claude Code as the model runs its tool calls over /agent/mcp. With its
    // run's credential they are the run's own calls: shell runs for an
    // employee allowed shell.
    #[tokio::test]
    async fn a_cli_provider_run_calls_as_its_run() {
        let (_d, store, registry) = setup(Probe::shell(), serde_json::json!({})).await;
        let run = cli_run(&[("shell", true)], None);
        let r = call_tool(&store, &registry, Some(&run), "os", shell_call()).await;
        assert!(!r.is_error, "the run's own shell call was refused: {}", r.content);
        assert_eq!(r.content, "RAN");
    }

    // ...and under that employee's rules: shell off, nobody to ask → refused.
    #[tokio::test]
    async fn a_cli_provider_run_keeps_its_employees_rules() {
        let (_d, store, registry) = setup(Probe::shell(), serde_json::json!({})).await;
        let run = cli_run(&[("shell", false)], None);
        let r = call_tool(&store, &registry, Some(&run), "os", shell_call()).await;
        assert!(r.is_error, "{}", r.content);
        assert!(r.content.starts_with("PERMISSION_REQUIRED:shell"), "{}", r.content);
    }

    // A capability that is off asks the owner on the run's approval card,
    // exactly as the runner's own call would; "once" lets it run.
    #[tokio::test]
    async fn a_cli_provider_runs_ask_reaches_the_owner() {
        let (_d, store, registry) = setup(Probe::shell(), serde_json::json!({})).await;
        let channels: tools::ApprovalChannels = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let door = agent::tool_credentials::OwnedApprovalDoor {
            channels: channels.clone(),
            tx,
            cancel_token: tokio_util::sync::CancellationToken::new(),
        };
        let owner = tokio::spawn(async move {
            let card = rx.recv().await.expect("an approval card");
            assert_eq!(card.event_type, ai::StreamEventType::ApprovalRequest);
            let id = card.tool_call.expect("the call on the card").id;
            let answer = channels.lock().await.remove(&id).expect("a waiting answer");
            answer.send("once".to_string()).unwrap();
        });
        let run = cli_run(&[("shell", false)], Some(door));
        let r = call_tool(&store, &registry, Some(&run), "os", shell_call()).await;
        owner.abort();
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.content, "RAN");
    }
}
