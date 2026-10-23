use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

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
    owner: Option<axum::Extension<crate::middleware::OwnerKey>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    // One of the owner's own clients (it presented the install key) is the
    // owner: its calls, its chats and their helpers run as his own (10-21:
    // an owner-driven build through MCP, and its helpers, had no shell).
    // Any other MCP client is another program's words.
    let outside = if owner.is_some() { Caller::Owner } else { Caller::Outside };
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
            )
                .into_response();
        }
    };

    info!(method = %req.method, "MCP request");

    // A notification (no id) is answered 202 with no body, as MCP's
    // Streamable HTTP transport says: a client such as Codex's drops a
    // server that answers one with a JSON-RPC response.
    if req.id.is_none() {
        return StatusCode::ACCEPTED.into_response();
    }

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
                let origin = if owner.is_some() { Origin::User } else { Origin::Mcp };
                let (text, is_error) = execute_nebo_tool(&state, &arguments, origin).await;
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
                let tool_use_id = tool_use_id(&req.params);
                info!(tool = %name, run = credential.is_some(), "MCP tool call");
                let result = match grant {
                    Some(None) => tools::ToolResult::error(
                        "This run has ended, so its tool access has too. Nothing was run.",
                    ),
                    Some(Some(grant)) => {
                        let result = call_tool(&state.tools, &state.store, Caller::Run(&grant), tool_use_id, name, arguments.clone()).await;
                        if let (Some(token), Some(id)) = (credential, tool_use_id)
                            && let Some(handed) = owner_handed(&state.tools, name, &arguments, &result).await
                        {
                            state.tool_credentials.hand(token, id, handed);
                        }
                        result
                    }
                    None => call_tool(&state.tools, &state.store, outside, None, name, arguments).await,
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

    (StatusCode::OK, axum::Json(resp)).into_response()
}

/// Who a `tools/call` over `/agent/mcp` comes from.
#[derive(Clone, Copy)]
enum Caller<'a> {
    /// A CLI provider's run, by the credential the run issued it.
    Run(&'a agent::RunGrant),
    /// One of the owner's own clients: it presented the install key.
    Owner,
    /// Any other MCP client.
    Outside,
}

/// Run one `tools/call` that arrived over `/agent/mcp`.
///
/// A CLI provider's run (`Caller::Run`) calls as that run, its context and
/// its grant, exactly as the runner's own tool call would. Any other client
/// calls through `Door::Mcp` under the main assistant's grant: the owner's
/// own as the owner (`Origin::User`), any other as `Origin::Mcp`. Either way
/// the registry's one permission check decides.
async fn call_tool(
    tools: &tools::Registry,
    store: &db::Store,
    caller: Caller<'_>,
    tool_use_id: Option<&str>,
    name: &str,
    mut arguments: serde_json::Value,
) -> tools::ToolResult {
    let (run, mut ctx) = match caller {
        Caller::Run(run) => (Some(run), run.ctx.clone()),
        Caller::Owner => (None, mcp_client(Origin::User)),
        Caller::Outside => (None, mcp_client(Origin::Mcp)),
    };
    if let Some(id) = tool_use_id {
        ctx.tool_call_id = id.to_string();
    }
    // The run's code work: the call is pointed at the project's own copy,
    // its first change to a project makes the copy, exactly as the runner's
    // own call is routed. A copy that can't be made refuses the call.
    let verify = run.and_then(|r| r.verify.as_ref());
    let mut started = None;
    // What the call's routing watched for a deploy, judged once it ran.
    let mut watched = Default::default();
    if let Some(verify) = verify {
        let changes = tools.target(name, &arguments).await.is_some_and(|t| !t.read_only);
        match verify.route(store, &mut arguments, &mut ctx.cwd, changes).await {
            Ok(routed) => {
                started = routed.started;
                watched = routed.watched;
            }
            Err(e) => {
                return tools::ToolResult::error(format!("{e}. Nothing was changed: code work happens only in its own copy."));
            }
        }
    }
    let mut result = tools.execute(&ctx, name, arguments).await;
    if let Some(verify) = verify
        && let Some(note) = verify.after_call(store, watched).await
    {
        started = Some(started.map_or(note.clone(), |s| format!("{s}\n\n{note}")));
    }
    if let Some(note) = started {
        result.content = format!("{}\n\n{note}", result.content);
    }
    result
}

