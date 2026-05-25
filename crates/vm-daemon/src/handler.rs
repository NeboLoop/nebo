//! RPC request handler — the guest daemon's main loop.
//!
//! Reads length-prefixed JSON requests from the host, dispatches to
//! the appropriate handler, and writes responses back.

use crate::process::{ProcessManager, SpawnParams};
use crate::wire::{self, Event, Request, Response};
use base64::Engine;
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Main handler loop — reads requests from `reader`, writes responses + events to `writer`.
pub async fn run<R, W>(reader: R, writer: W) -> Result<(), Box<dyn std::error::Error>>
where
    R: AsyncReadExt + Unpin + Send + 'static,
    W: AsyncWriteExt + Unpin + Send + 'static,
{
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Event>();
    let mut process_manager = ProcessManager::new(event_tx);

    let writer = std::sync::Arc::new(tokio::sync::Mutex::new(writer));

    // Send ready event
    {
        let mut w = writer.lock().await;
        wire::write_message(&mut *w, &Event::ready()).await?;
    }
    info!("guest daemon ready, waiting for requests");

    let mut reader = reader;

    // Event forwarding task
    let writer_clone = writer.clone();
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let mut w = writer_clone.lock().await;
            if let Err(e) = wire::write_message(&mut *w, &event).await {
                error!(%e, "failed to send event to host");
                break;
            }
        }
    });

    // Request handling loop
    loop {
        let msg = match wire::read_message(&mut reader).await {
            Ok(msg) => msg,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                info!("host connection closed");
                break;
            }
            Err(e) => {
                error!(%e, "error reading request");
                break;
            }
        };

        let req: Request = match serde_json::from_value(msg) {
            Ok(r) => r,
            Err(e) => {
                warn!(%e, "invalid request format");
                continue;
            }
        };

        tracing::debug!(method = %req.method, id = req.id, "handling request");

        let response = dispatch(&mut process_manager, &req).await;

        let mut w = writer.lock().await;
        wire::write_message(&mut *w, &response).await?;
    }

    Ok(())
}

/// Dispatch a request to the appropriate handler.
async fn dispatch(pm: &mut ProcessManager, req: &Request) -> Response {
    let params = req.params.clone().unwrap_or(serde_json::Value::Null);

    match req.method.as_str() {
        "spawn" => handle_spawn(pm, req.id, params).await,
        "kill" => handle_kill(pm, req.id, params).await,
        "writeStdin" => handle_write_stdin(req.id, params).await,
        "isProcessRunning" => handle_is_running(pm, req.id, params).await,
        "readFile" => handle_read_file(req.id, params).await,
        "writeFile" => handle_write_file(req.id, params).await,
        "listDir" => handle_list_dir(req.id, params).await,
        "copyOut" => handle_copy_out(req.id, params).await,
        "deleteSessionDirs" => handle_delete_sessions(req.id, params).await,
        _ => Response::err(req.id, format!("unknown method: {}", req.method)),
    }
}

// ── Handlers ───────────────────────────────────────────────────────

async fn handle_spawn(pm: &mut ProcessManager, id: u64, params: serde_json::Value) -> Response {
    let spawn_params: SpawnParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::err(id, format!("invalid spawn params: {e}")),
    };

    match pm.spawn(spawn_params).await {
        Ok(process_id) => Response::ok(id, serde_json::json!({ "process_id": process_id })),
        Err(e) => Response::err(id, e),
    }
}

async fn handle_kill(pm: &mut ProcessManager, id: u64, params: serde_json::Value) -> Response {
    let process_id = params
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let signal = params
        .get("signal")
        .and_then(|v| v.as_str())
        .unwrap_or("TERM");

    match pm.kill(process_id, signal) {
        Ok(()) => Response::ok(id, serde_json::json!({})),
        Err(e) => Response::err(id, e),
    }
}

async fn handle_write_stdin(id: u64, _params: serde_json::Value) -> Response {
    // TODO: implement stdin writing via process manager
    Response::ok(id, serde_json::json!({}))
}

async fn handle_is_running(
    pm: &mut ProcessManager,
    id: u64,
    params: serde_json::Value,
) -> Response {
    let process_id = params
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let running = pm.is_running(process_id);
    Response::ok(id, serde_json::json!({ "running": running }))
}

async fn handle_read_file(id: u64, params: serde_json::Value) -> Response {
    let path = match params.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return Response::err(id, "missing path parameter"),
    };

    // Security: only allow reading from /sessions/ and /tmp/
    if !path.starts_with("/sessions/") && !path.starts_with("/tmp/") {
        return Response::err(id, format!("access denied: {path}"));
    }

    match std::fs::read_to_string(path) {
        Ok(content) => Response::ok(id, serde_json::json!({ "content": content })),
        Err(e) => Response::err(id, format!("read failed: {e}")),
    }
}

