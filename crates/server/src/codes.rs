//! Code interception and dispatch for NeboLoop marketplace codes.
//!
//! Detects NEBO/SKILL/TOOL/WORK/ROLE/LOOP codes in chat prompts, handles them
//! before the prompt reaches the agent runner, and broadcasts results to the client.

use std::collections::HashMap;

use tracing::{info, warn};

use comm::api::NeboLoopApi;
use types::NeboError;

use crate::state::AppState;

// ── Code Detection ──────────────────────────────────────────────────

/// The type of a marketplace/connection code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeType {
    Nebo,
    Skill,
    Tool,
    Work,
    Role,
    Loop,
}

/// Crockford Base32 charset (no I, L, O, U).
const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

fn is_crockford_base32(s: &str) -> bool {
    s.bytes().all(|b| CROCKFORD.contains(&b))
}

/// Detect if a prompt is exactly a marketplace code.
///
/// Code format: `PREFIX-XXXX-XXXX` where XXXX = 4 Crockford Base32 characters.
pub fn detect_code(prompt: &str) -> Option<(CodeType, &str)> {
    let trimmed = prompt.trim();
    let upper = trimmed.to_ascii_uppercase();

    // Must match PREFIX-XXXX-XXXX exactly
    let (prefix, code_type) = if upper.starts_with("NEBO-") {
        ("NEBO-", CodeType::Nebo)
    } else if upper.starts_with("SKILL-") {
        ("SKILL-", CodeType::Skill)
    } else if upper.starts_with("TOOL-") {
        ("TOOL-", CodeType::Tool)
    } else if upper.starts_with("WORK-") {
        ("WORK-", CodeType::Work)
    } else if upper.starts_with("ROLE-") {
        ("ROLE-", CodeType::Role)
    } else if upper.starts_with("LOOP-") {
        ("LOOP-", CodeType::Loop)
    } else {
        return None;
    };

    let rest = &upper[prefix.len()..];
    // Must be exactly XXXX-XXXX (9 chars: 4 + 1 + 4)
    if rest.len() != 9 {
        return None;
    }

    let parts: Vec<&str> = rest.split('-').collect();
    if parts.len() != 2 || parts[0].len() != 4 || parts[1].len() != 4 {
        return None;
    }
    if !is_crockford_base32(parts[0]) || !is_crockford_base32(parts[1]) {
        return None;
    }

    // Return the original trimmed input (preserving case as entered)
    Some((code_type, trimmed))
}

// ── Code Dispatch ───────────────────────────────────────────────────

/// Handle a detected code: broadcast processing event, dispatch to handler, broadcast result.
pub async fn handle_code(state: &AppState, code_type: CodeType, code: &str, session_id: &str) {
    state.hub.broadcast(
        "code_processing",
        serde_json::json!({
            "session_id": session_id,
            "code": code,
            "code_type": format!("{:?}", code_type),
        }),
    );

    let result = match code_type {
        CodeType::Nebo => handle_nebo_code(state, code).await,
        CodeType::Skill => handle_skill_code(state, code).await,
        CodeType::Tool => handle_tool_code(state, code).await,
        CodeType::Work => handle_work_code(state, code).await,
        CodeType::Role => handle_role_code(state, code).await,
        CodeType::Loop => handle_loop_code(state, code).await,
    };

    match result {
        Ok(msg) => {
            state.hub.broadcast(
                "code_result",
                serde_json::json!({
                    "session_id": session_id,
                    "code": code,
                    "success": true,
                    "message": msg,
                }),
            );
        }
        Err(e) => {
            warn!(code = code, error = %e, "code handling failed");
            state.hub.broadcast(
                "code_result",
                serde_json::json!({
                    "session_id": session_id,
                    "code": code,
                    "success": false,
                    "error": e.to_string(),
                }),
            );
        }
    }

    // Always send chat_complete so frontend resets loading state
    state.hub.broadcast(
        "chat_complete",
        serde_json::json!({ "session_id": session_id }),
    );
}

