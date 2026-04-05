//! Plugin handlers — listing installed plugins and authentication.
//!
//! Plugins that require credentials (e.g., GWS needing Google OAuth) declare
//! auth requirements in their manifest. These handlers run the plugin's own
//! auth CLI commands and report status via WebSocket events.

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::Json;
use tokio::io::AsyncReadExt;
use tracing::{info, warn};

use super::{to_error_response, HandlerResult};
use crate::state::AppState;
use types::NeboError;

/// GET /plugins
///
/// Lists all installed plugins, deduped by slug (highest version wins).
/// Enriches each entry with manifest data (name, description, author, auth info).
pub async fn list_plugins(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let installed = state.plugin_store.list_installed();

    // Dedup by slug — list_installed sorts by slug asc, version desc,
    // so first occurrence per slug is the highest version.
    let mut seen = HashMap::new();
    for (slug, version, _binary_path, source) in &installed {
        seen.entry(slug.clone()).or_insert_with(|| (version.clone(), *source));
    }

    let mut plugins = Vec::new();
    for (slug, (version, source)) in &seen {
        let manifest = state.plugin_store.get_manifest(slug);
        let (has_auth, auth_label) = match &manifest {
            Some(m) => match &m.auth {
                Some(auth) => (true, auth.label.clone()),
                None => (false, String::new()),
            },
            None => (false, String::new()),
        };

        let event_count = manifest
            .as_ref()
            .and_then(|m| m.events.as_ref())
            .map(|e| e.len())
            .unwrap_or(0);

        plugins.push(serde_json::json!({
            "slug": slug,
            "version": version.to_string(),
            "name": manifest.as_ref().map(|m| m.name.as_str()).unwrap_or(slug.as_str()),
            "description": manifest.as_ref().map(|m| m.description.as_str()).unwrap_or(""),
            "author": manifest.as_ref().map(|m| m.author.as_str()).unwrap_or(""),
            "hasAuth": has_auth,
            "authLabel": auth_label,
            "hasEvents": event_count > 0,
            "eventCount": event_count,
            "source": source,
        }));
    }

    plugins.sort_by(|a, b| {
        a["slug"].as_str().unwrap_or("").cmp(b["slug"].as_str().unwrap_or(""))
    });

    let total = plugins.len();
    Ok(Json(serde_json::json!({
        "plugins": plugins,
        "total": total,
    })))
}

