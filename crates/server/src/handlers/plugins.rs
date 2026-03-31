//! Plugin handlers — listing installed plugins and authentication.
//!
//! Plugins that require credentials (e.g., GWS needing Google OAuth) declare
//! auth requirements in their manifest. These handlers run the plugin's own
//! auth CLI commands and report status via WebSocket events.

use std::collections::HashMap;

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

        plugins.push(serde_json::json!({
            "slug": slug,
            "version": version.to_string(),
            "name": manifest.as_ref().map(|m| m.name.as_str()).unwrap_or(slug.as_str()),
            "description": manifest.as_ref().map(|m| m.description.as_str()).unwrap_or(""),
            "author": manifest.as_ref().map(|m| m.author.as_str()).unwrap_or(""),
            "hasAuth": has_auth,
            "authLabel": auth_label,
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

        // Read stderr/stdout in chunks — gws may not flush with a trailing newline,
        // so line-based reading blocks. Instead we read chunks and scan for URLs.
        let stderr_task = tokio::spawn(async move {
            let mut all = String::new();
            let mut opened = false;
            if let Some(mut stream) = stderr_handle {
                let mut buf = [0u8; 4096];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 { break; }
                    let chunk = String::from_utf8_lossy(&buf[..n]);
                    info!(plugin = %slug_for_stderr, chunk = %chunk, "plugin auth stderr");
                    all.push_str(&chunk);
                    if !opened {
                        if let Some(url) = extract_url(&all) {
                            info!(plugin = %slug_for_stderr, url = %url, "opening OAuth URL in browser");
                            if let Err(e) = open::that(&url) {
                                warn!(plugin = %slug_for_stderr, error = %e, "failed to open browser");
                            }
                            opened = true;
                        }
                    }
                }
            }
            all
        });

        let slug_for_stdout = slug_owned.clone();
        let stdout_task = tokio::spawn(async move {
            let mut all = String::new();
            let mut opened = false;
            if let Some(mut stream) = stdout_handle {
                let mut buf = [0u8; 4096];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 { break; }
                    let chunk = String::from_utf8_lossy(&buf[..n]);
                    info!(plugin = %slug_for_stdout, chunk = %chunk, "plugin auth stdout");
                    all.push_str(&chunk);
                    if !opened {
                        if let Some(url) = extract_url(&all) {
                            info!(plugin = %slug_for_stdout, url = %url, "opening OAuth URL in browser");
                            if let Err(e) = open::that(&url) {
                                warn!(plugin = %slug_for_stdout, error = %e, "failed to open browser");
                            }
                            opened = true;
                        }
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

/// Extract the first HTTP(S) URL from accumulated output text.
///
/// Only returns a URL that is bounded by whitespace on both sides, so we never
/// match a partial URL when the output arrives in multiple chunks.
fn extract_url(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        let trimmed = word.trim_matches(|c: char| c == '"' || c == '\'' || c == '<' || c == '>');
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            // Only return if this isn't the last token — the last token might be
            // a partial URL still being written. Exception: if the text ends with
            // whitespace, the last token is complete.
            let is_last = i == words.len() - 1;
            if !is_last || text.ends_with(char::is_whitespace) {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}