// ── Per-Type Handlers ───────────────────────────────────────────────

async fn handle_nebo_code(state: &AppState, code: &str) -> Result<String, NeboError> {
    let bot_id = redeem_nebo_code(state, code).await?;
    Ok(format!("Connected to NeboLoop (bot: {})", &bot_id[..8]))
}

async fn handle_skill_code(state: &AppState, code: &str) -> Result<String, NeboError> {
    let api = build_api_client(state)?;
    let resp = api
        .install_skill(code)
        .await
        .map_err(|e| NeboError::Internal(format!("install_skill: {e}")))?;
    let name = resp
        .skill
        .as_ref()
        .map(|s| s.name.as_str())
        .unwrap_or("skill");
    Ok(format!("Installed skill: {}", name))
}

async fn handle_tool_code(state: &AppState, code: &str) -> Result<String, NeboError> {
    let api = build_api_client(state)?;
    let resp = api
        .install_app(code)
        .await
        .map_err(|e| NeboError::Internal(format!("install_app: {e}")))?;

    let name = resp
        .app
        .as_ref()
        .map(|a| a.name.as_str())
        .unwrap_or("tool");

    // Download and install the .napp package
    let download_url = resp.download_url(api.api_server(), &format!("/api/v1/apps/{}/download", code));
    let registry = state.napp_registry.clone();
    tokio::spawn(async move {
        match registry.install_from_url(&download_url).await {
            Ok(tool_id) => info!(tool_id = %tool_id, "napp installed from code"),
            Err(e) => warn!(error = %e, "napp install from code failed"),
        }
    });

    Ok(format!("Installing tool: {}", name))
}

async fn handle_work_code(state: &AppState, code: &str) -> Result<String, NeboError> {
    let api = build_api_client(state)?;
    let resp = api
        .install_workflow(code)
        .await
        .map_err(|e| NeboError::Internal(format!("install_workflow: {e}")))?;
    Ok(format!("Installed workflow: {}", resp.id))
}

async fn handle_role_code(state: &AppState, code: &str) -> Result<String, NeboError> {
    let api = build_api_client(state)?;
    let resp = api
        .install_role(code)
        .await
        .map_err(|e| NeboError::Internal(format!("install_role: {e}")))?;
    Ok(format!("Installed role: {}", resp.id))
}

async fn handle_loop_code(state: &AppState, code: &str) -> Result<String, NeboError> {
    let api = build_api_client(state)?;
    let resp = api
        .join_loop(code)
        .await
        .map_err(|e| NeboError::Internal(format!("join_loop: {e}")))?;
    Ok(format!("Joined loop: {}", resp.name))
}

// ── API Client Helper ───────────────────────────────────────────────

fn build_api_client(state: &AppState) -> Result<NeboLoopApi, NeboError> {
    let bot_id = config::read_bot_id()
        .ok_or_else(|| NeboError::Internal("no bot_id configured".into()))?;
    let profiles = state
        .store
        .list_active_auth_profiles_by_provider("neboloop")
        .unwrap_or_default();
    let profile = profiles
        .first()
        .ok_or_else(|| NeboError::Internal("not connected to NeboLoop".into()))?;
    let api_server = state.config.neboloop.api_url.clone();
    Ok(NeboLoopApi::new(api_server, bot_id, profile.api_key.clone()))
}

// ── Shared Helpers ──────────────────────────────────────────────────