/// The CLI's own id for a `tools/call`, which it streams that call's result
/// under: `params._meta["claudecode/toolUseId"]`.
fn tool_use_id(params: &serde_json::Value) -> Option<&str> {
    params
        .get("_meta")
        .and_then(|m| m.get("claudecode/toolUseId"))
        .and_then(|v| v.as_str())
        .filter(|id| !id.is_empty())
}

/// What a run's call hands the owner, kept for the run beside the text the
/// CLI passes on. Files only when the tool's spec says they are the owner's
/// (`DynTool::emits_image`), the rule a runner-executed call's are kept by.
async fn owner_handed(
    tools: &tools::Registry,
    name: &str,
    arguments: &serde_json::Value,
    result: &tools::ToolResult,
) -> Option<agent::tool_credentials::Handed> {
    if result.is_error {
        return None;
    }
    let mut handed = agent::tool_credentials::Handed::of(result)?;
    let owners = match tools.get(name).await {
        Some(tool) => tool.emits_image(arguments),
        None => false,
    };
    if !owners {
        handed.image_url = None;
        handed.more_files.clear();
    }
    (handed != Default::default()).then_some(handed)
}

/// An MCP client's standing, from `origin`: the main assistant's grant,
/// which the permission check resolves from the session.
fn mcp_client(origin: Origin) -> tools::ToolContext {
    tools::ToolContext {
        origin,
        door: types::permissions::Door::Mcp,
        user_id: "mcp-client".into(),
        session_key: "mcp".into(),
        ..Default::default()
    }
}

// ── nebo service tool ────────────────────────────────────────────────

