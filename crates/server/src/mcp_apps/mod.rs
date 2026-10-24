//! MCP Apps (`io.modelcontextprotocol/ui`) on the engine's side: the view
//! origin the desktop's sandbox proxy runs on, and a view's requests that
//! need the engine (`mcp_app_request` on the client socket).
//!
//! A tool's view arrives stored with its result (`payload.kind ==
//! "mcp_app"`, `tools::mcp_tool::McpProxyTool`). Each client renders it
//! itself and handles the view's own requests locally (initialize, sizes,
//! display mode, links). Three requests reach the engine, over the client
//! WebSocket every client already holds, as one frame type:
//!
//! - `tools/call`: only a tool of the server that showed the view, and
//!   only one its server lets views call; run through the registry's one
//!   door as the employee's own call (its mode, its rules, the owner's
//!   asks), entered by `Door::View` so every decision records it;
//! - `ui/message`: the owner's message in that conversation, as if typed;
//! - `ui/update-model-context`: kept for the conversation's next step.

use std::sync::OnceLock;

use axum::extract::Query;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::state::AppState;

/// The sandbox proxy page (spec "Sandbox proxy").
const SANDBOX_PAGE: &str = include_str!("sandbox.html");

/// Where the sandbox proxy is served: an origin of its own on this
/// computer (`http://127.0.0.1:<port>`), set once its listener binds.
static VIEW_ORIGIN: OnceLock<String> = OnceLock::new();

/// The view origin, once its listener is up; told to clients in `auth_ok`.
pub fn view_origin() -> Option<&'static str> {
    VIEW_ORIGIN.get().map(String::as_str)
}

/// Bind the view origin's listener and serve it: a loopback port of its
/// own that serves the sandbox proxy page and nothing else. The spec wants
/// the proxy on an origin other than the host's; Nebo's own origin would
/// put the owner's session and the whole API within a view's `'self'`.
pub async fn serve_view_origin() {
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(l) => l,
        Err(e) => {
            warn!(error = %e, "the view origin could not bind; interactive results show as text");
            return;
        }
    };
    let Ok(addr) = listener.local_addr() else { return };
    let _ = VIEW_ORIGIN.set(format!("http://127.0.0.1:{}", addr.port()));
    info!(origin = %format!("http://127.0.0.1:{}", addr.port()), "view origin listening");
    let app = axum::Router::new().route("/sandbox", axum::routing::get(sandbox_page));
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            warn!(error = %e, "the view origin stopped");
        }
    });
}

#[derive(serde::Deserialize)]
struct SandboxQuery {
    /// The view's declared `_meta.ui.csp`, as JSON. Absent: it declared none.
    csp: Option<String>,
}

/// The frames that may hold the proxy: Nebo's own pages on this computer.
const FRAME_ANCESTORS: &str = "frame-ancestors http://localhost:* http://127.0.0.1:* tauri: http://tauri.localhost";