/// Activate the NeboLoop connection using stored credentials.
///
/// This is the canonical implementation — called by both startup auto-connect
/// and code handlers. Builds config from stored credentials and connects.
pub async fn activate_neboloop(state: &AppState) -> Result<(), NeboError> {
    let bot_id = config::read_bot_id()
        .ok_or_else(|| NeboError::Internal("no bot_id".into()))?;
    let profiles = state
        .store
        .list_active_auth_profiles_by_provider("neboloop")
        .unwrap_or_default();
    let token = profiles
        .first()
        .map(|p| p.api_key.clone())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| NeboError::Internal("no NeboLoop credentials".into()))?;

    let mut config = HashMap::new();
    config.insert("gateway".into(), state.config.neboloop.comms_url.clone());
    config.insert("api_server".into(), state.config.neboloop.api_url.clone());
    config.insert("bot_id".into(), bot_id);
    config.insert("token".into(), token);

    state
        .comm_manager
        .set_active("neboloop")
        .await
        .map_err(|e| NeboError::Internal(format!("set_active: {e}")))?;
    state
        .comm_manager
        .connect_active(config)
        .await
        .map_err(|e| NeboError::Internal(format!("connect: {e}")))?;

    state.hub.broadcast(
        "settings_updated",
        serde_json::json!({"commEnabled": true}),
    );
    Ok(())
}

/// Core NEBO code redemption logic. Called by both:
/// - `handle_nebo_code()` (chat-based code interception)
/// - `connect_handler()` (HTTP POST /neboloop/connect)
pub async fn redeem_nebo_code(state: &AppState, code: &str) -> Result<String, NeboError> {
    let bot_id = config::ensure_bot_id();
    let api_server = state.config.neboloop.api_url.clone();

    // 1. Redeem code (pre-auth, standalone)
    let resp = comm::api::redeem_code(&api_server, code, "nebo-rs", "desktop", &bot_id)
        .await
        .map_err(|e| NeboError::Internal(format!("redeem failed: {e}")))?;

    // 2. Store bot_id + connection token
    if let Err(e) = config::write_bot_id(&bot_id) {
        warn!("failed to persist bot_id: {}", e);
    }

    // Store connection token as auth profile
    crate::handlers::neboloop::store_neboloop_profile(
        state,
        &api_server,
        &resp.id,      // owner_id from redeem response
        &resp.name,     // use bot name as display identifier
        &resp.connection_token,
        "",             // no refresh token from code redemption
        false,          // not a janus provider
    )
    .map_err(|e| NeboError::Internal(format!("store profile: {e}")))?;

    // 3. Activate connection
    activate_neboloop(state).await?;

    Ok(bot_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_code_valid() {
        assert!(matches!(detect_code("NEBO-A1B2-C3D4"), Some((CodeType::Nebo, _))));
        assert!(matches!(detect_code("SKILL-0000-ZZZZ"), Some((CodeType::Skill, _))));
        assert!(matches!(detect_code("TOOL-ABCD-EF01"), Some((CodeType::Tool, _))));
        assert!(matches!(detect_code("WORK-1234-5678"), Some((CodeType::Work, _))));
        assert!(matches!(detect_code("ROLE-9999-AAAA"), Some((CodeType::Role, _))));
        assert!(matches!(detect_code("LOOP-QRST-VWXY"), Some((CodeType::Loop, _))));
    }

    #[test]
    fn test_detect_code_case_insensitive() {
        assert!(matches!(detect_code("nebo-a1b2-c3d4"), Some((CodeType::Nebo, _))));
        assert!(matches!(detect_code("Skill-0000-ZZZZ"), Some((CodeType::Skill, _))));
    }

    #[test]
    fn test_detect_code_trimmed() {
        assert!(matches!(detect_code("  NEBO-A1B2-C3D4  "), Some((CodeType::Nebo, _))));
    }

    #[test]
    fn test_detect_code_invalid() {
        // Wrong format
        assert!(detect_code("NEBO-A1B2").is_none());
        assert!(detect_code("NEBO-A1B2-C3D4-EXTRA").is_none());
        assert!(detect_code("INVALID-A1B2-C3D4").is_none());
        assert!(detect_code("hello world").is_none());
        assert!(detect_code("").is_none());
        // Invalid Crockford chars (I, L, O, U excluded)
        assert!(detect_code("NEBO-IIIL-OOOU").is_none());
    }
}
