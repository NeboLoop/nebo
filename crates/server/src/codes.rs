//! Code interception and dispatch for NeboLoop marketplace codes.
//!
//! Detects NEBO/SKILL/WORK/ROLE/AGNT/LOOP codes in chat prompts, handles them
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
    Agent,
    Loop,
    Plugin,
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
    } else if upper.starts_with("AGNT-") {
        ("AGNT-", CodeType::Agent)
    } else if upper.starts_with("LOOP-") {
        ("LOOP-", CodeType::Loop)
    } else if upper.starts_with("PLUG-") {
        ("PLUG-", CodeType::Plugin)
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
#[derive(Default)]
struct CodeHandlerResult {
    message: String,
    artifact_name: Option<String>,
    checkout_url: Option<String>,
    artifact_id: Option<String>,
}

/// Handle a detected code: broadcast processing event, dispatch to handler, broadcast result.
pub async fn handle_code(state: &AppState, code_type: CodeType, code: &str, session_id: &str) {
    let (code_type_str, status_message) = match code_type {
        CodeType::Nebo => ("nebo", "Connecting to NeboLoop..."),
        CodeType::Skill => ("skill", "Installing skill..."),
        CodeType::Work => ("workflow", "Installing workflow..."),
        CodeType::Agent => ("agent", "Installing agent..."),
        CodeType::Loop => ("loop", "Joining loop..."),
        CodeType::Plugin => ("plugin", "Installing plugin..."),
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
        CodeType::Agent => handle_agent_code(state, code).await,
        CodeType::Loop => handle_loop_code(state, code).await,
        CodeType::Plugin => handle_plugin_code(state, code).await,
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
                    "artifact_id": r.artifact_id,
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
        ..Default::default()
    })
}

async fn handle_skill_code(state: &AppState, code: &str) -> Result<CodeHandlerResult, NeboError> {
    let api = build_api_client(state)?;

    // Try to redeem code — may fail if already redeemed (re-install)
    let redeem_result = api.install_skill(code).await;

    if let Ok(ref resp) = redeem_result {
        if resp.status == "payment_required" {
            let name = resp.artifact.name.clone();
            return Ok(CodeHandlerResult {
                message: format!("Skill requires payment: {}", name),
                artifact_name: Some(name),
                checkout_url: Some(resp.checkout_url.clone().unwrap_or_default()),
                ..Default::default()
            });
        }
    }

    let artifact_id;
    let name;

    if let Ok(ref resp) = redeem_result {
        artifact_id = resp.artifact.id.clone();
        name = resp.artifact.name.clone();
    } else {
        // Redemption failed (likely already redeemed) — look up by code
        warn!(code, "skill redeem failed, attempting to look up artifact by code");
        let products = api.list_products(Some("skill"), None, None, None, None).await
            .map_err(|e| NeboError::Internal(format!("list_products: {e}")))?;
        let items = products.get("results").and_then(|v| v.as_array())
            .or_else(|| products.get("skills").and_then(|v| v.as_array()));
        let found = items.and_then(|arr| arr.iter().find(|item| {
            item.get("code").and_then(|c| c.as_str()) == Some(code)
        }));
        if let Some(item) = found {
            artifact_id = item["id"].as_str().unwrap_or("").to_string();
            name = item["name"].as_str().unwrap_or("").to_string();
        } else {
            return Err(NeboError::Internal(format!("install_skill: code not found: {code}")));
        }
    }

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
        ..Default::default()
    })
}

async fn handle_work_code(state: &AppState, code: &str) -> Result<CodeHandlerResult, NeboError> {
    let api = build_api_client(state)?;

    // Try to redeem code — may fail if already redeemed (re-install)
    let redeem_result = api.install_workflow(code).await;

    if let Ok(ref resp) = redeem_result {
        if resp.status == "payment_required" {
            let name = resp.artifact.name.clone();
            return Ok(CodeHandlerResult {
                message: format!("Workflow requires payment: {}", name),
                artifact_name: Some(name),
                checkout_url: Some(resp.checkout_url.clone().unwrap_or_default()),
                ..Default::default()
            });
        }
    }

    let artifact_id;
    let artifact_name;

    if let Ok(ref resp) = redeem_result {
        artifact_id = resp.artifact.id.clone();
        artifact_name = resp.artifact.name.clone();
    } else {
        // Redemption failed (likely already redeemed) — look up by code
        warn!(code, "workflow redeem failed, attempting to look up artifact by code");
        let products = api.list_products(Some("workflow"), None, None, None, None).await
            .map_err(|e| NeboError::Internal(format!("list_products: {e}")))?;
        let items = products.get("results").and_then(|v| v.as_array())
            .or_else(|| products.get("workflows").and_then(|v| v.as_array()));
        let found = items.and_then(|arr| arr.iter().find(|item| {
            item.get("code").and_then(|c| c.as_str()) == Some(code)
        }));
        if let Some(item) = found {
            artifact_id = item["id"].as_str().unwrap_or("").to_string();
            artifact_name = item["name"].as_str().unwrap_or("").to_string();
        } else {
            return Err(NeboError::Internal(format!("install_workflow: code not found: {code}")));
        }
    }

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
        ..Default::default()
    })
}

