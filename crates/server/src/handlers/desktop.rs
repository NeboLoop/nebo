//! GET /ws/desktop — the live view of the bot's computer.
//!
//! A thin byte pipe between the browser (noVNC speaking RFB over binary
//! WebSocket frames) and the session's loopback x11vnc. The desktop starts
//! on demand when the first viewer connects; each connection holds a
//! ViewerGuard so the idle reaper leaves a watched desktop alone. Auth is
//! the same trust gate as /ws: locally that's the trusted-origin check, and
//! through the tunnel neboai.com has already authenticated the caller and
//! stripped the Origin header.

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

/// POST /api/v1/desktop/teach/stop — finalize the recording. Returns the
/// artifact locations; the caller (chat) hands them to the agent to study.
pub async fn teach_stop(State(_state): State<AppState>) -> Response {
    match tools::desktop_session::stop_recording().await {
        Ok((id, dir, keyframes)) => axum::Json(serde_json::json!({
            "sessionId": id,
            "dir": dir.to_string_lossy(),
            "keyframes": keyframes,
        }))
        .into_response(),
        Err(e) => teach_err(&e),
    }
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