/// POST /plugins/{slug}/auth/login
///
/// Spawns the plugin's auth login command in the background. Returns immediately.
/// Broadcasts `plugin_auth_complete` or `plugin_auth_error` via WebSocket when done.
pub async fn auth_login(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let (binary_path, auth) = state
        .plugin_store
        .get_auth_info(&slug)
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;

    let hub = state.hub.clone();
    let slug_owned = slug.clone();
    let plugin_path = state.plugin_store.path_with_plugins();

    info!(plugin = %slug, "starting plugin auth login");

    hub.broadcast(
        "plugin_auth_started",
        serde_json::json!({
            "plugin": &slug,
            "label": &auth.label,
        }),
    );

    // Spawn background task — auth login may take minutes (user authorizes in browser).
    // gws writes the OAuth URL to stderr, so we read both streams and open the URL
    // with open::that(), mirroring how onboarding opens the browser.
    tokio::spawn(async move {
        let args: Vec<&str> = auth.commands.login.split_whitespace().collect();
        let mut cmd = tokio::process::Command::new(&binary_path);
        cmd.args(&args);
        cmd.env("PATH", &plugin_path);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        for (key, value) in &auth.env {
            cmd.env(key, value);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                warn!(plugin = %slug_owned, error = %e, "plugin auth login command failed");
                hub.broadcast(
                    "plugin_auth_error",
                    serde_json::json!({
                        "plugin": &slug_owned,
                        "error": e.to_string(),
                    }),
                );
                return;
            }
        };

        // Read stderr lines for OAuth URLs — gws writes to stderr, not stdout.
        let stderr_handle = child.stderr.take();
        let stdout_handle = child.stdout.take();
        let slug_for_stderr = slug_owned.clone();
        let hub_for_stderr = hub.clone();

        // Read stderr/stdout in chunks, scanning for OAuth URLs. When found,
        // broadcast via WebSocket so the frontend can open the browser (primary),
        // and also try open::that() as a server-side fallback.
        let stderr_task = tokio::spawn(async move {
            let mut all = String::new();
            let mut opened = false;
            if let Some(mut stream) = stderr_handle {
                let mut buf = [0u8; 4096];
                loop {
                    let has_candidate = !opened && has_url_candidate(&all);
                    let read_result = if has_candidate {
                        match tokio::time::timeout(
                            Duration::from_secs(1),
                            stream.read(&mut buf),
                        ).await {
                            Ok(r) => r,
                            Err(_) => {
                                // Timeout — no more data coming, treat URL as complete.
                                if let Some(url) = extract_url(&all, true) {
                                    open_auth_url(&slug_for_stderr, &url, &hub_for_stderr);
                                    opened = true;
                                }
                                continue;
                            }
                        }
                    } else {
                        stream.read(&mut buf).await
                    };
                    match read_result {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = String::from_utf8_lossy(&buf[..n]);
                            info!(plugin = %slug_for_stderr, chunk = %chunk, "plugin auth stderr");
                            all.push_str(&chunk);
                            if !opened {
                                if let Some(url) = extract_url(&all, false) {
                                    open_auth_url(&slug_for_stderr, &url, &hub_for_stderr);
                                    opened = true;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            all
        });

        let slug_for_stdout = slug_owned.clone();
        let hub_for_stdout = hub.clone();
        let stdout_task = tokio::spawn(async move {
            let mut all = String::new();
            let mut opened = false;
            if let Some(mut stream) = stdout_handle {
                let mut buf = [0u8; 4096];
                loop {
                    let has_candidate = !opened && has_url_candidate(&all);
                    let read_result = if has_candidate {
                        match tokio::time::timeout(
                            Duration::from_secs(1),
                            stream.read(&mut buf),
                        ).await {
                            Ok(r) => r,
                            Err(_) => {
                                if let Some(url) = extract_url(&all, true) {
                                    open_auth_url(&slug_for_stdout, &url, &hub_for_stdout);
                                    opened = true;
                                }
                                continue;
                            }
                        }
                    } else {
                        stream.read(&mut buf).await
                    };
                    match read_result {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = String::from_utf8_lossy(&buf[..n]);
                            info!(plugin = %slug_for_stdout, chunk = %chunk, "plugin auth stdout");
                            all.push_str(&chunk);
                            if !opened {
                                if let Some(url) = extract_url(&all, false) {
                                    open_auth_url(&slug_for_stdout, &url, &hub_for_stdout);
                                    opened = true;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            all
        });

        let (stderr_output, stdout_output) = tokio::join!(stderr_task, stdout_task);
        let all_stderr = stderr_output.unwrap_or_default();
        let all_stdout = stdout_output.unwrap_or_default();

        match child.wait().await {
            Ok(status) if status.success() => {
                info!(plugin = %slug_owned, "plugin auth login succeeded");
                hub.broadcast(
                    "plugin_auth_complete",
                    serde_json::json!({ "plugin": &slug_owned }),
                );
            }
            Ok(_status) => {
                let error = if all_stderr.trim().is_empty() {
                    all_stdout.trim().to_string()
                } else {
                    all_stderr.trim().to_string()
                };
                warn!(plugin = %slug_owned, error = %error, "plugin auth login failed");
                hub.broadcast(
                    "plugin_auth_error",
                    serde_json::json!({
                        "plugin": &slug_owned,
                        "error": error,
                    }),
                );
            }
            Err(e) => {
                warn!(plugin = %slug_owned, error = %e, "plugin auth login command failed");
                hub.broadcast(
                    "plugin_auth_error",
                    serde_json::json!({
                        "plugin": &slug_owned,
                        "error": e.to_string(),
                    }),
                );
            }
        }
    });

    Ok(Json(serde_json::json!({ "started": true })))
}

/// POST /plugins/{slug}/auth/logout
///
/// Runs the plugin's auth logout command. Returns immediately.
pub async fn auth_logout(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let (binary_path, auth) = state
        .plugin_store
        .get_auth_info(&slug)
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;

    let logout_cmd = auth.commands.logout.as_deref().ok_or_else(|| {
        to_error_response(NeboError::Validation(
            "plugin has no auth logout command".into(),
        ))
    })?;

    let args: Vec<&str> = logout_cmd.split_whitespace().collect();
    let mut cmd = tokio::process::Command::new(&binary_path);
    cmd.args(&args);
    cmd.env("PATH", state.plugin_store.path_with_plugins());
    for (key, value) in &auth.env {
        cmd.env(key, value);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| to_error_response(NeboError::Internal(e.to_string())))?;

    if output.status.success() {
        info!(plugin = %slug, "plugin auth logout succeeded");
        Ok(Json(serde_json::json!({ "success": true })))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(plugin = %slug, error = %stderr, "plugin auth logout failed");
        Err(to_error_response(NeboError::Internal(
            format!("logout failed: {}", stderr),
        )))
    }
}

/// DELETE /plugins/{slug}
///
/// Removes a plugin and all its versions from disk.
pub async fn remove_plugin(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .plugin_store
        .remove(&slug)
        .map_err(|e| to_error_response(NeboError::Internal(e.to_string())))?;

    info!(plugin = %slug, "plugin removed via settings");
    Ok(Json(serde_json::json!({ "message": "Plugin removed" })))
}

/// GET /plugins/{slug}/auth/status
///
/// Runs the plugin's auth status command. Exit code 0 = authenticated.
/// Returns `{ "authenticated": bool }` plus any JSON the plugin outputs.
pub async fn auth_status(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let (binary_path, auth) = state
        .plugin_store
        .get_auth_info(&slug)
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;

    let status_cmd = auth.commands.status.as_deref().ok_or_else(|| {
        to_error_response(NeboError::Validation(
            "plugin has no auth status command".into(),
        ))
    })?;

    let args: Vec<&str> = status_cmd.split_whitespace().collect();
    let mut cmd = tokio::process::Command::new(&binary_path);
    cmd.args(&args);
    cmd.env("PATH", state.plugin_store.path_with_plugins());
    for (key, value) in &auth.env {
        cmd.env(key, value);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| to_error_response(NeboError::Internal(e.to_string())))?;

    let authenticated = output.status.success();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Include any plugin-specific details if it outputs JSON
    let mut result = serde_json::json!({ "authenticated": authenticated });
    if let Ok(plugin_data) = serde_json::from_str::<serde_json::Value>(&stdout) {
        if let Some(obj) = plugin_data.as_object() {
            for (k, v) in obj {
                result[k] = v.clone();
            }
        }
    }

    Ok(Json(result))
}

/// GET /plugins/events
///
/// Lists all declared events across all installed plugins.
pub async fn list_all_plugin_events(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let installed = state.plugin_store.list_installed();

    // Dedup by slug (highest version wins).
    let mut seen = HashMap::new();
    for (slug, version, _binary_path, _source) in &installed {
        seen.entry(slug.clone()).or_insert_with(|| version.clone());
    }

    let mut events = Vec::new();
    for slug in seen.keys() {
        if let Some(event_defs) = state.plugin_store.get_events(slug) {
            for ev in &event_defs {
                events.push(serde_json::json!({
                    "plugin": slug,
                    "name": ev.name,
                    "source": format!("{}.{}", slug, ev.name),
                    "description": ev.description,
                    "multiplexed": ev.multiplexed,
                }));
            }
        }
    }

    events.sort_by(|a, b| {
        a["source"].as_str().unwrap_or("").cmp(b["source"].as_str().unwrap_or(""))
    });

    let total = events.len();
    Ok(Json(serde_json::json!({
        "events": events,
        "total": total,
    })))
}

/// GET /plugins/{slug}/events
///
/// Lists declared events for a specific plugin.
pub async fn list_plugin_events(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let event_defs = state
        .plugin_store
        .get_events(&slug)
        .unwrap_or_default();

    let events: Vec<serde_json::Value> = event_defs
        .iter()
        .map(|ev| {
            serde_json::json!({
                "name": ev.name,
                "source": format!("{}.{}", slug, ev.name),
                "description": ev.description,
                "multiplexed": ev.multiplexed,
            })
        })
        .collect();

    let total = events.len();
    Ok(Json(serde_json::json!({
        "plugin": slug,
        "events": events,
        "total": total,
    })))
}

/// Open an OAuth URL: broadcast it to the frontend via WebSocket so the
/// frontend can call `window.open()`.
fn open_auth_url(slug: &str, url: &str, hub: &super::ws::ClientHub) {
    info!(plugin = %slug, url = %url, "broadcasting plugin OAuth URL to frontend");
    hub.broadcast(
        "plugin_auth_url",
        serde_json::json!({
            "plugin": slug,
            "url": url,
        }),
    );
}

/// Returns true if the text contains a URL-like token that `extract_url(text, false)`
/// would skip because it's the last token without trailing whitespace.
fn has_url_candidate(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    if let Some(last) = words.last() {
        let trimmed = last.trim_matches(|c: char| c == '"' || c == '\'' || c == '<' || c == '>');
        (trimmed.starts_with("https://") || trimmed.starts_with("http://"))
            && !text.ends_with(char::is_whitespace)
    } else {
        false
    }
}

/// Extract the first HTTP(S) URL from accumulated output text.
///
/// When `complete` is false (streaming), only returns a URL that is followed by
/// more text or trailing whitespace — this avoids matching a partial URL that is
/// still being written. When `complete` is true (after EOF or timeout), the last
/// token is accepted unconditionally since no more data is expected.
fn extract_url(text: &str, complete: bool) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        let trimmed = word.trim_matches(|c: char| c == '"' || c == '\'' || c == '<' || c == '>');
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            let is_last = i == words.len() - 1;
            if complete || !is_last || text.ends_with(char::is_whitespace) {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}