/// Returns the MCP-only `nebo` service tool definition.
fn service_tools() -> Vec<serde_json::Value> {
    vec![serde_json::json!({
        "name": "nebo",
        "description": "Chat with nebo's agent and manage sessions. \
            For skills, workflows and agents use the existing skill (use_skill, find_skills, …), workflow (list_workflows, run_workflow, …) and agent tools.\n\n\
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
async fn execute_nebo_tool(state: &AppState, input: &serde_json::Value, origin: Origin) -> (String, bool) {
    let action = input["action"].as_str().unwrap_or("");
    let resource = input["resource"].as_str().unwrap_or("");

    match (resource, action) {
        ("chat", "send") => handle_chat_send(state, input, origin).await,
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
/// `origin`: `Origin::User` for the owner's own client, whose chat and its
/// helpers run as his own; `Origin::Mcp` for any other.
async fn handle_chat_send(state: &AppState, input: &serde_json::Value, origin: Origin) -> (String, bool) {
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

    let req = agent::TurnRequest {
        session_key,
        input: agent::harness::TurnInput::Owner { text: message, images: Vec::new(), attachments: Vec::new(), message_id: None },
        seat: agent::harness::SeatRequest {
            agent_id: String::new(),
            user_id: String::new(),
            // Another MCP client is Origin::Mcp, under its deny list; the
            // owner's own is the owner.
            origin,
            door: types::permissions::Door::Mcp,
            mode: None,
            narrow: None,
            ceiling: None,
            cwd: None,
            seed_taint: Vec::new(),
            audience: None,
            tool_allowlist: None,
            tool_denial_hint: None,
            handoff_depth: 0,
            model_override: String::new(),
            model_preference: None,
            personality_snippet: None,
            tool_scope: None,
        },
        mode: agent::harness::TurnMode::Chat,
        delivery: agent::harness::Delivery { channel: "mcp".into(), channel_ctx: None, platform: None, mention_briefing: None },
        cancel: cancel_token.clone(),
        progress: None,
    };

    let mut rx = match state.harness.start_turn(req).await {
        Ok(handle) => handle.events,
        Err(e) => {
            watchdog.abort();
            return (format!("The turn could not start: {}", e), true);
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
                    if let Some(pending) = channels.remove(&tc.id) {
                        let _ = pending.answer.send("once".to_string());
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
    match state.harness.sessions().list_sessions("agent") {
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

    match state.harness.sessions().get_messages(&key) {
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

    match state.harness.sessions().reset(&key) {
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
    use types::permissions::{Effect, Rule, RuleKey, RuleSource, Scope, Writer};

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
            Self { name: "run_command", operation: None, key: "run_command", capability: Some("shell") }
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

    fn rule(scope: Scope, key: RuleKey, effect: Effect) -> Rule {
        Rule {
            id: uuid::Uuid::new_v4().to_string(),
            scope,
            key,
            field: None,
            effect,
            money: None,
            source: RuleSource::Owner,
            locked: false,
            created_at: 0,
        }
    }

    async fn setup(probe: Probe, rules: Vec<Rule>) -> (tempfile::TempDir, Arc<db::Store>, tools::Registry) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(dir.path().join("t.db").to_str().unwrap()).unwrap());
        for r in rules {
            store.write_permission_rule(&r, &Writer::Owner).unwrap();
        }
        let registry = tools::Registry::new(Arc::new(agent::Check::new(store.clone())));
        registry.register(Box::new(probe)).await;
        (dir, store, registry)
    }

    fn company(key: RuleKey, effect: Effect) -> Rule {
        rule(Scope::Company, key, effect)
    }

    #[tokio::test]
    async fn a_capability_the_employee_lacks_is_refused() {
        let (_d, store, registry) = setup(Probe::web(), vec![company(RuleKey::Capability("web".into()), Effect::Deny)]).await;
        let r = call_tool(&registry, &store, Caller::Outside, None, "web", serde_json::json!({})).await;
        assert!(r.is_error, "ran without the web capability: {}", r.content);
        assert!(r.content.contains("permission is off"), "{}", r.content);
    }

    #[tokio::test]
    async fn a_capability_the_employee_has_runs() {
        let (_d, store, registry) = setup(Probe::web(), vec![company(RuleKey::Capability("web".into()), Effect::Allow)]).await;
        let r = call_tool(&registry, &store, Caller::Outside, None, "web", serde_json::json!({})).await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.content, "RAN");
    }

    #[tokio::test]
    async fn a_blocked_operation_is_refused() {
        let (_d, store, registry) = setup(
            Probe { name: "payments_charge", operation: Some("payments.charge"), key: "payments_charge", capability: None },
            vec![company(RuleKey::Operation("payments.charge".into()), Effect::Deny)],
        )
        .await;
        let r = call_tool(&registry, &store, Caller::Outside, None, "payments_charge", serde_json::json!({})).await;
        assert!(r.is_error, "a denied operation ran: {}", r.content);
        assert!(r.content.contains("turned off"), "{}", r.content);
    }

    // An outside client is an MCP client, whatever any run is doing.
    #[tokio::test]
    async fn an_outside_client_is_an_mcp_client() {
        let (_d, store, registry) = setup(Probe::shell(), vec![company(RuleKey::Capability("shell".into()), Effect::Allow)]).await;
        let r = call_tool(&registry, &store, Caller::Outside, None, "run_command", shell_call()).await;
        assert!(r.is_error, "shell ran for an MCP client: {}", r.content);
        assert!(r.content.contains("not permitted"), "{}", r.content);
    }

    // The owner's own client (it presented the install key) is the owner:
    // the shell an employee has runs for it, and for its helpers.
    #[tokio::test]
    async fn the_owners_own_client_is_the_owner() {
        let (_d, store, registry) = setup(Probe::shell(), vec![company(RuleKey::Capability("shell".into()), Effect::Allow)]).await;
        let r = call_tool(&registry, &store, Caller::Owner, None, "run_command", shell_call()).await;
        assert!(!r.is_error && r.content == "RAN", "{}", r.content);
    }

    fn shell_call() -> serde_json::Value {
        serde_json::json!({ "command": "true", "description": "Do nothing" })
    }

    /// A chat run on the CLI provider, as the runner issues it: the owner's
    /// own chat (Origin::User) for an employee with the given shell rule.
    fn cli_run(store: &db::Store, shell: Effect) -> agent::RunGrant {
        store
            .write_permission_rule(
                &rule(Scope::Employee("emp-1".into()), RuleKey::Capability("shell".into()), shell),
                &Writer::Owner,
            )
            .unwrap();
        agent::RunGrant {
            ctx: ToolContext {
                origin: Origin::User,
                session_key: "agent:emp-1:web".into(),
                session_id: "s-1".into(),
                grant: Some(Arc::new(agent::resolve_grant(store, "emp-1", None))),
                ..Default::default()
            },
            agent_id: "emp-1".into(),
            verify: None,
        }
    }

    // The `claude` CLI as the model runs its tool calls over /agent/mcp. With its
    // run's credential they are the run's own calls: shell runs for an
    // employee allowed shell.
    #[tokio::test]
    async fn a_cli_provider_run_calls_as_its_run() {
        let (_d, store, registry) = setup(Probe::shell(), vec![]).await;
        let run = cli_run(&store, Effect::Allow);
        let r = call_tool(&registry, &store, Caller::Run(&run), None, "run_command", shell_call()).await;
        assert!(!r.is_error, "the run's own shell call was refused: {}", r.content);
        assert_eq!(r.content, "RAN");
    }

    // ...and under that employee's rules: shell denied → refused.
    #[tokio::test]
    async fn a_cli_provider_run_keeps_its_employees_rules() {
        let (_d, store, registry) = setup(Probe::shell(), vec![]).await;
        let run = cli_run(&store, Effect::Deny);
        let r = call_tool(&registry, &store, Caller::Run(&run), None, "run_command", shell_call()).await;
        assert!(r.is_error, "{}", r.content);
        assert!(r.content.contains("permission is off"), "{}", r.content);
    }

    // A call an ask rule covers parks on the owner, exactly as the runner's
    // own call would: it does not run, and the ask is written.
    #[tokio::test]
    async fn a_cli_provider_runs_ask_parks_for_the_owner() {
        let (_d, store, registry) = setup(Probe::shell(), vec![]).await;
        let run = cli_run(&store, Effect::Ask);
        let r = call_tool(&registry, &store, Caller::Run(&run), None, "run_command", shell_call()).await;
        assert_ne!(r.content, "RAN", "an asked call ran");
        let ask = r.parked_ask.as_deref().expect("the call parked");
        assert_eq!(store.get_permission_ask(ask).unwrap().map(|a| a.status).as_deref(), Some("open"));
    }

    /// Writes `content` to `path`.
    struct FileWriter;
    impl DynTool for FileWriter {
        fn name(&self) -> &str {
            "save_code"
        }
        fn description(&self) -> String {
            String::new()
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn read_only(&self, _input: &serde_json::Value) -> bool {
            false
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a ToolContext,
            input: serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
            Box::pin(async move {
                match std::fs::write(input["path"].as_str().unwrap_or(""), input["content"].as_str().unwrap_or("")) {
                    Ok(()) => ToolResult::ok("written"),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            })
        }
    }

    /// A CLI model's write to a project over /agent/mcp goes where the
    /// runner's own would: into the run's copy of the project, never the
    /// owner's files, so it is verified like any other code work.
    #[tokio::test]
    async fn a_cli_runs_write_to_a_project_goes_to_its_own_copy() {
        static HOME: std::sync::OnceLock<Option<tempfile::TempDir>> = std::sync::OnceLock::new();
        HOME.get_or_init(|| {
            if std::env::var_os("NEBO_HOME").is_some() {
                return None;
            }
            let dir = tempfile::tempdir().unwrap();
            // SAFETY: one-time initialization, and only when no root is set.
            unsafe { std::env::set_var("NEBO_HOME", dir.path()) };
            Some(dir)
        });
        let (_d, store, registry) = setup(Probe::shell(), vec![]).await;
        registry.register(Box::new(FileWriter)).await;
        let project = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let mut cmd = command::new::<std::process::Command>("git", command::Console::Hidden);
            assert!(cmd.args(args).current_dir(project.path()).status().unwrap().success());
        };
        for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@t"], &["config", "user.name", "t"]] {
            git(args);
        }
        std::fs::write(project.path().join("Makefile"), "test:\n\ttrue\n").unwrap();
        std::fs::write(project.path().join("total.py"), "def total(a, b): return 0\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "base"]);
        let builder = agent::harness::verify::engine::Builder {
            agent_id: "emp-1".into(),
            session_key: "agent:emp-1:web".into(),
            spec: "Make total add".into(),
            ..Default::default()
        };
        let mut run = cli_run(&store, Effect::Allow);
        run.verify = Some(agent::harness::verify::engine::Verify::begin(&store, builder, None).await);
        let file = project.path().join("total.py").to_string_lossy().into_owned();
        let r = call_tool(&registry, &store, Caller::Run(&run), None, "save_code", serde_json::json!({"path": file, "content": "def total(a, b): return a + b\n"})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("your own working copy"), "the model is told where it works: {}", r.content);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "def total(a, b): return 0\n", "the owner's file is untouched");
        let v = store.list_verifications(&db::VerificationQuery::default()).unwrap().remove(0);
        assert_eq!(std::fs::read_to_string(std::path::Path::new(&v.work_dir).join("total.py")).unwrap(), "def total(a, b): return a + b\n");
    }
}