/// The proxy page, under the view's policy ([`mcp::ui::csp_policy`]) as its
/// header, where the view's HTML can't change it.
async fn sandbox_page(Query(q): Query<SandboxQuery>) -> Response {
    let csp = match q.csp.as_deref().map(serde_json::from_str::<mcp::ui::UiCsp>) {
        None => None,
        Some(Ok(csp)) => Some(csp),
        Some(Err(_)) => return (StatusCode::BAD_REQUEST, "csp is not a view's declared csp").into_response(),
    };
    let policy = format!("{}; {FRAME_ANCESTORS}", mcp::ui::csp_policy(csp.as_ref()));
    let mut resp = (StatusCode::OK, SANDBOX_PAGE).into_response();
    let headers = resp.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(v) = HeaderValue::from_str(&policy) {
        headers.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    resp
}

/// A JSON-RPC error for the view.
fn error(code: i64, message: &str) -> Value {
    json!({ "error": { "code": code, "message": message } })
}

/// Answer one `mcp_app_request` frame's `data`
/// (`{requestId, chatId, messageId, method, params}`): the
/// `mcp_app_response` frame's `data`, `{requestId, result}` or
/// `{requestId, error: {code, message}}`. `messageId` is the call id the
/// view's result is stored under; the server is the one stored with it.
pub async fn answer(state: &AppState, data: &Value) -> Value {
    let request_id = data.get("requestId").cloned().unwrap_or(Value::Null);
    let mut reply = request(state, data).await;
    reply["requestId"] = request_id;
    reply
}

async fn request(state: &AppState, data: &Value) -> Value {
    let chat_id = data["chatId"].as_str().unwrap_or("");
    let call_id = data["messageId"].as_str().unwrap_or("");
    let method = data["method"].as_str().unwrap_or("");
    let params = data.get("params").cloned().unwrap_or_else(|| json!({}));
    let Some(chat) = state.store.get_chat(chat_id).ok().flatten() else {
        return error(-32602, "no such conversation");
    };
    let Some(session_key) = chat.session_name.filter(|k| !k.is_empty()) else {
        return error(-32602, "no such conversation");
    };
    let stored = state.store.find_tool_result(chat_id, call_id).ok().flatten();
    let Some(view) = stored
        .as_ref()
        .and_then(|r| r.get("payload"))
        .filter(|p| p["kind"] == tools::mcp_tool::MCP_APP_KIND)
    else {
        return error(-32602, "no interactive result for this message");
    };
    let server_id = view["serverId"].as_str().unwrap_or("").to_string();
    info!(chat = chat_id, call = call_id, server = %server_id, method, "view request");
    match method {
        "tools/call" => call_tool(state, &session_key, &server_id, &params).await,
        "ui/message" => {
            let text = message_text(&params);
            if text.trim().is_empty() {
                return error(-32602, "Invalid message format");
            }
            let agent_id = types::keyparser::extract_agent_id(&session_key);
            crate::handlers::ws::dispatch_owner_message(state, &session_key, &agent_id, text, None).await;
            json!({ "result": {} })
        }
        "ui/update-model-context" => {
            state
                .harness
                .view_contexts()
                .set(&session_key, call_id, &agent::harness::view_context::update_text(&params));
            json!({ "result": {} })
        }
        _ => error(-32601, "Method not found"),
    }
}

/// The text of a `ui/message`: its text content (one block or several).
fn message_text(params: &Value) -> String {
    let content = &params["content"];
    let blocks: Vec<&Value> = match content {
        Value::Array(list) => list.iter().collect(),
        Value::Object(_) => vec![content],
        _ => Vec::new(),
    };
    blocks
        .into_iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A view's `tools/call`: a tool of its own server that views may call,
/// run through the registry's one door as the employee's call through
/// `Door::View`. The view gets the `CallToolResult` as the server sent it,
/// or, for a call that didn't run (refused, waiting on the owner's OK),
/// the reason as an error result.
async fn call_tool(state: &AppState, session_key: &str, server_id: &str, params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or("");
    let arguments = params.get("arguments").cloned().filter(|a| a.is_object()).unwrap_or_else(|| json!({}));
    let Some((def, proxy)) = state.bridge.server_tool(server_id, name).await else {
        return error(-32602, &format!("Unknown tool: {name}"));
    };
    if !def.visibility().app {
        warn!(server = server_id, tool = name, "a view called a tool its server keeps from views");
        return error(-32602, &format!("{name} can't be called from an interactive result"));
    }
    let session_id = state.harness.sessions().resolve_session_id_by_key(session_key).unwrap_or_default();
    let mut ctx = tools::ToolContext::new(tools::Origin::User).with_session(session_key, session_id);
    ctx.door = types::permissions::Door::View;
    // The arguments are the view's, the server's page, not the owner's words.
    ctx.untrusted_input = true;
    ctx.tool_call_id = format!("view-{}", uuid::Uuid::new_v4());
    info!(server = server_id, tool = name, session = session_key, "view tools/call");
    let result = state.tools.execute(&ctx, &proxy, arguments).await;
    match result.payload.as_ref().filter(|p| p["kind"] == tools::mcp_tool::MCP_RESULT_KIND) {
        Some(p) => json!({ "result": p["toolResult"] }),
        None => json!({ "result": { "content": [{ "type": "text", "text": result.content }], "isError": true } }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_messages_text_is_its_text_blocks() {
        assert_eq!(message_text(&json!({"role": "user", "content": {"type": "text", "text": "Book it"}})), "Book it");
        assert_eq!(
            message_text(&json!({"content": [{"type": "text", "text": "a"}, {"type": "image"}, {"type": "text", "text": "b"}]})),
            "a\nb"
        );
        assert_eq!(message_text(&json!({})), "");
    }

    #[tokio::test]
    async fn the_proxy_page_carries_the_views_policy_as_its_header() {
        let declared = r#"{"connectDomains":["https://api.example.com","http://localhost:27895"]}"#;
        let resp = sandbox_page(Query(SandboxQuery { csp: Some(declared.into()) })).await;
        let policy = resp.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().to_string();
        assert!(policy.starts_with("default-src 'none'; "), "{policy}");
        assert!(policy.contains("connect-src 'self' https://api.example.com;"), "{policy}");
        assert!(!policy.contains("localhost:27895"), "{policy}");
        assert!(policy.ends_with(FRAME_ANCESTORS), "{policy}");

        let none = sandbox_page(Query(SandboxQuery { csp: None })).await;
        let policy = none.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().to_string();
        assert!(policy.contains("connect-src 'none'"), "{policy}");

        let bad = sandbox_page(Query(SandboxQuery { csp: Some("x; script-src *".into()) })).await;
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    }
}