async fn handle_agent_code(state: &AppState, code: &str) -> Result<CodeHandlerResult, NeboError> {
    let api = build_api_client(state)?;

    // Try to redeem code — may fail if already redeemed (re-install)
    let redeem_result = api.install_agent(code).await;

    if let Ok(ref resp) = redeem_result {
        if resp.status == "payment_required" {
            let name = resp.artifact.name.clone();
            return Ok(CodeHandlerResult {
                message: format!("Agent requires payment: {}", name),
                artifact_name: Some(name),
                checkout_url: Some(resp.checkout_url.clone().unwrap_or_default()),
                ..Default::default()
            });
        }
    }

    let artifact_id;
    let artifact_name;

    if let Ok(ref resp) = redeem_result {
        artifact_id = resp.artifact.id.clone();
        artifact_name = resp.artifact.name.clone();
    } else {
        // Redemption failed (likely already redeemed) — look up by code in local DB
        // or fetch detail from NeboLoop to get the artifact ID
        warn!(code, "redeem failed, attempting to look up artifact by code");
        // Search products to find the artifact by code
        let products = api.list_products(Some("agent"), None, None, None, None).await
            .map_err(|e| NeboError::Internal(format!("list_products: {e}")))?;
        let items = products.get("results").and_then(|v| v.as_array())
            .or_else(|| products.get("skills").and_then(|v| v.as_array()));
        let found = items.and_then(|arr| arr.iter().find(|item| {
            item.get("code").and_then(|c| c.as_str()) == Some(code)
        }));
        if let Some(item) = found {
            artifact_id = item["id"].as_str().unwrap_or("").to_string();
            artifact_name = item["name"].as_str().unwrap_or("").to_string();
        } else {
            return Err(NeboError::Internal(format!("install_agent: code not found: {code}")));
        }
    }

    // Clean reinstall: if agent already exists, fully remove it first
    if let Ok(Some(existing)) = state.store.get_agent(&artifact_id) {
        info!(agent = %artifact_name, id = %artifact_id, "clean reinstall: removing existing agent before re-install");
        state.agent_workers.stop_agent(&artifact_id).await;
        state.agent_registry.write().await.remove(&artifact_id);
        workflow::triggers::unregister_agent_triggers(&artifact_id, &state.store);
        state.event_dispatcher.unsubscribe_agent(&artifact_id).await;
        let _ = state.store.delete_agent_workflows(&artifact_id);
        let _ = state.store.delete_agent(&artifact_id);
        // Clean filesystem
        let slug = existing.name.to_lowercase().replace(' ', "-");
        if let Ok(nebo_dir) = config::nebo_dir() {
            let dir = nebo_dir.join("agents").join(&slug);
            if dir.exists() {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }

    // Fetch artifact content from NeboLoop and persist to DB + filesystem
    let persist_result = match tools::persist_agent_from_api(&api, &artifact_id, &artifact_name, code, &state.store).await {
        Ok(result) => Some(result),
        Err(e) => {
            warn!(code, error = %e, "failed to persist agent artifact after redeem");
            None
        }
    };

    // Process workflow bindings — from persist result or from existing frontmatter in DB
    let mut bindings_processed = false;
    if let Some(ref result) = persist_result {
        if let Some(ref tc) = result.type_config {
            let tc_str = serde_json::to_string(tc).unwrap_or_default();
            match napp::agent::parse_agent_config(&tc_str) {
                Ok(agent_config) => {
                    info!(agent = %artifact_name, workflows = agent_config.workflows.len(), "processing workflow bindings from typeConfig");
                    let _ = crate::handlers::agents::process_agent_bindings(&artifact_id, &agent_config, state).await;
                    bindings_processed = true;
                }
                Err(e) => {
                    warn!(agent = %artifact_name, error = %e, "failed to parse agent config from typeConfig");
                }
            }
        } else {
            info!(agent = %artifact_name, "persist result has no type_config");
        }
    }

    // Fallback: process from existing frontmatter in DB (covers re-install case)
    if !bindings_processed {
        if let Ok(Some(agent)) = state.store.get_agent(&artifact_id) {
            if !agent.frontmatter.is_empty() {
                match napp::agent::parse_agent_config(&agent.frontmatter) {
                    Ok(agent_config) => {
                        info!(agent = %artifact_name, workflows = agent_config.workflows.len(), "processing workflow bindings from DB frontmatter (fallback)");
                        let _ = crate::handlers::agents::process_agent_bindings(&artifact_id, &agent_config, state).await;
                    }
                    Err(e) => {
                        warn!(agent = %artifact_name, error = %e, "failed to parse agent config from DB frontmatter");
                    }
                }
            }
        }
    }

    // Auto-activate the agent so it appears in the sidebar immediately
    if let Ok(Some(agent)) = state.store.get_agent(&artifact_id) {
        let config = if !agent.frontmatter.is_empty() {
            napp::agent::parse_agent_config(&agent.frontmatter).ok()
        } else {
            None
        };
        let active = tools::ActiveAgent {
            agent_id: artifact_id.clone(),
            name: agent.name.clone(),
            agent_md: agent.agent_md.clone(),
            config,
            channel_id: None,
            degraded: None,
        };
        state.agent_registry.write().await.insert(artifact_id.clone(), active);
        state.hub.broadcast(
            "agent_activated",
            serde_json::json!({ "agentId": artifact_id, "name": agent.name }),
        );
    }

    // Register agent in the owner's personal loop
    {
        let st = state.clone();
        let name = artifact_name.clone();
        let slug = artifact_name.to_lowercase().replace(' ', "-");
        tokio::spawn(async move {
            if let Err(e) = register_agent_in_loop(&st, &name, &slug).await {
                warn!(agent = %name, error = %e, "failed to register agent in loop");
            }
        });
    }

    // Cascade: resolve agent deps (workflows, skills, tools from frontmatter)
    let state_clone = state.clone();
    let artifact_id_clone = artifact_id.clone();
    tokio::spawn(async move {
        if let Ok(Some(agent)) = state_clone.store.get_agent(&artifact_id_clone) {
            let deps = crate::deps::extract_agent_deps_from_frontmatter(&agent.frontmatter);
            if !deps.is_empty() {
                let mut visited = std::collections::HashSet::new();
                crate::deps::resolve_cascade(&state_clone, deps, &mut visited).await;
            }
        }
    });

    Ok(CodeHandlerResult {
        message: format!("Installed agent: {}", artifact_name),
        artifact_name: Some(artifact_name),
        checkout_url: None,
        artifact_id: Some(artifact_id),
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
        ..Default::default()
    })
}

async fn handle_plugin_code(state: &AppState, code: &str) -> Result<CodeHandlerResult, NeboError> {
    let api = build_api_client(state)?;

    // Try to redeem code — may fail if already redeemed (re-install)
    let redeem_result = api.install_skill(code).await; // plugins use the same install endpoint

    if let Ok(ref resp) = redeem_result {
        if resp.status == "payment_required" {
            let name = resp.artifact.name.clone();
            return Ok(CodeHandlerResult {
                message: format!("Plugin requires payment: {}", name),
                artifact_name: Some(name),
                checkout_url: Some(resp.checkout_url.clone().unwrap_or_default()),
                ..Default::default()
            });
        }
    }

    let artifact_id;
    let name;

    if let Ok(ref resp) = redeem_result {
        artifact_id = resp.artifact.id.clone();
        name = resp.artifact.name.clone();
    } else {
        // Redemption failed (likely already redeemed) — look up by code
        warn!(code, "plugin redeem failed, attempting to look up artifact by code");
        let products = api.list_products(Some("plugin"), None, None, None, None).await
            .map_err(|e| NeboError::Internal(format!("list_products: {e}")))?;
        let items = products.get("results").and_then(|v| v.as_array())
            .or_else(|| products.get("plugins").and_then(|v| v.as_array()));
        let found = items.and_then(|arr| arr.iter().find(|item| {
            item.get("code").and_then(|c| c.as_str()) == Some(code)
        }));
        if let Some(item) = found {
            artifact_id = item["id"].as_str().unwrap_or("").to_string();
            name = item["name"].as_str().unwrap_or("").to_string();
        } else {
            return Err(NeboError::Internal(format!("install_plugin: code not found: {code}")));
        }
    }

    let platform = napp::plugin::current_platform_key();

    // Broadcast installing event
    state.hub.broadcast(
        "plugin_installing",
        serde_json::json!({
            "plugin": name,
            "platform": platform,
        }),
    );

    // Fetch plugin manifest from NeboLoop (platform-specific binary info)
    let slug = name.to_lowercase().replace(' ', "-");
    let detail = api.get_plugin(&slug, &platform).await
        .map_err(|e| NeboError::Internal(format!("fetch plugin detail: {e}")))?;

    let version = if detail.version.is_empty() { "1.0.0".to_string() } else { detail.version.clone() };

    // Remove existing version so extract re-runs (enables upgrade via re-paste)
    let plugin_store = state.plugin_store.clone();
    let _ = plugin_store.remove(&slug); // ignore error if not installed

    // Get platform-specific download URL from the manifest
    let platform_binary = detail.platforms.get(&platform)
        .ok_or_else(|| NeboError::Internal(format!(
            "plugin {} has no binary for platform {}", slug, platform
        )))?;
    let download_url = &platform_binary.download_url;

    info!(plugin = %name, url = %download_url, "downloading plugin .napp");

    let napp_data = api.download_napp(download_url).await
        .map_err(|e| NeboError::Internal(format!("download .napp for {}: {}", name, e)))?;

    info!(plugin = %name, size = napp_data.len(), "downloaded .napp archive");

    let install_result = plugin_store.install_from_napp(&slug, &version, &napp_data).await;

    match install_result {
        Ok(path) => {
            state.hub.broadcast(
                "plugin_installed",
                serde_json::json!({
                    "plugin": name,
                }),
            );
            info!(code, plugin = %name, artifact_id = %artifact_id, path = %path.display(), "installed plugin");

            // Check if plugin requires authentication
            if let Some(auth) = state.plugin_store.get_manifest(&slug).and_then(|m| m.auth) {
                state.hub.broadcast(
                    "plugin_auth_required",
                    serde_json::json!({
                        "plugin": name,
                        "label": auth.label,
                        "description": auth.description,
                    }),
                );
            }
        }
        Err(e) => {
            state.hub.broadcast(
                "plugin_error",
                serde_json::json!({
                    "plugin": name,
                    "error": e.to_string(),
                }),
            );
            return Err(NeboError::Internal(format!("plugin install failed: {e}")));
        }
    }

    // Reload skill loader so skills with this plugin dep can activate
    state.skill_loader.load_all().await;

    // Re-register plugin tool so the new plugin appears as a resource
    state.tools.unregister("plugin").await;
    if !plugin_store.list_installed().is_empty() {
        state.tools.register(Box::new(
            tools::plugin_tool::PluginTool::new(plugin_store.clone())
        )).await;
    }

    Ok(CodeHandlerResult {
        message: format!("Installed plugin: {}", name),
        artifact_name: Some(name),
        ..Default::default()
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
        CodeType::Agent => handle_agent_code(&state, validated_code).await,
        CodeType::Loop => handle_loop_code(&state, validated_code).await,
        CodeType::Plugin => handle_plugin_code(&state, validated_code).await,
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
        .list_all_active_auth_profiles_by_provider("neboloop")
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
// Skills and agents: canonical implementation in tools::persist_skill_from_api
// and tools::persist_agent_from_api. Workflows have a unique DB+filesystem
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

    // Try sealed .napp download — use API-provided URL or construct from artifact ID.
    // Include platform so the server can serve the right binary for this OS/arch.
    let platform = napp::plugin::current_platform_key();
    let download_url = detail.download_url.clone()
        .or_else(|| Some(format!("/api/v1/apps/{}/download/{}", artifact_id, platform)));
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
    // Guard against re-entry: if already connected, skip.
    if state.comm_manager.is_connected().await {
        return Ok(());
    }

    let bot_id = config::read_bot_id()
        .ok_or_else(|| NeboError::Internal("no bot_id".into()))?;
    let profiles = state
        .store
        .list_all_active_auth_profiles_by_provider("neboloop")
        .unwrap_or_default();
    let profile = profiles
        .first()
        .ok_or_else(|| NeboError::Internal("no NeboLoop credentials".into()))?;
    let mut token = if profile.api_key.is_empty() {
        return Err(NeboError::Internal("empty NeboLoop token".into()));
    } else {
        profile.api_key.clone()
    };

    // Prefer cached rotated token over DB token — the cache is written immediately
    // on AUTH_OK, so it survives hot-reload/crash where the DB persist hasn't run yet.
    if let Ok(dir) = config::data_dir() {
        let cache_path = dir.join("neboloop_token.cache");
        if let Ok(cached) = std::fs::read_to_string(&cache_path) {
            let cached = cached.trim().to_string();
            if !cached.is_empty() && cached != token {
                info!("neboloop: using cached rotated token (differs from DB)");
                token = cached;
            }
        }
    }

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

    let connect_result = state.comm_manager.connect_active(config.clone()).await;

    // If connect fails with stale token, try refreshing via OAuth
    if let Err(ref e) = connect_result {
        let err_msg = e.to_string();
        if err_msg.contains("stale token") || err_msg.contains("auth failed") {
            info!("NeboLoop token stale, attempting OAuth refresh");
            if let Some(new_token) = refresh_neboloop_token(state, profile).await {
                // Retry connect with fresh token
                let mut retry_config = config;
                retry_config.insert("token".into(), new_token);
                state
                    .comm_manager
                    .connect_active(retry_config)
                    .await
                    .map_err(|e| NeboError::Internal(format!("connect after refresh: {e}")))?;
            } else {
                return Err(NeboError::Internal(format!("connect: {err_msg} (refresh failed)")));
            }
        } else {
            return Err(NeboError::Internal(format!("connect: {err_msg}")));
        }
    }

    // Persist rotated JWT so next reconnect uses the fresh token
    if let Some(new_token) = state.comm_manager.take_rotated_token().await {
        if let Ok(profs) = state.store.list_all_active_auth_profiles_by_provider("neboloop") {
            if let Some(p) = profs.first() {
                let _ = state.store.update_auth_profile(
                    &p.id,
                    &p.name,
                    &new_token,
                    p.model.as_deref(),
                    p.base_url.as_deref(),
                    p.priority.unwrap_or(0),
                    p.auth_type.as_deref(),
                    p.metadata.as_deref(),
                );
                info!("persisted rotated NeboLoop token");
            }
        }
    }

    state.hub.broadcast(
        "settings_updated",
        serde_json::json!({"commEnabled": true}),
    );

    // Reconcile agents + sync bot identity in background (non-blocking)
    {
        let st = state.clone();
        tokio::spawn(async move {
            if let Err(e) = reconcile_agents(&st).await {
                warn!(error = %e, "agent reconciliation failed");
            }
            // Sync bot identity (name) to NeboLoop
            sync_bot_identity(&st).await;
        });
    }

    Ok(())
}

/// Sync the bot's display name to NeboLoop from the local agent profile.
pub(crate) async fn sync_bot_identity(state: &AppState) {
    let name = state
        .store
        .get_agent_profile()
        .ok()
        .flatten()
        .map(|p| p.name)
        .unwrap_or_default();
    if name.is_empty() {
        return;
    }
    let api = match build_api_client(state) {
        Ok(a) => a,
        Err(_) => return,
    };
    match api.update_bot_identity(&name, "").await {
        Ok(_) => info!(name = %name, "synced bot identity to NeboLoop"),
        Err(e) => warn!(error = %e, "failed to sync bot identity"),
    }
}

/// Reconcile agents: sync all local agents (enabled AND disabled) to NeboLoop.
/// Only deregister agents that are truly deleted locally, not just paused.
async fn reconcile_agents(state: &AppState) -> Result<(), NeboError> {
    let api = build_api_client(state)?;
    let loops = api
        .list_bot_loops()
        .await
        .map_err(|e| NeboError::Internal(format!("list loops: {e}")))?;
    let personal = match loops.first() {
        Some(l) => l,
        None => return Ok(()), // No loops, nothing to reconcile
    };

    let remote_agents = api
        .list_agents(&personal.loop_id)
        .await
        .map_err(|e| NeboError::Internal(format!("list agents: {e}")))?;

    // Build map of ALL local roles (enabled + disabled) by slug
    let local_agents: std::collections::HashMap<String, bool> =
        if let Ok(roles) = state.store.list_agents(1000, 0) {
            roles
                .iter()
                .map(|r| {
                    let slug = r.name.to_lowercase().replace(' ', "-");
                    let enabled = r.is_enabled != 0;
                    (slug, enabled)
                })
                .collect()
        } else {
            std::collections::HashMap::new()
        };

    // Deregister remote agents that are truly deleted locally (not in DB at all)
    for agent in &remote_agents {
        // Skip the default bot agent (slug starts with "bot_")
        if agent.slug.starts_with("bot_") {
            continue;
        }
        if !local_agents.contains_key(&agent.slug) {
            info!(agent_slug = %agent.slug, agent_id = %agent.id, "reconcile: deregistering deleted agent");
            if let Err(e) = api.deregister_agent(&personal.loop_id, &agent.id).await {
                warn!(agent_slug = %agent.slug, agent_id = %agent.id, error = %e, "reconcile: failed to deregister");
            }
        }
    }

    // Register local roles missing from remote (both enabled and disabled)
    let remote_slugs: std::collections::HashSet<String> =
        remote_agents.iter().map(|a| a.slug.clone()).collect();
    if let Ok(agents) = state.store.list_agents(1000, 0) {
        for agent in &agents {
            let slug = agent.name.to_lowercase().replace(' ', "-");
            if !remote_slugs.contains(&slug) {
                let status = if agent.is_enabled != 0 { "active" } else { "paused" };
                info!(agent = %agent.name, slug = %slug, status = %status, "reconcile: registering missing agent");
                if let Err(e) = api
                    .register_agent(&personal.loop_id, &agent.name, &slug, None)
                    .await
                {
                    warn!(slug = %slug, error = %e, "reconcile: failed to register");
                }
            }
        }
    }

    info!("agent reconciliation complete");
    Ok(())
}

/// Try to refresh the NeboLoop OAuth token using the stored refresh_token.
/// Returns the new access_token if successful, or None.
async fn refresh_neboloop_token(
    state: &AppState,
    profile: &db::models::AuthProfile,
) -> Option<String> {
    // Extract refresh_token from profile metadata
    let metadata: HashMap<String, String> = profile
        .metadata
        .as_deref()
        .and_then(|m| serde_json::from_str(m).ok())
        .unwrap_or_default();
    let refresh_token = metadata.get("refresh_token").filter(|t| !t.is_empty())?;

    let api_url = &state.config.neboloop.api_url;
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": "nbl_nebo_desktop",
    });

    let resp = match reqwest::Client::new()
        .post(format!("{api_url}/oauth/token"))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "OAuth refresh request failed");
            return None;
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        warn!(status = %status, body = %text, "OAuth refresh failed");
        return None;
    }

    #[derive(serde::Deserialize)]
    struct RefreshResponse {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
    }

    let token_resp: RefreshResponse = match resp.json().await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "OAuth refresh response parse failed");
            return None;
        }
    };

    // Persist the new tokens
    let new_refresh = token_resp.refresh_token.as_deref().unwrap_or(refresh_token);
    let mut new_metadata = metadata.clone();
    new_metadata.insert("refresh_token".to_string(), new_refresh.to_string());
    let metadata_json = serde_json::to_string(&new_metadata).unwrap_or_default();

    let _ = state.store.update_auth_profile(
        &profile.id,
        &profile.name,
        &token_resp.access_token,
        profile.model.as_deref(),
        profile.base_url.as_deref(),
        profile.priority.unwrap_or(0),
        profile.auth_type.as_deref(),
        Some(&metadata_json),
    );
    info!("NeboLoop OAuth token refreshed successfully");

    Some(token_resp.access_token)
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
    info!(agent = %name, loop_id = %personal.loop_id, "registered agent in loop");
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
        assert!(matches!(detect_code("AGNT-9999-AAAA"), Some((CodeType::Agent, _))));
        assert!(matches!(detect_code("LOOP-QRST-VWXY"), Some((CodeType::Loop, _))));
        assert!(matches!(detect_code("PLUG-A1B2-C3D4"), Some((CodeType::Plugin, _))));
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
