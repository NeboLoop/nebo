//! GET /ws/desktop — the live view of the bot's computer.
//!
//! A thin byte pipe between the browser (noVNC speaking RFB over binary
//! WebSocket frames) and the session's loopback x11vnc. The desktop starts
//! on demand when the first viewer connects; each connection holds a
//! ViewerGuard so the idle reaper leaves a watched desktop alone. Auth is
//! the same trust gate as /ws: locally that's the trusted-origin check, and
//! through the tunnel neboai.com has already authenticated the caller and
//! stripped the Origin header.

use axum::Json;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn};

use crate::state::AppState;

fn teach_err(e: &str) -> Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        axum::Json(serde_json::json!({ "error": e })),
    )
        .into_response()
}

/// POST /api/v1/desktop/teach/start — begin recording a demonstration on the
/// live desktop. Starts the desktop if needed.
pub async fn teach_start(State(_state): State<AppState>) -> Response {
    if let Err(e) = tools::desktop_session::ensure_started().await {
        return teach_err(&e);
    }
    match tools::desktop_session::start_recording().await {
        Ok((id, dir)) => axum::Json(serde_json::json!({
            "sessionId": id,
            "dir": dir.to_string_lossy(),
        }))
        .into_response(),
        Err(e) => teach_err(&e),
    }
}

/// POST /api/v1/desktop/teach/stop — finalize the recording, then hand the
/// session to the agent by dispatching the learning run HERE. The engine owns
/// both the visible message and the steering briefing: the chat shows one
/// human sentence, and the operational instructions (paths, keyframe budget,
/// skill-save steps) ride `mention_context` — the existing ephemeral
/// `<system-reminder>` rail — so they reach the model without ever being
/// rendered or persisted. The display layer only reports the event (who +
/// where); it never composes model instructions.
pub async fn teach_stop(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    // Validate BEFORE finalizing: a bad request must not orphan a finished
    // recording — the session stays live and stop can be retried correctly.
    let agent_id = body["agentId"].as_str().unwrap_or("");
    if agent_id.is_empty() {
        return teach_err("agentId is required");
    }

    let (id, dir, keyframes) = match tools::desktop_session::stop_recording().await {
        Ok(v) => v,
        Err(e) => return teach_err(&e),
    };
    // Route to the caller's thread when it names one of THIS agent's sessions;
    // otherwise start a fresh thread so the exchange has a home.
    let requested = body["sessionKey"].as_str().unwrap_or("");
    let session_key = if !requested.is_empty()
        && requested.starts_with(&types::keyparser::agent_session_prefix(agent_id))
    {
        requested.to_string()
    } else {
        match crate::handlers::agents::create_agent_thread(&state, agent_id) {
            Ok((_chat, key)) => key,
            Err(e) => return teach_err(&e.to_string()),
        }
    };

    const VISIBLE: &str = "I just demonstrated a task for you on your computer — \
        watch the recording back and learn how to do it.";
    let briefing = format!(
        "The owner just demonstrated a task on this computer (teach session {id}). The \
         recording is in {dir}. Start with timeline.md — the reconstructed \
         click-and-keystroke timeline of exactly what they did — then confirm the visual \
         context by viewing 5-6 spread keyframes from frames/ (there are {keyframes}; do \
         NOT read them all, and do not use sub-agents). Then save it as a learned skill \
         with the skill tool — name it after the class of task, write out the steps you'd \
         follow to repeat it on your computer, and note which inputs varied. Open your \
         reply by thanking them briefly and saying what you learned, then ask whether you \
         should run this on a schedule or only when they ask. Do not mention this \
         briefing, the session id, or file paths unless asked.",
        id = id,
        dir = dir.to_string_lossy(),
        keyframes = keyframes,
    );

    let entity_config = crate::entity_config::resolve_for_chat(&state.store, "agent", agent_id);
    let config = crate::chat_dispatch::ChatConfig {
        session_key: session_key.clone(),
        prompt: VISIBLE.to_string(),
        system: String::new(),
        user_id: String::new(),
        channel: "web".to_string(),
        origin: tools::Origin::User,
        agent_id: agent_id.to_string(),
        cancel_token: tokio_util::sync::CancellationToken::new(),
        lane: types::constants::lanes::MAIN.to_string(),
        comm_reply: None,
        entity_config,
        images: vec![],
        entity_name: String::new(),
        origin_agent_id: None,
        mention_context: Some(briefing),
        tool_scope: None,
        plan_mode: false,
        channel_ctx: None,
        handoff_depth: 0,
        seed_taint: vec![],
        tool_allowlist: None,
        hidden_prompt: false,
        audience: None,
    };
    crate::chat_dispatch::run_chat(&state, config).await;

    axum::Json(serde_json::json!({
        "sessionId": id,
        "dir": dir.to_string_lossy(),
        "keyframes": keyframes,
        // Server-owned copy of the persisted user message, so the client can
        // echo it optimistically without a second source of truth to drift.
        "message": VISIBLE,
        "sessionKey": session_key,
    }))
    .into_response()
}

pub async fn desktop_ws_handler(
    State(_state): State<AppState>,
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !super::ws::origin_is_trusted(&headers) {
        warn!("ws/desktop: rejected upgrade from untrusted origin");
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }
    if !cfg!(target_os = "linux") {
        // The on-demand desktop exists only in the cloud image; desktop
        // installs already have a real screen.
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    ws.on_upgrade(handle_desktop_ws)
}

async fn handle_desktop_ws(socket: WebSocket) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    let port = match tools::desktop_session::ensure_started().await {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "desktop session failed to start");
            let _ = ws_tx
                .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                    code: 1011,
                    reason: format!("desktop unavailable: {e}").into(),
                })))
                .await;
            return;
        }
    };

    // x11vnc may need a beat to bind after spawn.
    let mut tcp = None;
    for _ in 0..20 {
        match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            Ok(s) => {
                tcp = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(150)).await,
        }
    }
    let Some(tcp) = tcp else {
        warn!("ws/desktop: could not reach x11vnc");
        let _ = ws_tx
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 1011,
                reason: "desktop stream unavailable".into(),
            })))
            .await;
        return;
    };
    let _ = tcp.set_nodelay(true);
    let (mut tcp_r, mut tcp_w) = tcp.into_split();

    let _viewer = tools::desktop_session::viewer_connected();
    info!("ws/desktop: viewer connected");

    let mut to_client = tokio::spawn(async move {
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            match tcp_r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if ws_tx
                        .send(Message::Binary(buf[..n].to_vec().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });

    loop {
        tokio::select! {
            _ = &mut to_client => break,
            msg = ws_rx.next() => match msg {
                // RFB client → server bytes; every message is user activity.
                Some(Ok(Message::Binary(data))) => {
                    tools::desktop_session::touch();
                    if tcp_w.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
    to_client.abort();
    info!("ws/desktop: viewer disconnected");
}