async fn handle_write_file(id: u64, params: serde_json::Value) -> Response {
    let path = match params.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return Response::err(id, "missing path parameter"),
    };
    let content = match params.get("content").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return Response::err(id, "missing content parameter"),
    };
    let append = params
        .get("append")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Security: only allow writing to /sessions/ and /tmp/
    if !path.starts_with("/sessions/") && !path.starts_with("/tmp/") {
        return Response::err(id, format!("write denied: {path}"));
    }

    // Create parent directories
    if let Some(parent) = Path::new(path).parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Response::err(id, format!("mkdir failed: {e}"));
        }
    }

    let result = if append {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .and_then(|mut f| f.write_all(content.as_bytes()))
    } else {
        std::fs::write(path, content)
    };

    match result {
        Ok(()) => Response::ok(id, serde_json::json!({})),
        Err(e) => Response::err(id, format!("write failed: {e}")),
    }
}

async fn handle_list_dir(id: u64, params: serde_json::Value) -> Response {
    let path = match params.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return Response::err(id, "missing path parameter"),
    };

    // Security: only allow listing /sessions/ and /tmp/
    if !path.starts_with("/sessions/") && !path.starts_with("/tmp/") {
        return Response::err(id, format!("access denied: {path}"));
    }

    match std::fs::read_dir(path) {
        Ok(entries) => {
            let mut files: Vec<serde_json::Value> = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                files.push(serde_json::json!({
                    "name": name,
                    "is_dir": is_dir,
                    "size": size,
                }));
            }
            Response::ok(id, serde_json::json!({ "entries": files }))
        }
        Err(e) => Response::err(id, format!("listdir failed: {e}")),
    }
}

async fn handle_copy_out(id: u64, params: serde_json::Value) -> Response {
    let src_paths: Vec<String> = match params.get("src_paths") {
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(p) => p,
            Err(e) => return Response::err(id, format!("invalid src_paths: {e}")),
        },
        None => return Response::err(id, "missing src_paths parameter"),
    };

    let mut files = Vec::new();
    let mut errors: Vec<serde_json::Value> = Vec::new();

    for src in &src_paths {
        // Security: only allow copying from /sessions/ and /tmp/
        if !src.starts_with("/sessions/") && !src.starts_with("/tmp/") {
            errors.push(serde_json::json!([src, "access denied"]));
            continue;
        }

        let path = Path::new(src);
        if path.is_dir() {
            // Recursively collect files
            match collect_files_recursive(path, src) {
                Ok(entries) => files.extend(entries),
                Err(e) => {
                    errors.push(serde_json::json!([src, e]));
                }
            }
        } else if path.is_file() {
            match read_file_as_base64(path) {
                Ok((content, size)) => {
                    files.push(serde_json::json!({
                        "path": src,
                        "content_base64": content,
                        "size_bytes": size,
                    }));
                }
                Err(e) => {
                    errors.push(serde_json::json!([src, e]));
                }
            }
        } else {
            errors.push(serde_json::json!([src, "not found"]));
        }
    }

    Response::ok(
        id,
        serde_json::json!({
            "files": files,
            "errors": errors,
        }),
    )
}

async fn handle_delete_sessions(id: u64, params: serde_json::Value) -> Response {
    let names: Vec<String> = match params.get("names") {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => return Response::err(id, "missing names parameter"),
    };

    let mut deleted = Vec::new();
    let mut errors: Vec<(String, String)> = Vec::new();

    for name in &names {
        let path = format!("/sessions/{name}");
        match std::fs::remove_dir_all(&path) {
            Ok(()) => deleted.push(name.clone()),
            Err(e) => errors.push((name.clone(), e.to_string())),
        }
    }

    Response::ok(
        id,
        serde_json::json!({
            "deleted": deleted,
            "errors": errors,
        }),
    )
}

// ── Helpers ────────────────────────────────────────────────────────

fn read_file_as_base64(path: &Path) -> Result<(String, u64), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let size = bytes.len() as u64;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok((encoded, size))
}

fn collect_files_recursive(
    dir: &Path,
    base: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let mut files = Vec::new();

    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let sub = collect_files_recursive(&path, base)?;
            files.extend(sub);
        } else if path.is_file() {
            match read_file_as_base64(&path) {
                Ok((content, size)) => {
                    files.push(serde_json::json!({
                        "path": path.to_string_lossy(),
                        "content_base64": content,
                        "size_bytes": size,
                    }));
                }
                Err(e) => {
                    // Skip files we can't read (permissions, etc.)
                    tracing::warn!(path = %path.display(), %e, "skipping unreadable file");
                }
            }
        }
    }

    Ok(files)
}
