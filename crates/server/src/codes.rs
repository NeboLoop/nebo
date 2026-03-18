//! Code interception and dispatch for NeboLoop marketplace codes.
//!
//! Detects NEBO/SKILL/WORK/ROLE/LOOP codes in chat prompts, handles them
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
    } else if upper.starts_with("SKIL-") {
        ("SKIL-", CodeType::Skill)
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

/// Rich result from a per-type code handler.
struct CodeHandlerResult {
    message: String,
    artifact_name: Option<String>,
    checkout_url: Option<String>,
}

/// Handle a detected code: broadcast processing event, dispatch to handler, broadcast result.
pub async fn handle_code(state: &AppState, code_type: CodeType, code: &str, session_id: &str) {
    let (code_type_str, status_message) = match code_type {
        CodeType::Nebo => ("nebo", "Connecting to NeboLoop..."),
        CodeType::Skill => ("skill", "Installing skill..."),
        CodeType::Work => ("workflow", "Installing workflow..."),
        CodeType::Role => ("role", "Installing role..."),
        CodeType::Loop => ("loop", "Joining loop..."),
    };

    state.hub.broadcast(
        "code_processing",
        serde_json::json!({
            "session_id": session_id,
            "code": code,
            "code_type": code_type_str,
            "status_message": status_message,
        }),
    );

    let result = match code_type {
        CodeType::Nebo => handle_nebo_code(state, code).await,
        CodeType::Skill => handle_skill_code(state, code).await,
        CodeType::Work => handle_work_code(state, code).await,
        CodeType::Role => handle_role_code(state, code).await,
        CodeType::Loop => handle_loop_code(state, code).await,
    };

    match result {
        Ok(r) => {
            let payment_required = r.checkout_url.is_some();
            state.hub.broadcast(
                "code_result",
                serde_json::json!({
                    "session_id": session_id,
                    "code": code,
                    "code_type": code_type_str,
                    "success": true,
                    "message": r.message,
                    "artifact_name": r.artifact_name,
                    "payment_required": payment_required,
                    "checkout_url": r.checkout_url,
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
                    "code_type": code_type_str,
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

async fn handle_nebo_code(state: &AppState, code: &str) -> Result<CodeHandlerResult, NeboError> {
    let bot_id = redeem_nebo_code(state, code).await?;
    Ok(CodeHandlerResult {
        message: format!("Connected to NeboLoop (bot: {})", &bot_id[..8]),
        artifact_name: None,
        checkout_url: None,
    })
}

async fn handle_skill_code(state: &AppState, code: &str) -> Result<CodeHandlerResult, NeboError> {
    let api = build_api_client(state)?;
    let resp = api
        .install_skill(code)
        .await
        .map_err(|e| NeboError::Internal(format!("install_skill: {e}")))?;

    if resp.status == "payment_required" {
        let name = resp.artifact.name.clone();
        return Ok(CodeHandlerResult {
            message: format!("Skill requires payment: {}", name),
            artifact_name: Some(name),
            checkout_url: Some(resp.checkout_url.unwrap_or_default()),
        });
    }

    let artifact_id = resp.artifact.id.clone();
    let name = resp.artifact.name.clone();

    // Fetch artifact content from NeboLoop and persist to filesystem
    let skill_dir = match tools::persist_skill_from_api(&api, &artifact_id, &name, code).await {
        Ok(dir) => {
            info!(code, name = %name, dir = %dir.display(), "persisted skill artifact to filesystem");
            Some(dir)
        }
        Err(e) => {
            warn!(code, artifact_id = %artifact_id, error = %e, "failed to persist skill artifact after redeem");
            None
        }
    };

    // Reload skill loader so skill appears in catalog immediately
    state.skill_loader.load_all().await;

    // Cascade: resolve skill deps (tools[], dependencies[])
    if let Some(skill_dir) = skill_dir {
        let state_clone = state.clone();
        tokio::spawn(async move {
            let skill_path = skill_dir.join("SKILL.md");
            if let Ok(data) = std::fs::read(&skill_path) {
                if let Ok(skill) = tools::skills::parse_skill_md(&data) {
                    let deps = crate::deps::extract_skill_deps(&skill);
                    if !deps.is_empty() {
                        let mut visited = std::collections::HashSet::new();
                        crate::deps::resolve_cascade(&state_clone, deps, &mut visited).await;
                    }
                }
            }
        });
    }

    Ok(CodeHandlerResult {
        message: format!("Installed skill: {}", name),
        artifact_name: Some(name),
        checkout_url: None,
    })
}

async fn handle_work_code(state: &AppState, code: &str) -> Result<CodeHandlerResult, NeboError> {
    let api = build_api_client(state)?;
    let resp = api
        .install_workflow(code)
        .await
        .map_err(|e| NeboError::Internal(format!("install_workflow: {e}")))?;

    if resp.status == "payment_required" {
        let name = resp.artifact.name.clone();
        return Ok(CodeHandlerResult {
            message: format!("Workflow requires payment: {}", name),
            artifact_name: Some(name),
            checkout_url: Some(resp.checkout_url.unwrap_or_default()),
        });
    }

    let artifact_id = resp.artifact.id.clone();
    let artifact_name = resp.artifact.name.clone();

    // Fetch artifact content from NeboLoop and persist to DB + filesystem
    if let Err(e) = persist_workflow_artifact(&api, &artifact_id, &artifact_name, code, &state.store).await {
        warn!(code, error = %e, "failed to persist workflow artifact after redeem");
    }

    // Cascade: resolve workflow deps (skills, tools, sub-workflows)
    let state_clone = state.clone();
    let artifact_id_clone = artifact_id.clone();
    tokio::spawn(async move {
        if let Ok(Some(wf)) = state_clone.store.get_workflow(&artifact_id_clone) {
            if let Ok(def) = workflow::parser::parse_workflow(&wf.definition) {
                let deps = crate::deps::extract_workflow_deps(&def);
                if !deps.is_empty() {
                    let mut visited = std::collections::HashSet::new();
                    crate::deps::resolve_cascade(&state_clone, deps, &mut visited).await;
                }
            }
        }
    });

    Ok(CodeHandlerResult {
        message: format!("Installed workflow: {}", artifact_name),
        artifact_name: Some(artifact_name),
        checkout_url: None,
    })
}

async fn handle_role_code(state: &AppState, code: &str) -> Result<CodeHandlerResult, NeboError> {
    let api = build_api_client(state)?;
    let resp = api
        .install_role(code)
        .await
        .map_err(|e| NeboError::Internal(format!("install_role: {e}")))?;

    if resp.status == "payment_required" {
        let name = resp.artifact.name.clone();
        return Ok(CodeHandlerResult {
            message: format!("Role requires payment: {}", name),
            artifact_name: Some(name),
            checkout_url: Some(resp.checkout_url.unwrap_or_default()),
        });
    }

    let artifact_id = resp.artifact.id.clone();
    let artifact_name = resp.artifact.name.clone();

    // Fetch artifact content from NeboLoop and persist to DB + filesystem
    if let Err(e) = tools::persist_role_from_api(&api, &artifact_id, &artifact_name, code, &state.store).await {
        warn!(code, error = %e, "failed to persist role artifact after redeem");
    }

    // Auto-activate the role so it appears in the sidebar immediately
    if let Ok(Some(role)) = state.store.get_role(&artifact_id) {
        let config = if !role.frontmatter.is_empty() {
            napp::role::parse_role_config(&role.frontmatter).ok()
        } else {
            None
        };
        let active = tools::ActiveRole {
            role_id: artifact_id.clone(),
            name: role.name.clone(),
            role_md: role.role_md.clone(),
            config,
            channel_id: None,
        };
        state.role_registry.write().await.insert(artifact_id.clone(), active);
        state.hub.broadcast(
            "role_activated",
            serde_json::json!({ "roleId": artifact_id, "name": role.name }),
        );
    }

    // Register agent in the owner's personal loop
    {
        let st = state.clone();
        let name = artifact_name.clone();
        let slug = artifact_name.to_lowercase().replace(' ', "-");
        tokio::spawn(async move {
            if let Err(e) = register_agent_in_loop(&st, &name, &slug).await {
                warn!(role = %name, error = %e, "failed to register agent in loop");
            }
        });
    }

    // Cascade: resolve role deps (workflows, skills, tools from frontmatter)
    let state_clone = state.clone();
    let artifact_id_clone = artifact_id.clone();
    tokio::spawn(async move {
        if let Ok(Some(role)) = state_clone.store.get_role(&artifact_id_clone) {
            let deps = crate::deps::extract_role_deps_from_frontmatter(&role.frontmatter);
            if !deps.is_empty() {
                let mut visited = std::collections::HashSet::new();
                crate::deps::resolve_cascade(&state_clone, deps, &mut visited).await;
            }
        }
    });

    Ok(CodeHandlerResult {
        message: format!("Installed role: {}", artifact_name),
        artifact_name: Some(artifact_name),
        checkout_url: None,
    })
}

async fn handle_loop_code(state: &AppState, code: &str) -> Result<CodeHandlerResult, NeboError> {
    let api = build_api_client(state)?;
    let resp = api
        .join_loop(code)
        .await
        .map_err(|e| NeboError::Internal(format!("join_loop: {e}")))?;
    Ok(CodeHandlerResult {
        message: format!("Joined loop {}", resp.loop_id),
        artifact_name: Some(resp.loop_id),
        checkout_url: None,
    })
}

// ── REST Endpoint ───────────────────────────────────────────────────

/// POST /api/v1/codes — submit a marketplace code via REST.
///
/// Body: `{ "code": "SKIL-RFBM-XCYT" }`
/// Returns: `{ "success": true, "message": "Installed skill: ..." }`
pub async fn submit_code(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::response::Json(body): axum::response::Json<serde_json::Value>,
) -> Result<
    axum::response::Json<serde_json::Value>,
    (axum::http::StatusCode, axum::response::Json<types::api::ErrorResponse>),
> {
    let code = body["code"]
        .as_str()
        .ok_or_else(|| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                axum::response::Json(types::api::ErrorResponse {
                    error: "code is required".into(),
                }),
            )
        })?;

    let (code_type, validated_code) = detect_code(code).ok_or_else(|| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            axum::response::Json(types::api::ErrorResponse {
                error: format!("invalid code format: {}", code),
            }),
        )
    })?;

    let result = match code_type {
        CodeType::Nebo => handle_nebo_code(&state, validated_code).await,
        CodeType::Skill => handle_skill_code(&state, validated_code).await,
        CodeType::Work => handle_work_code(&state, validated_code).await,
        CodeType::Role => handle_role_code(&state, validated_code).await,
        CodeType::Loop => handle_loop_code(&state, validated_code).await,
    };

    match result {
        Ok(r) => Ok(axum::response::Json(serde_json::json!({
            "success": true,
            "code": validated_code,
            "codeType": format!("{:?}", code_type),
            "message": r.message,
            "artifact_name": r.artifact_name,
            "payment_required": r.checkout_url.is_some(),
            "checkout_url": r.checkout_url,
        }))),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::response::Json(types::api::ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

// ── API Client Helper ───────────────────────────────────────────────

pub(crate) fn build_api_client(state: &AppState) -> Result<NeboLoopApi, NeboError> {
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

// ── Artifact Persistence ────────────────────────────────────────────
//
// After redeem_code() registers the install in the NeboLoop cloud DB,
// these functions fetch the actual artifact content and persist locally.
//
// Skills and roles: canonical implementation in tools::persist_skill_from_api
// and tools::persist_role_from_api. Workflows have a unique DB+filesystem
// persist path that only exists here.

/// Fetch workflow content from NeboLoop and persist to DB + filesystem.
///
/// If the API provides a `downloadUrl`, downloads the sealed `.napp` archive
/// and stores it at `nebo/workflows/{slug}/{version}.napp`, then extracts it.
/// Otherwise falls back to writing loose WORKFLOW.md + workflow.json files.
async fn persist_workflow_artifact(
    api: &NeboLoopApi,
    artifact_id: &str,
    name: &str,
    code: &str,
    store: &db::Store,
) -> Result<(), String> {
    let detail = api.get_skill(artifact_id).await
        .map_err(|e| format!("fetch workflow detail: {e}"))?;

    let manifest_text = tools::extract_manifest_text(&detail)
        .unwrap_or_default();

    // For workflows, manifest is WORKFLOW.md and type_config may hold the definition
    let definition = detail.type_config
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_default())
        .unwrap_or_default();

    // Persist to DB
    let _ = store.create_workflow(
        artifact_id,
        Some(code),
        name,
        &detail.item.version,
        &definition,
        if manifest_text.is_empty() { None } else { Some(&manifest_text) },
        None,
    ).map_err(|e| format!("create_workflow: {e}"))?;

    // Marketplace artifacts go to nebo/ namespace (installed)
    let nebo_dir = config::nebo_dir()
        .map_err(|e| format!("nebo_dir: {e}"))?;
    let slug = &detail.item.slug;
    let dir_name = if slug.is_empty() { name } else { slug.as_str() };
    let version = if detail.item.version.is_empty() { "1.0.0" } else { &detail.item.version };

    // Try sealed .napp download — use API-provided URL or construct from artifact ID
    let download_url = detail.download_url.clone()
        .or_else(|| Some(format!("/api/v1/apps/{}/download", artifact_id)));
    if let Some(ref download_url) = download_url {
        let napp_dir = nebo_dir.join("workflows").join(dir_name);
        std::fs::create_dir_all(&napp_dir)
            .map_err(|e| format!("create workflow dir: {e}"))?;
        let napp_path = napp_dir.join(format!("{}.napp", version));

        match api.download_napp(download_url).await {
            Ok(data) => {
                std::fs::write(&napp_path, &data)
                    .map_err(|e| format!("write .napp: {e}"))?;
                tracing::info!(workflow = name, path = %napp_path.display(), size = data.len(), "stored sealed .napp");

                match napp::reader::extract_napp_alongside(&napp_path) {
                    Ok(extract_dir) => {
                        tracing::info!(workflow = name, dir = %extract_dir.display(), "extracted .napp");
                        // Set napp_path on DB record to the sealed archive
                        if let Err(e) = store.set_workflow_napp_path(artifact_id, &napp_path.to_string_lossy()) {
                            warn!(workflow = name, error = %e, "failed to set napp_path");
                        }
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::warn!(workflow = name, error = %e, "failed to extract .napp; falling back to loose files");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(workflow = name, error = %e, "failed to download .napp; falling back to loose files");
            }
        }
    }

    // Fallback: write loose WORKFLOW.md + workflow.json
    let wf_dir = nebo_dir.join("workflows").join(dir_name);
    std::fs::create_dir_all(&wf_dir)
        .map_err(|e| format!("create workflow dir: {e}"))?;

    if !manifest_text.is_empty() {
        if let Err(e) = std::fs::write(wf_dir.join("WORKFLOW.md"), &manifest_text) {
            warn!(workflow = name, error = %e, "failed to write WORKFLOW.md");
        }
    }
    if !definition.is_empty() {
        if let Err(e) = std::fs::write(wf_dir.join("workflow.json"), &definition) {
            warn!(workflow = name, error = %e, "failed to write workflow.json");
        }
    }

    // Set napp_path on DB record
    if let Err(e) = store.set_workflow_napp_path(artifact_id, &wf_dir.to_string_lossy()) {
        warn!(workflow = name, error = %e, "failed to set napp_path");
    }

    tracing::info!(workflow = name, dir = %wf_dir.display(), "persisted workflow artifact (loose)");
    Ok(())
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
    if let Ok(dir) = config::data_dir() {
        config.insert("data_dir".into(), dir.to_string_lossy().to_string());
    }

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
        &resp.id,               // owner_id from redeem response
        &resp.owner_email,      // owner email from redeem response
        &resp.owner_display_name, // owner display name from redeem response
        &resp.connection_token,
        "",             // no refresh token from code redemption
        false,          // not a janus provider
    )
    .map_err(|e| NeboError::Internal(format!("store profile: {e}")))?;

    // 3. Activate connection
    activate_neboloop(state).await?;

    Ok(bot_id)
}

/// Register an agent in the owner's personal loop after role install/activate.
///
/// The gateway auto-creates an agent space conversation and subscribes
/// the bot to it. Errors are non-fatal — logged by callers.
pub(crate) async fn register_agent_in_loop(
    state: &AppState,
    name: &str,
    slug: &str,
) -> Result<(), NeboError> {
    let api = build_api_client(state)?;
    let loops = api
        .list_bot_loops()
        .await
        .map_err(|e| NeboError::Internal(format!("list loops: {e}")))?;
    let personal = loops
        .first()
        .ok_or_else(|| NeboError::Internal("bot not in any loop".into()))?;
    api.register_agent(&personal.loop_id, name, slug, None)
        .await
        .map_err(|e| NeboError::Internal(format!("register agent: {e}")))?;
    info!(role = %name, loop_id = %personal.loop_id, "registered agent in loop");
    Ok(())
}

/// Deregister an agent from the owner's personal loop.
pub(crate) async fn deregister_agent_from_loop(
    state: &AppState,
    agent_slug: &str,
) -> Result<(), NeboError> {
    let api = build_api_client(state)?;
    let loops = api
        .list_bot_loops()
        .await
        .map_err(|e| NeboError::Internal(format!("list loops: {e}")))?;
    let personal = loops
        .first()
        .ok_or_else(|| NeboError::Internal("bot not in any loop".into()))?;
    // Use slug as agent_id for deregister — gateway supports both
    api.deregister_agent(&personal.loop_id, agent_slug)
        .await
        .map_err(|e| NeboError::Internal(format!("deregister agent: {e}")))?;
    info!(agent = %agent_slug, loop_id = %personal.loop_id, "deregistered agent from loop");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_code_valid() {
        assert!(matches!(detect_code("NEBO-A1B2-C3D4"), Some((CodeType::Nebo, _))));
        assert!(matches!(detect_code("SKIL-0000-ZZZZ"), Some((CodeType::Skill, _))));
        assert!(matches!(detect_code("WORK-1234-5678"), Some((CodeType::Work, _))));
        assert!(matches!(detect_code("ROLE-9999-AAAA"), Some((CodeType::Role, _))));
        assert!(matches!(detect_code("LOOP-QRST-VWXY"), Some((CodeType::Loop, _))));
    }

    #[test]
    fn test_detect_code_case_insensitive() {
        assert!(matches!(detect_code("nebo-a1b2-c3d4"), Some((CodeType::Nebo, _))));
        assert!(matches!(detect_code("skil-0000-ZZZZ"), Some((CodeType::Skill, _))));
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
