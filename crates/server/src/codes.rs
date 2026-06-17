//! Code interception and dispatch for NeboAI marketplace codes.
//!
//! Detects NEBO/SKILL/WORK/ROLE/AGNT/LOOP codes in chat prompts, handles them
//! before the prompt reaches the agent runner, and broadcasts results to the client.

use std::collections::HashMap;

use tracing::{debug, info, warn};

use comm::api::NeboAIApi;
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
    App,
    Collection,
    Connection,
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
    } else if upper.starts_with("APPS-") {
        ("APPS-", CodeType::App)
    } else if upper.starts_with("CONN-") {
        ("CONN-", CodeType::Connection)
    } else if upper.starts_with("COLL-") {
        ("COLL-", CodeType::Collection)
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
    artifact_type: Option<String>,
    needs_auth: bool,
    /// Pricing tier info forwarded from NeboAI (name, recurringPriceCents, billingInterval, pricingModel).
    tier: Option<serde_json::Value>,
}

/// Handle a detected code: broadcast processing event, dispatch to handler, broadcast result.
pub async fn handle_code(state: &AppState, code_type: CodeType, code: &str, session_id: &str) {
    let (code_type_str, status_message) = match code_type {
        CodeType::Nebo => ("nebo", "Connecting to NeboAI..."),
        CodeType::Skill => ("skill", "Installing skill..."),
        CodeType::Work => ("workflow", "Installing workflow..."),
        CodeType::Agent => ("agent", "Installing agent..."),
        CodeType::Loop => ("loop", "Joining loop..."),
        CodeType::Plugin => ("plugin", "Installing plugin..."),
        CodeType::App => ("app", "Installing app..."),
        CodeType::Collection => ("collection", "Installing collection..."),
        CodeType::Connection => ("connection", "Adding MCP connection..."),
    };

    state.hub.broadcast(
        "code_processing",
        serde_json::json!({
            "session_id": session_id,
            "code": code,
            "code_type": code_type_str,
            "status_message": status_message,
            // User-initiated from the desktop UI: the modal stays open for the
            // user to read until they dismiss it.
            "interactive": true,
        }),
    );

    let result = match code_type {
        CodeType::Nebo => handle_nebo_code(state, code).await,
        CodeType::Skill => handle_skill_code(state, code).await,
        CodeType::Work => handle_work_code(state, code).await,
        CodeType::Agent => handle_agent_code(state, code).await,
        CodeType::Loop => handle_loop_code(state, code).await,
        CodeType::Plugin => handle_plugin_code(state, code).await,
        CodeType::App => handle_app_code(state, code).await,
        CodeType::Collection => handle_collection_code(state, code).await,
        CodeType::Connection => handle_connection_code(state, code).await,
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
                    "artifact_type": r.artifact_type.as_deref().unwrap_or(code_type_str),
                    "payment_required": payment_required,
                    "checkout_url": r.checkout_url,
                    "needsAuth": r.needs_auth,
                    "tier": r.tier,
                    "interactive": true,
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
                    "interactive": true,
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

/// Handle a detected code and return a text response (for channel bridges).
///
/// Same logic as `handle_code` but returns the result as a string instead
/// of broadcasting WebSocket events. Used by Slack, Telegram, etc.
pub async fn handle_code_text(state: &AppState, code_type: CodeType, code: &str) -> String {
    let code_type_str = match code_type {
        CodeType::Nebo => "NeboAI connection",
        CodeType::Skill => "skill",
        CodeType::Work => "workflow",
        CodeType::Agent => "agent",
        CodeType::Loop => "loop",
        CodeType::Plugin => "plugin",
        CodeType::App => "app",
        CodeType::Collection => "collection",
        CodeType::Connection => "connection",
    };

    // Also broadcast for the frontend UI. Triggered remotely via a channel
    // (loop, Slack, etc.) — no human is waiting on the desktop modal, so it
    // auto-dismisses rather than blocking until manually closed.
    state.hub.broadcast(
        "code_processing",
        serde_json::json!({
            "code": code,
            "code_type": code_type_str,
            "status_message": format!("Installing {code_type_str}..."),
            "interactive": false,
        }),
    );

    let result = match code_type {
        CodeType::Nebo => handle_nebo_code(state, code).await,
        CodeType::Skill => handle_skill_code(state, code).await,
        CodeType::Work => handle_work_code(state, code).await,
        CodeType::Agent => handle_agent_code(state, code).await,
        CodeType::Loop => handle_loop_code(state, code).await,
        CodeType::Plugin => handle_plugin_code(state, code).await,
        CodeType::App => handle_app_code(state, code).await,
        CodeType::Collection => handle_collection_code(state, code).await,
        CodeType::Connection => handle_connection_code(state, code).await,
    };

    match result {
        Ok(r) => {
            // Broadcast for frontend
            state.hub.broadcast(
                "code_result",
                serde_json::json!({
                    "code": code,
                    "code_type": code_type_str,
                    "success": true,
                    "message": r.message,
                    "artifact_name": r.artifact_name,
                    "artifact_id": r.artifact_id,
                    "interactive": false,
                }),
            );

            if let Some(url) = r.checkout_url {
                format!("{}. Payment required: {}", r.message, url)
            } else {
                r.message
            }
        }
        Err(e) => {
            warn!(code = code, error = %e, "code handling failed (channel)");
            format!("Failed to install {code_type_str}: {e}")
        }
    }
}

// ── Per-Type Handlers ───────────────────────────────────────────────

async fn handle_nebo_code(state: &AppState, code: &str) -> Result<CodeHandlerResult, NeboError> {
    let bot_id = redeem_nebo_code(state, code).await?;
    Ok(CodeHandlerResult {
        message: format!("Connected to NeboAI (bot: {})", &bot_id[..8]),
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
                artifact_type: Some("skill".to_string()),
                checkout_url: Some(resp.checkout_url.clone().unwrap_or_default()),
                tier: resp.tier.clone(),
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
        warn!(
            code,
            "skill redeem failed, attempting to look up artifact by code"
        );
        let products = api
            .list_products(Some("skill"), None, None, None, None)
            .await
            .map_err(|e| NeboError::Internal(format!("list_products: {e}")))?;
        let items = products
            .get("products")
            .and_then(|v| v.as_array())
            .or_else(|| products.get("results").and_then(|v| v.as_array()))
            .or_else(|| products.get("skills").and_then(|v| v.as_array()));
        let found = items.and_then(|arr| {
            arr.iter()
                .find(|item| item.get("code").and_then(|c| c.as_str()) == Some(code))
        });
        if let Some(item) = found {
            artifact_id = item["id"].as_str().unwrap_or("").to_string();
            name = item["name"].as_str().unwrap_or("").to_string();
        } else {
            return Err(NeboError::Internal(format!(
                "install_skill: code not found: {code}"
            )));
        }
    }

    // Fetch artifact content from NeboAI and persist to filesystem
    let skill_dir = match tools::persist_skill_from_api(
        &api,
        &artifact_id,
        &name,
        code,
        Some(&state.store),
    )
    .await
    {
        Ok(dir) => {
            info!(code, name = %name, dir = %dir.display(), "persisted skill artifact to filesystem");
            Some(dir)
        }
        Err(e) => {
            warn!(code, artifact_id = %artifact_id, error = %e, "failed to persist skill artifact after redeem");
            None
        }
    };

    // Seed artifact update tracking for skills
    if let Some(ref dir) = skill_dir {
        let version = dir
            .join("manifest.json")
            .to_str()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v["version"].as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "1.0.0".to_string());
        let _ = state.store.upsert_artifact_update_pref(&artifact_id, "skill", &version);
    }

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
                artifact_type: Some("workflow".to_string()),
                checkout_url: Some(resp.checkout_url.clone().unwrap_or_default()),
                tier: resp.tier.clone(),
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
        warn!(
            code,
            "workflow redeem failed, attempting to look up artifact by code"
        );
        let products = api
            .list_products(Some("workflow"), None, None, None, None)
            .await
            .map_err(|e| NeboError::Internal(format!("list_products: {e}")))?;
        let items = products
            .get("products")
            .and_then(|v| v.as_array())
            .or_else(|| products.get("results").and_then(|v| v.as_array()))
            .or_else(|| products.get("workflows").and_then(|v| v.as_array()));
        let found = items.and_then(|arr| {
            arr.iter()
                .find(|item| item.get("code").and_then(|c| c.as_str()) == Some(code))
        });
        if let Some(item) = found {
            artifact_id = item["id"].as_str().unwrap_or("").to_string();
            artifact_name = item["name"].as_str().unwrap_or("").to_string();
        } else {
            return Err(NeboError::Internal(format!(
                "install_workflow: code not found: {code}"
            )));
        }
    }

    // Fetch artifact content from NeboAI and persist to DB + filesystem
    if let Err(e) =
        persist_workflow_artifact(&api, &artifact_id, &artifact_name, code, &state.store).await
    {
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

/// Install a collection (COLL-): redeem the code to resolve the collection, then
/// install EVERY item through the one canonical multi-artifact installer
/// (`deps::resolve_cascade`), which redeems, persists, reloads, and cascades
/// each item's transitive dependencies. There is intentionally no separate
/// collection installer — a collection is just a named list of artifact codes, so
/// it flows through the same path a single pasted code would.
async fn handle_collection_code(
    state: &AppState,
    code: &str,
) -> Result<CodeHandlerResult, NeboError> {
    let api = build_api_client(state)?;

    // Redeem resolves the collection artifact (and records the collection install
    // for this bot). Payment-gated collections surface a checkout URL like any code.
    let resp = api
        .redeem_code(code)
        .await
        .map_err(|e| NeboError::Internal(format!("redeem collection code: {e}")))?;

    let artifact_id = resp.artifact.id.clone();
    let artifact_name = resp.artifact.name.clone();

    if resp.status == "payment_required" {
        return Ok(CodeHandlerResult {
            message: format!("Collection requires payment: {artifact_name}"),
            artifact_name: Some(artifact_name),
            artifact_type: Some("collection".to_string()),
            checkout_url: Some(resp.checkout_url.clone().unwrap_or_default()),
            tier: resp.tier.clone(),
            ..Default::default()
        });
    }

    // Fetch the collection's resolved items — each carries its own install code + type.
    let coll = api
        .get_collection(&artifact_id)
        .await
        .map_err(|e| NeboError::Internal(format!("get collection {artifact_id}: {e}")))?;
    let items = coll
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Map each item to a DepRef keyed by its install code. Items without a usable
    // code or with an unrecognized type are logged and skipped — never silently
    // dropped (Rule 6.1: log and continue).
    let mut deps = Vec::new();
    let mut has_app = false;
    for item in &items {
        let item_code = item.get("code").and_then(|c| c.as_str()).unwrap_or("");
        let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let item_name = item
            .get("name")
            .and_then(|n| n.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        // Canonical plugin slug — lets the cascade detect an already-installed
        // plugin that's referenced here by code.
        let item_slug = item
            .get("slug")
            .and_then(|i| i.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        if item_code.is_empty() {
            warn!(collection = %artifact_id, item_type, "collection item has no install code — skipping");
            continue;
        }
        let dep_type = match item_type {
            "skill" => crate::deps::DepType::Skill,
            // Apps ARE agents (artifact_type="app") — install via the agent path,
            // then reconcile app fields below so they're recognised as apps.
            "agent" | "app" => crate::deps::DepType::Agent,
            "plugin" => crate::deps::DepType::Plugin,
            "workflow" => crate::deps::DepType::Workflow,
            other => {
                warn!(collection = %artifact_id, item_type = other, code = item_code, "unrecognized collection item type — skipping");
                continue;
            }
        };
        if item_type == "app" {
            has_app = true;
        }
        deps.push(crate::deps::DepRef {
            dep_type,
            reference: item_code.to_string(),
            name: item_name,
            slug: item_slug,
        });
    }
    let total = deps.len();

    // Install every item via the canonical installer.
    let mut visited = std::collections::HashSet::new();
    let result = crate::deps::resolve_cascade(state, deps, &mut visited).await;

    // Apps install through the agent path; detect & persist their app-specific
    // paths so they show up and launch as apps (the cascade only does the agent part).
    if has_app {
        reconcile_app_fields(state).await;
    }

    let mut message = format!(
        "Installed collection \"{}\": {} of {} items installed",
        artifact_name, result.installed_count, total
    );
    if result.failed_count > 0 {
        message.push_str(&format!(", {} failed", result.failed_count));
    }

    // Surface installed plugins that still need credentials/config so the modal
    // can show a "Needs setup" section. Same source as the agent-install wizard.
    let needs_setup = sweep_plugin_auth(state).await;
    if !needs_setup.is_empty() {
        state.hub.broadcast(
            "dep_needs_setup",
            serde_json::json!({ "items": needs_setup }),
        );
    }

    Ok(CodeHandlerResult {
        message,
        artifact_name: Some(artifact_name),
        artifact_id: Some(artifact_id),
        artifact_type: Some("collection".to_string()),
        ..Default::default()
    })
}

/// Install an MCP connection (CONN-): the connector's manifest is a standard MCP
/// server config block (`{ "mcpServers": { ... } }`). Redeem resolves the
/// connector, then we hand the block to the SAME parser + local-integration
/// creation the settings "paste a config" path uses — no separate installer.
/// stdio servers connect immediately; OAuth servers surface in Settings → MCP
/// for the user to authorize, exactly like a manually added one.
async fn handle_connection_code(
    state: &AppState,
    code: &str,
) -> Result<CodeHandlerResult, NeboError> {
    let api = build_api_client(state)?;

    let resp = api
        .redeem_code(code)
        .await
        .map_err(|e| NeboError::Internal(format!("redeem connection code: {e}")))?;
    let artifact_id = resp.artifact.id.clone();
    let artifact_name = resp.artifact.name.clone();

    if resp.status == "payment_required" {
        return Ok(CodeHandlerResult {
            message: format!("Connection requires payment: {artifact_name}"),
            artifact_name: Some(artifact_name),
            artifact_type: Some("connection".to_string()),
            checkout_url: Some(resp.checkout_url.clone().unwrap_or_default()),
            tier: resp.tier.clone(),
            ..Default::default()
        });
    }

    // The MCP config travels as the connector's manifest. It may be a JSON object
    // or a JSON-encoded string (how some artifacts store manifest content).
    let detail = api
        .get_skill(&artifact_id)
        .await
        .map_err(|e| NeboError::Internal(format!("fetch connector {artifact_id}: {e}")))?;
    let raw = detail.manifest.ok_or_else(|| {
        NeboError::Internal(format!("connector '{artifact_name}' has no MCP config"))
    })?;
    let block = match raw {
        serde_json::Value::String(s) => serde_json::from_str(&s)
            .map_err(|e| NeboError::Internal(format!("connector config is not valid JSON: {e}")))?,
        other => other,
    };

    let created = crate::handlers::integrations::create_integrations_from_block(state, &block)
        .await
        .map_err(|e| NeboError::Internal(format!("create MCP integration(s): {e}")))?;

    if created.is_empty() {
        return Err(NeboError::Internal(format!(
            "connector '{artifact_name}' has no valid MCP servers in its config"
        )));
    }

    Ok(CodeHandlerResult {
        message: format!(
            "Added MCP connection \"{}\": {} server(s)",
            artifact_name,
            created.len()
        ),
        artifact_name: Some(artifact_name),
        artifact_id: Some(artifact_id),
        artifact_type: Some("connection".to_string()),
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
                artifact_type: Some("agent".to_string()),
                checkout_url: Some(resp.checkout_url.clone().unwrap_or_default()),
                tier: resp.tier.clone(),
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
        // or fetch detail from NeboAI to get the artifact ID
        warn!(
            code,
            "redeem failed, attempting to look up artifact by code"
        );
        // Search products to find the artifact by code
        let products = api
            .list_products(Some("agent"), None, None, None, None)
            .await
            .map_err(|e| NeboError::Internal(format!("list_products: {e}")))?;
        let items = products
            .get("products")
            .and_then(|v| v.as_array())
            .or_else(|| products.get("results").and_then(|v| v.as_array()))
            .or_else(|| products.get("skills").and_then(|v| v.as_array()));
        let found = items.and_then(|arr| {
            arr.iter()
                .find(|item| item.get("code").and_then(|c| c.as_str()) == Some(code))
        });
        if let Some(item) = found {
            artifact_id = item["id"].as_str().unwrap_or("").to_string();
            artifact_name = item["name"].as_str().unwrap_or("").to_string();
        } else {
            return Err(NeboError::Internal(format!(
                "install_agent: code not found: {code}"
            )));
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
        // Deregister from NeboAI so re-registration doesn't 409
        let _ = deregister_agent_from_loop(state, &existing.name).await;
    }

    // Fetch artifact content from NeboAI and persist to DB + filesystem
    let persist_result =
        match tools::persist_agent_from_api(&api, &artifact_id, &artifact_name, code, &state.store)
            .await
        {
            Ok(result) => Some(result),
            Err(e) => {
                warn!(code, error = %e, "failed to persist agent artifact after redeem");
                None
            }
        };

    // Reload the in-memory AgentLoader so the newly-installed agent appears immediately.
    // list_agents() enumerates the loader (filesystem source of truth) and only supplements
    // with DB rows — a DB row with no loaded agent is never emitted. Without this reload the
    // agent doesn't surface until a server restart, so even a frontend hard-reload shows
    // nothing. (Apps already reload via handle_app_code; do it for plain agents too.)
    if persist_result.is_some() {
        state.agent_loader.load_all().await;
    }

    // Notify frontend immediately so sidebar refreshes
    state.hub.broadcast(
        "agent_installed",
        serde_json::json!({ "agentId": artifact_id, "name": artifact_name }),
    );

    // Cascade: resolve agent deps (plugins, skills) in background — don't block code_result
    {
        let has_type_config = persist_result
            .as_ref()
            .and_then(|r| r.type_config.as_ref())
            .is_some();
        info!(agent = %artifact_name, has_type_config, "cascade: checking for deps");
        let frontmatter = persist_result
            .as_ref()
            .and_then(|r| r.type_config.as_ref())
            .and_then(|tc| serde_json::to_string(tc).ok());
        let fm = match frontmatter {
            Some(fm) => fm,
            None => state
                .store
                .get_agent(&artifact_id)
                .ok()
                .flatten()
                .map(|a| a.frontmatter.clone())
                .unwrap_or_default(),
        };
        info!(agent = %artifact_name, fm_len = fm.len(), fm_empty = fm.is_empty(), "cascade: frontmatter");
        if !fm.is_empty() {
            // Log a snippet of the frontmatter to verify requires block is present
            let snippet: String = fm.chars().take(200).collect();
            info!(agent = %artifact_name, snippet, "cascade: frontmatter snippet");
            let deps = crate::deps::extract_agent_deps_from_frontmatter(&fm);
            info!(agent = %artifact_name, dep_count = deps.len(), "cascade: extracted deps");
            for d in &deps {
                info!(dep_type = ?d.dep_type, reference = %d.reference, "cascade: dep");
            }
            if !deps.is_empty() {
                let bg_state = state.clone();
                let bg_name = artifact_name.clone();
                tokio::spawn(async move {
                    let mut visited = std::collections::HashSet::new();
                    crate::deps::resolve_cascade(&bg_state, deps, &mut visited).await;
                    info!(agent = %bg_name, "cascade: background dep resolution complete");
                });
            }
        }
    }

    // Sweep installed plugins for pending auth requirements. Gates auto-activation
    // below (defer activation while a plugin still needs connecting). The frontend
    // discovers what to connect via getAgent().pluginsNeedingAuth (the one canonical
    // pull) — there is intentionally no separate auth event broadcast here.
    let auth_required = sweep_plugin_auth(state).await;
    let has_auth_requirements = !auth_required.is_empty();
    if has_auth_requirements {
        info!(agent = %artifact_name, plugins = auth_required.len(), "agent install: plugins need auth (deferring activation)");
    }

    // Process workflow bindings — from persist result or from existing frontmatter in DB
    // (persists to DB regardless of auth; triggers only fire after activation)
    let mut bindings_processed = false;
    if let Some(ref result) = persist_result {
        if let Some(ref tc) = result.type_config {
            let tc_str = serde_json::to_string(tc).unwrap_or_default();
            match napp::agent::parse_agent_config(&tc_str) {
                Ok(agent_config) => {
                    info!(agent = %artifact_name, workflows = agent_config.workflows.len(), "processing workflow bindings from typeConfig");
                    let _ = crate::handlers::agents::process_agent_bindings(
                        &artifact_id,
                        &agent_config,
                        state,
                    )
                    .await;
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
                        let _ = crate::handlers::agents::process_agent_bindings(
                            &artifact_id,
                            &agent_config,
                            state,
                        )
                        .await;
                    }
                    Err(e) => {
                        warn!(agent = %artifact_name, error = %e, "failed to parse agent config from DB frontmatter");
                    }
                }
            }
        }
    }

    // Auto-activate only if no auth is pending — otherwise the frontend wizard
    // will call activateAgent() after the user completes all OAuth flows.
    if !has_auth_requirements {
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
                soul: agent.soul.clone(),
                rules: agent.rules.clone(),
            };
            state
                .agent_registry
                .write()
                .await
                .insert(artifact_id.clone(), active);
            state.hub.broadcast(
                "agent_activated",
                serde_json::json!({ "agentId": artifact_id, "name": agent.name }),
            );
        }

        // Register agent in the owner's personal loop
        {
            let st = state.clone();
            let name = artifact_name.clone();
            tokio::spawn(async move {
                if let Err(e) = register_agent_in_loop(&st, &name).await {
                    warn!(agent = %name, error = %e, "failed to register agent in loop");
                }
            });
        }
    }

    Ok(CodeHandlerResult {
        message: format!("Installed agent: {}", artifact_name),
        artifact_name: Some(artifact_name),
        artifact_id: Some(artifact_id),
        needs_auth: has_auth_requirements,
        ..Default::default()
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
                artifact_type: Some("plugin".to_string()),
                checkout_url: Some(resp.checkout_url.clone().unwrap_or_default()),
                tier: resp.tier.clone(),
                ..Default::default()
            });
        }
    }

    let artifact_id;
    let name;
    let slug_hint;

    if let Ok(ref resp) = redeem_result {
        artifact_id = resp.artifact.id.clone();
        name = resp.artifact.name.clone();
        slug_hint = resp.artifact.slug.clone();
    } else {
        // Redemption failed (likely already redeemed) — look up by code
        warn!(
            code,
            "plugin redeem failed, attempting to look up artifact by code"
        );
        let products = api
            .list_products(Some("plugin"), None, None, None, None)
            .await
            .map_err(|e| NeboError::Internal(format!("list_products: {e}")))?;
        let items = products
            .get("products")
            .and_then(|v| v.as_array())
            .or_else(|| products.get("results").and_then(|v| v.as_array()))
            .or_else(|| products.get("plugins").and_then(|v| v.as_array()));
        let found = items.and_then(|arr| {
            arr.iter()
                .find(|item| item.get("code").and_then(|c| c.as_str()) == Some(code))
        });
        if let Some(item) = found {
            artifact_id = item["id"].as_str().unwrap_or("").to_string();
            name = item["name"].as_str().unwrap_or("").to_string();
            slug_hint = item["slug"].as_str().unwrap_or("").to_string();
        } else {
            return Err(NeboError::Internal(format!(
                "install_plugin: code not found: {code}"
            )));
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

    // Resolve by the canonical marketplace slug — NEVER derive it from the display
    // name. ("Google Workspace" → "google-workspace" 404'd; the real slug is "gws".)
    // The server guarantees slug NOT NULL + UNIQUE(slug,type), so an empty slug is a
    // data error — fail loudly rather than silently munge the name into a wrong slug.
    if slug_hint.is_empty() {
        return Err(NeboError::Internal(format!(
            "plugin '{name}' has no slug in the marketplace record; refusing to guess from the display name"
        )));
    }
    let slug = slug_hint;

    // ONE plugin installer — resolve binary, download, install, register.
    if let Err(e) = fetch_and_install_plugin(state, &api, &slug, &name).await {
        state.hub.broadcast(
            "plugin_error",
            serde_json::json!({ "plugin": name, "error": e.to_string() }),
        );
        return Err(e);
    }
    state
        .hub
        .broadcast("plugin_installed", serde_json::json!({ "plugin": name }));
    info!(code, plugin = %name, artifact_id = %artifact_id, "installed plugin");

    // Cascade plugin-to-plugin dependencies (e.g., digest → ffmpeg).
    if let Some(manifest) = state.plugin_store.get_manifest(&slug) {
        if !manifest.dependencies.is_empty() {
            let ps = state.plugin_store.clone();
            if let Ok(api2) = build_api_client(state) {
                let api2 = std::sync::Arc::new(api2);
                match ps
                    .ensure_deps(&manifest, |dep_slug, _dep_version| {
                        let api_inner = api2.clone();
                        async move {
                            let platform = napp::plugin::current_platform_key();
                            let m = api_inner
                                .get_plugin(&dep_slug, &platform)
                                .await
                                .map_err(|e| {
                                    napp::NappError::PluginDownloadFailed(e.to_string())
                                })?;
                            let url = m
                                .platforms
                                .get(&platform)
                                .map(|pb| pb.download_url.clone())
                                .ok_or_else(|| {
                                    napp::NappError::PluginDownloadFailed(format!(
                                        "dep {} has no binary for {}",
                                        dep_slug, platform
                                    ))
                                })?;
                            let data = api_inner.download_napp(&url).await.map_err(|e| {
                                napp::NappError::PluginDownloadFailed(e.to_string())
                            })?;
                            Ok((m, data))
                        }
                    })
                    .await
                {
                    Ok(installed) => {
                        for dep_slug in &installed {
                            info!(plugin = %slug, dep = %dep_slug, "installed dependency plugin");
                        }
                    }
                    Err(e) => {
                        warn!(plugin = %slug, error = %e, "failed to install plugin dependencies");
                    }
                }
            }
        }
    }

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

    Ok(CodeHandlerResult {
        message: format!("Installed plugin: {}", name),
        artifact_name: Some(name),
        ..Default::default()
    })
}

/// Download a marketplace plugin's per-platform `.napp` by slug, install it,
/// register it in the DB, and wire up its tool + hooks.
///
/// The ONE plugin-install core — shared by the standalone code redeemer
/// (`handle_plugin_code`) and the dependency cascade (`deps::install_plugin`) so
/// their binary resolution and DB registration can't drift. Callers own their
/// own surrounding concerns (progress broadcasts, child-dep handling, auth).
pub(crate) async fn fetch_and_install_plugin(
    state: &AppState,
    api: &NeboAIApi,
    slug: &str,
    name: &str,
) -> Result<(), NeboError> {
    let platform = napp::plugin::current_platform_key();
    let detail = api
        .get_plugin(slug, &platform)
        .await
        .map_err(|e| NeboError::Internal(format!("fetch plugin detail for {slug}: {e}")))?;
    let version = if detail.version.is_empty() {
        "1.0.0".to_string()
    } else {
        detail.version.clone()
    };
    // Resolve the real per-platform download URL from the manifest — NOT the redeem
    // response's `download_url`, which is empty for code-redeemed plugins.
    let platform_binary = detail.platforms.get(&platform).ok_or_else(|| {
        NeboError::Internal(format!("plugin {slug} has no binary for platform {platform}"))
    })?;

    info!(plugin = %name, url = %platform_binary.download_url, "downloading plugin .napp");
    let napp_data = api
        .download_napp(&platform_binary.download_url)
        .await
        .map_err(|e| NeboError::Internal(format!("download .napp for {name}: {e}")))?;

    // Pause the skill watcher during extraction to prevent premature reloads.
    state.skill_loader.pause_watcher();
    let _ = state.plugin_store.remove(slug);
    let install = state
        .plugin_store
        .install_from_napp(slug, &version, &napp_data)
        .await;
    state.skill_loader.load_all().await;
    state.skill_loader.resume_watcher();
    let path = install.map_err(|e| NeboError::Internal(format!("install plugin {slug}: {e}")))?;
    info!(plugin = %name, path = %path.display(), "installed plugin");

    // Persist to the DB registry (Settings → Plugins reads this) + update tracking.
    let sig_status = if platform_binary.signature.is_empty() {
        "unverified"
    } else {
        "verified"
    };
    if let Err(e) = state.store.upsert_installed_plugin(
        slug,
        name,
        &version,
        &detail.author,
        &path.display().to_string(),
        &platform_binary.sha256,
        sig_status,
    ) {
        warn!(plugin = %slug, error = %e, "failed to upsert plugin into DB registry");
    }
    let _ = state.store.upsert_artifact_update_pref(slug, "plugin", &version);

    // Re-register the plugin tool + hooks so the new plugin is usable immediately.
    // Always register (never gate on count) — the tool must stay present in the prompt.
    state.tools.unregister("plugin").await;
    state
        .tools
        .register(Box::new(tools::plugin_tool::PluginTool::new(
            state.plugin_store.clone(),
            state.store.clone(),
        )))
        .await;
    if let Some(manifest) = state.plugin_store.get_manifest(slug) {
        if let Some(binary) = state.plugin_store.resolve(slug, "*") {
            let count =
                napp::register_plugin_hooks(&manifest, &binary, &state.hooks, state.plugin_store.clone());
            if count > 0 {
                info!(plugin = %slug, hooks = count, "registered plugin hooks");
            }
        }
    }
    Ok(())
}

/// Detect and persist app-specific fields (ui/, bin/, window config) for every
/// installed app agent. Apps install through the agent path (standalone code or
/// collection cascade) but only become recognised/launchable as apps once these
/// DB fields are set. Idempotent — safe to call after any agent/app install.
pub(crate) async fn reconcile_app_fields(state: &AppState) {
    state.agent_loader.load_all().await;
    for loaded in state.agent_loader.list().await {
        if !loaded.is_app {
            continue;
        }
        let Some(id) = loaded.id.as_deref() else {
            continue;
        };
        let _ = state.store.set_agent_app_fields(
            id,
            true,
            loaded.app_ui_path.as_ref().and_then(|p| p.to_str()),
            loaded.app_binary_path.as_ref().and_then(|p| p.to_str()),
            loaded
                .app_window_config
                .as_ref()
                .and_then(|w| serde_json::to_string(w).ok())
                .as_deref(),
        );
    }
}

async fn handle_app_code(state: &AppState, code: &str) -> Result<CodeHandlerResult, NeboError> {
    // Apps use the same install flow as agents — they ARE agents with artifact_type="app"
    let result = handle_agent_code(state, code).await?;

    // Detect & persist app-specific paths (ui/, bin/) so it's recognised as an app.
    reconcile_app_fields(state).await;

    Ok(CodeHandlerResult {
        message: format!(
            "Installed app: {}",
            result.artifact_name.as_deref().unwrap_or("unknown")
        ),
        artifact_name: result.artifact_name,
        artifact_id: result.artifact_id,
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
    (
        axum::http::StatusCode,
        axum::response::Json<types::api::ErrorResponse>,
    ),
> {
    let code = body["code"].as_str().ok_or_else(|| {
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
        CodeType::App => handle_app_code(&state, validated_code).await,
        CodeType::Collection => handle_collection_code(&state, validated_code).await,
        CodeType::Connection => handle_connection_code(&state, validated_code).await,
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

// ── Plugin Auth Sweep ───────────────────────────────────────────────

/// Check all installed plugins for pending authentication requirements.
///
/// Returns a list of plugins that have auth config but are not yet authenticated.
/// Used after dependency cascade to determine if the agent install wizard should
/// prompt the user for OAuth before activating the agent.
async fn sweep_plugin_auth(state: &AppState) -> Vec<serde_json::Value> {
    let mut needs_auth = Vec::new();
    let installed = state.plugin_store.list_installed();
    let mut seen = std::collections::HashSet::new();
    for (slug, _, _, _) in &installed {
        if !seen.insert(slug.clone()) {
            continue;
        }
        match crate::handlers::plugins::check_plugin_auth(&state.plugin_store, slug).await {
            Some(false) => {
                if let Some(manifest) = state.plugin_store.get_manifest(slug) {
                    if let Some(auth) = &manifest.auth {
                        needs_auth.push(serde_json::json!({
                            "slug": slug,
                            "label": auth.label,
                            "description": auth.description,
                            // "env" = user-supplied API keys (configured via a form in
                            // Settings → Plugins), anything else = interactive OAuth login.
                            "authType": auth.auth_type,
                        }));
                    }
                }
            }
            _ => {} // authenticated or no auth config
        }
    }
    needs_auth
}

// ── API Client Helper ───────────────────────────────────────────────

pub(crate) fn build_api_client(state: &AppState) -> Result<NeboAIApi, NeboError> {
    let bot_id =
        config::read_bot_id().ok_or_else(|| NeboError::Internal("no bot_id configured".into()))?;
    let profiles = state
        .store
        .list_all_active_auth_profiles_by_provider("neboai")
        .unwrap_or_default();
    let profile = profiles
        .first()
        .ok_or_else(|| NeboError::Internal("not connected to NeboAI".into()))?;
    let api_server = state.config.neboai.api_url.clone();
    Ok(NeboAIApi::new(
        api_server,
        bot_id,
        profile.api_key.clone(),
    ))
}

/// Push a single chat's (possibly newly-generated) title to its NeboLoop
/// agent-space conversation, so a title generated mid-session appears remotely
/// right away instead of only after the next reconnect-time reconcile. Uses the
/// same canonical `chats/sync` upsert as `reconcile_agents` — this is just the
/// incremental, one-chat case of that bulk push. Best-effort: silently skips
/// when the agent isn't loop-registered or the bot isn't connected to NeboAI.
pub(crate) async fn push_chat_title_to_loop(
    state: &AppState,
    local_agent_id: &str,
    chat_id: &str,
    title: &str,
) {
    let loop_agent_id = match state.store.get_agent(local_agent_id) {
        Ok(Some(agent)) => match agent.loop_agent_id {
            Some(id) => id,
            None => return, // agent not registered on the loop
        },
        _ => return,
    };
    let api = match build_api_client(state) {
        Ok(a) => a,
        Err(_) => return, // not connected to NeboAI
    };
    let chats = vec![comm::api::AgentChatSync {
        chat_id: chat_id.to_string(),
        title: title.to_string(),
        last_activity_at: None,
    }];
    match api.sync_agent_chats(&loop_agent_id, &chats).await {
        Ok(_) => info!(
            target: "neboai_identity",
            chat_id = %chat_id, loop_agent_id = %loop_agent_id, "loop chat title pushed"
        ),
        Err(e) => warn!(
            target: "neboai_identity",
            chat_id = %chat_id, error = %e, "loop chat title push failed"
        ),
    }
}

// ── Artifact Persistence ────────────────────────────────────────────
//
// After redeem_code() registers the install in the NeboAI cloud DB,
// these functions fetch the actual artifact content and persist locally.
//
// Skills and agents: canonical implementation in tools::persist_skill_from_api
// and tools::persist_agent_from_api. Workflows have a unique DB+filesystem
// persist path that only exists here.

/// Fetch workflow content from NeboAI and persist to DB + filesystem.
///
/// If the API provides a `downloadUrl`, downloads the sealed `.napp` archive
/// and stores it at `nebo/workflows/{slug}/{version}.napp`, then extracts it.
/// Otherwise falls back to writing loose WORKFLOW.md + workflow.json files.
async fn persist_workflow_artifact(
    api: &NeboAIApi,
    artifact_id: &str,
    name: &str,
    code: &str,
    store: &db::Store,
) -> Result<(), String> {
    let detail = api
        .get_skill(artifact_id)
        .await
        .map_err(|e| format!("fetch workflow detail: {e}"))?;

    let manifest_text = tools::extract_manifest_text(&detail).unwrap_or_default();

    // For workflows, manifest is WORKFLOW.md and type_config may hold the definition
    let definition = detail
        .type_config
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_default())
        .unwrap_or_default();

    // Persist to DB
    let _ = store
        .create_workflow(
            artifact_id,
            Some(code),
            name,
            &detail.item.version,
            &definition,
            if manifest_text.is_empty() {
                None
            } else {
                Some(&manifest_text)
            },
            None,
        )
        .map_err(|e| format!("create_workflow: {e}"))?;

    // Marketplace artifacts go to nebo/ namespace (installed)
    let nebo_dir = config::nebo_dir().map_err(|e| format!("nebo_dir: {e}"))?;
    let slug = &detail.item.slug;
    let dir_name = if slug.is_empty() { name } else { slug.as_str() };
    let version = if detail.item.version.is_empty() {
        "1.0.0"
    } else {
        &detail.item.version
    };

    // Try sealed .napp download — use API-provided URL or construct from artifact ID.
    // Include platform so the server can serve the right binary for this OS/arch.
    let platform = napp::plugin::current_platform_key();
    let download_url = detail.download_url.clone().or_else(|| {
        Some(format!(
            "/api/v1/apps/{}/download/{}",
            artifact_id, platform
        ))
    });
    if let Some(ref download_url) = download_url {
        let napp_dir = nebo_dir.join("workflows").join(dir_name);
        std::fs::create_dir_all(&napp_dir).map_err(|e| format!("create workflow dir: {e}"))?;
        let napp_path = napp_dir.join(format!("{}.napp", version));

        match api.download_napp(download_url).await {
            Ok(data) => {
                std::fs::write(&napp_path, &data).map_err(|e| format!("write .napp: {e}"))?;
                tracing::info!(workflow = name, path = %napp_path.display(), size = data.len(), "stored sealed .napp");

                match napp::reader::extract_napp_alongside(&napp_path) {
                    Ok(extract_dir) => {
                        tracing::info!(workflow = name, dir = %extract_dir.display(), "extracted .napp");
                        // Set napp_path on DB record to the sealed archive
                        if let Err(e) =
                            store.set_workflow_napp_path(artifact_id, &napp_path.to_string_lossy())
                        {
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
    std::fs::create_dir_all(&wf_dir).map_err(|e| format!("create workflow dir: {e}"))?;

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

/// Activate the NeboAI connection using stored credentials.
///
/// This is the canonical implementation — called by both startup auto-connect
/// and code handlers. Builds config from stored credentials and connects.
pub async fn activate_neboai(state: &AppState) -> Result<(), NeboError> {
    // Guard against re-entry: if already connected, skip.
    if state.comm_manager.is_connected().await {
        return Ok(());
    }

    let bot_id = config::read_bot_id().ok_or_else(|| NeboError::Internal("no bot_id".into()))?;
    let profiles = state
        .store
        .list_all_active_auth_profiles_by_provider("neboai")
        .unwrap_or_default();
    let profile = profiles
        .first()
        .ok_or_else(|| NeboError::Internal("no NeboAI credentials".into()))?;
    let mut token = if profile.api_key.is_empty() {
        return Err(NeboError::Internal("empty NeboAI token".into()));
    } else {
        profile.api_key.clone()
    };

    // Prefer cached rotated token over DB token — the cache is written immediately
    // on AUTH_OK, so it survives hot-reload/crash where the DB persist hasn't run yet.
    if let Ok(dir) = config::data_dir() {
        let cache_path = dir.join("neboai_token.cache");
        if let Ok(cached) = std::fs::read_to_string(&cache_path) {
            let cached = cached.trim().to_string();
            if !cached.is_empty() && cached != token {
                info!("neboai: using cached rotated token (differs from DB)");
                token = cached;
            }
        }
    }

    let mut config = HashMap::new();
    config.insert("gateway".into(), state.config.neboai.comms_url.clone());
    config.insert("api_server".into(), state.config.neboai.api_url.clone());
    config.insert("bot_id".into(), bot_id);
    config.insert("token".into(), token);
    if let Ok(dir) = config::data_dir() {
        config.insert("data_dir".into(), dir.to_string_lossy().to_string());
    }

    // Pin the PRIMARY agent's identity on CONNECT so the loop's default agent is
    // deterministically "Nebo" (the local `assistant` row) and can never be
    // overwritten by whichever secondary sorts first. Without this the loop has
    // no anchored primary and renames it to the alphabetically-first agent.
    // The handle is sent raw; the loop derives the canonical `bot_<id8>` form
    // (or `bot_<slug>` for a custom handle) — matching `comm::handle`.
    if let Ok(Some(primary)) = state.store.get_agent("assistant") {
        let name = if primary.name.is_empty() { "Nebo".to_string() } else { primary.name };
        // The stored handle is already canonical (`bot_d486d161`). The loop expects
        // a RAW handle and adds the `bot_` prefix itself (via `defaultHandle`), so
        // sending it pre-prefixed makes it re-slugify into garbage (`bot_bot-…`).
        // Strip our prefix → send raw → loop re-derives the same canonical handle.
        let stored = primary.handle.clone().unwrap_or_default();
        let raw_handle = stored.strip_prefix("bot_").unwrap_or(&stored).to_string();
        info!(
            target: "neboai_identity",
            assistant_name = %name,
            stored_handle = %stored,
            sending_raw_handle = %raw_handle,
            assistant_color = ?primary.color,
            "activate_neboai: plumbing primary identity into CONNECT config"
        );
        config.insert("agent_name".into(), name);
        config.insert("agent_handle".into(), raw_handle);
        config.insert("agent_color".into(), primary.color.unwrap_or_default());
    } else {
        info!(target: "neboai_identity", "activate_neboai: no 'assistant' row — CONNECT agent_name=Nebo only");
        config.insert("agent_name".into(), "Nebo".to_string());
    }

    state
        .comm_manager
        .set_active("neboai")
        .await
        .map_err(|e| NeboError::Internal(format!("set_active: {e}")))?;

    let connect_result = state.comm_manager.connect_active(config.clone()).await;

    // If connect fails with stale token, try refreshing via OAuth
    if let Err(ref e) = connect_result {
        let err_msg = e.to_string();
        if err_msg.contains("stale token") || err_msg.contains("auth failed") {
            info!("NeboAI token stale, attempting OAuth refresh");
            if let Some(new_token) = refresh_neboai_token(state, profile).await {
                // Retry connect with fresh token
                let mut retry_config = config;
                retry_config.insert("token".into(), new_token);
                state
                    .comm_manager
                    .connect_active(retry_config)
                    .await
                    .map_err(|e| NeboError::Internal(format!("connect after refresh: {e}")))?;
            } else {
                return Err(NeboError::Internal(format!(
                    "connect: {err_msg} (refresh failed)"
                )));
            }
        } else {
            return Err(NeboError::Internal(format!("connect: {err_msg}")));
        }
    }

    // Persist rotated JWT so next reconnect uses the fresh token
    if let Some(new_token) = state.comm_manager.take_rotated_token().await {
        if let Ok(profs) = state
            .store
            .list_all_active_auth_profiles_by_provider("neboai")
        {
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
                info!("persisted rotated NeboAI token");
            }
        }
    }

    state
        .hub
        .broadcast("settings_updated", serde_json::json!({"commEnabled": true}));

    // Reconcile agents + sync bot identity + refresh license keys in background (non-blocking)
    {
        let st = state.clone();
        tokio::spawn(async move {
            if let Err(e) = reconcile_agents(&st).await {
                warn!(error = %e, "agent reconciliation failed");
            }
            // Sync bot identity (name) to NeboAI
            sync_bot_identity(&st).await;
            // Refresh content protection license keys for sealed .napp files
            if let Err(e) = refresh_license_keys(&st).await {
                warn!(error = %e, "license key refresh failed");
            }
        });
    }

    Ok(())
}

/// Sync the bot's display name to NeboAI from the local agent profile.
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
        Ok(_) => info!(name = %name, "synced bot identity to NeboAI"),
        Err(e) => warn!(error = %e, "failed to sync bot identity"),
    }
}

/// Reconcile agents: sync all local agents (enabled AND disabled) to NeboAI.
/// Only deregister agents that are truly deleted locally, not just paused.
async fn reconcile_agents(state: &AppState) -> Result<(), NeboError> {
    let api = build_api_client(state)?;
    let bot_id = config::read_bot_id().unwrap_or_default();
    let loops = api
        .list_bot_loops()
        .await
        .map_err(|e| NeboError::Internal(format!("list loops: {e}")))?;
    let personal = match loops.first() {
        Some(l) => l,
        None => return Ok(()), // No loops, nothing to reconcile
    };

    // Store personal loop_id for session unification — in memory for this
    // connection AND persisted, so the unification branch is deterministic from
    // the first inbound DM after a restart (before reconcile completes).
    *state.personal_loop_id.write().await = Some(personal.loop_id.clone());
    if let Err(e) = state
        .store
        .set_plugin_setting("neboai", "personal_loop_id", &personal.loop_id)
    {
        warn!(error = %e, "failed to persist personal_loop_id");
    }

    let remote_agents = api
        .list_agents(&personal.loop_id)
        .await
        .map_err(|e| NeboError::Internal(format!("list agents: {e}")))?;

    info!(
        target: "neboai_identity",
        loop_id = %personal.loop_id,
        remote_count = remote_agents.len(),
        remote = ?remote_agents.iter().map(|a| format!("{}#{}", a.slug, &a.id[..a.id.len().min(8)])).collect::<Vec<_>>(),
        "reconcile_agents: START — remote loop agents"
    );

    // Local agents that should appear on the loop as their OWN identity:
    // "Expose to Loop" is on, excluding the primary ("assistant"). The primary
    // is the bot's canonical "Nebo" identity, auto-created and kept current by
    // the gateway (slug "bot_<id8>") — it always shows and must never be
    // registered as a named secondary, or it would duplicate itself.
    // Canonical display name comes from each agent's manifest.json (the source of
    // truth), resolved via the loader by id — NOT the local DB `name`, which can
    // drift to the AGENT.md frontmatter slug. The @handle is bot-scoped
    // (`bot_<id8>_<slug>`) so two bots loading the same agent never collide. We
    // also heal a drifted DB name here so display/reads are correct end-to-end.
    let manifest_name_by_id: std::collections::HashMap<String, String> = state
        .agent_loader
        .list()
        .await
        .into_iter()
        .filter_map(|l| Some((l.id?, l.agent_def.name)))
        .collect();
    let exposed: Vec<(db::models::Agent, String, String)> = state
        .store
        .list_agents(1000, 0)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.id != "assistant" && a.loop_exposed != 0)
        .map(|a| {
            let name = manifest_name_by_id
                .get(&a.id)
                .filter(|n| !n.is_empty())
                .cloned()
                .unwrap_or_else(|| a.name.clone());
            if name != a.name {
                info!(target: "neboai_identity", id = %a.id, db_name = %a.name, manifest_name = %name, "reconcile: healing drifted local name from manifest");
                if let Err(e) = state.store.sync_agent_identity(&a.id, &name, &a.description) {
                    warn!(id = %a.id, error = %e, "reconcile: failed to heal local name (non-fatal)");
                }
            }
            let slug = comm::handle::secondary_handle(&bot_id, &name);
            (a, name, slug)
        })
        .collect();
    let exposed_slugs: std::collections::HashSet<String> =
        exposed.iter().map(|(_, _, slug)| slug.clone()).collect();

    // Deregister remote secondary agents that are no longer exposed locally.
    // Never touch the primary (its slug is bot_<id8> with no further "_").
    for agent in &remote_agents {
        if comm::handle::is_primary_handle(&agent.slug) {
            continue;
        }
        if !exposed_slugs.contains(&agent.slug) {
            info!(agent_slug = %agent.slug, agent_id = %agent.id, "reconcile: deregistering un-exposed agent");
            if let Err(e) = api.deregister_agent(&personal.loop_id, &agent.id).await {
                warn!(agent_slug = %agent.slug, agent_id = %agent.id, error = %e, "reconcile: failed to deregister");
            }
        }
    }

    // Register exposed local agents missing from remote. The server upserts by
    // slug, so this is idempotent (re-registration updates in place).
    // Re-register when the agent is missing OR the loop's stored name differs
    // from the manifest name — the server upserts by slug, so this updates the
    // display name in place (otherwise a previously slug-named agent never gets
    // its real name).
    let remote_by_slug: std::collections::HashMap<String, String> = remote_agents
        .iter()
        .map(|a| (a.slug.clone(), a.name.clone()))
        .collect();
    for (agent, name, slug) in &exposed {
        let needs_register = match remote_by_slug.get(slug) {
            None => true,
            Some(remote_name) => remote_name != name,
        };
        if needs_register {
            let desc = if agent.description.is_empty() {
                None
            } else {
                Some(agent.description.as_str())
            };
            info!(target: "neboai_identity", name = %name, slug = %slug, remote_name = ?remote_by_slug.get(slug), "reconcile: registering/updating exposed agent (manifest name)");
            if let Err(e) = api
                .register_agent(&personal.loop_id, name, slug, desc)
                .await
            {
                warn!(slug = %slug, error = %e, "reconcile: failed to register");
            }
        }
    }

    // Stabilize ids: persist each loop agent's stable UUID onto the matching
    // local agent (`loop_agent_id`) so routing/attribution key on the stable id,
    // never on a name-derived slug. Re-fetch so just-registered secondaries are
    // included with the ids the loop assigned them.
    let slug_to_local: std::collections::HashMap<String, String> = exposed
        .iter()
        .map(|(a, _, slug)| (slug.clone(), a.id.clone()))
        .collect();
    let mut chat_sync_targets: Vec<(String, String)> = Vec::new();
    match api.list_agents(&personal.loop_id).await {
        Ok(final_agents) => {
            info!(target: "neboai_identity", count = final_agents.len(), local_slugs = ?slug_to_local.keys().collect::<Vec<_>>(), "reconcile: stabilize pass — loop agents to map");
            for remote in &final_agents {
                // HARD RULE: the primary (slug "bot_<id8>", no further "_") is
                // ALWAYS the local "assistant". Never map it to a secondary, so
                // the primary (Nebo) can never be overwritten.
                let local_id = if comm::handle::is_primary_handle(&remote.slug) {
                    Some("assistant".to_string())
                } else {
                    slug_to_local.get(&remote.slug).cloned()
                };
                match local_id {
                    Some(local_id) => match state.store.set_agent_loop_agent_id(&local_id, Some(remote.id.as_str())) {
                        Ok(()) => {
                            info!(target: "neboai_identity", local_id = %local_id, loop_agent_id = %remote.id, slug = %remote.slug, "reconcile: STABILIZED loop id");
                            chat_sync_targets.push((local_id, remote.id.clone()));
                        }
                        Err(e) => warn!(target: "neboai_identity", local_id = %local_id, loop_agent_id = %remote.id, error = %e, "reconcile: FAILED to store loop_agent_id"),
                    },
                    None => warn!(target: "neboai_identity", slug = %remote.slug, loop_agent_id = %remote.id, "reconcile: remote agent has NO local match — not stabilized"),
                }
            }
        }
        Err(e) => warn!(target: "neboai_identity", error = %e, "reconcile: stabilize pass — list_agents FAILED"),
    }

    // Per-chat agent spaces: publish each agent's desktop chat list so every
    // chat gets its own loop conversation (the remote emulates the local
    // Threads tab). Additive server-side — loop-created chats are untouched.
    for (local_id, loop_agent_id) in &chat_sync_targets {
        let prefix = format!("agent:{}:", local_id);
        let chats = match state.store.list_chats_by_session_enriched(&prefix) {
            Ok(rows) => rows
                .into_iter()
                .map(|(chat, _, _)| comm::api::AgentChatSync {
                    chat_id: chat.id,
                    title: chat.title,
                    last_activity_at: chrono::DateTime::from_timestamp(chat.updated_at, 0)
                        .map(|t| t.to_rfc3339()),
                })
                .collect::<Vec<_>>(),
            Err(e) => {
                warn!(target: "neboai_identity", local_id = %local_id, error = %e, "reconcile: chat list failed — skipping chats sync");
                continue;
            }
        };
        if chats.is_empty() {
            continue;
        }
        let results = match api.sync_agent_chats(loop_agent_id, &chats).await {
            Ok(results) => {
                info!(target: "neboai_identity", local_id = %local_id, loop_agent_id = %loop_agent_id, count = chats.len(), "reconcile: chats synced");
                results
            }
            Err(e) => {
                warn!(target: "neboai_identity", local_id = %local_id, error = %e, "reconcile: chats sync FAILED");
                continue;
            }
        };

        // Deletion propagation: a chat tombstoned on the loop deletes the
        // desktop copy too (same pathway as DELETE /api/v1/chats/:id). The
        // tombstone survives the additive upsert above, so this also stops
        // the chat from resurrecting on the next sync.
        for r in results.iter().filter(|r| r.deleted) {
            if let Err(e) = state
                .store
                .delete_chat_messages_by_chat_id(&r.chat_id)
                .and_then(|_| state.store.delete_chat(&r.chat_id))
            {
                warn!(target: "neboai_identity", chat_id = %r.chat_id, error = %e, "reconcile: loop-deleted chat — local delete FAILED");
            } else {
                info!(target: "neboai_identity", chat_id = %r.chat_id, local_id = %local_id, "reconcile: chat deleted on loop — local copy removed");
            }
        }

        // One-shot history backfill: a chat whose loop conversation was just
        // created opens EMPTY remotely — mirror its recent desktop messages
        // so the remote shows the same conversation as the local Threads
        // tab. Runs once per chat by construction (created=false afterward).
        let agent_display = state
            .store
            .get_agent(local_id)
            .ok()
            .flatten()
            .map(|a| a.name)
            .unwrap_or_else(|| "Nebo".to_string());
        // Once-guard within this process: reconnect flaps can run two
        // reconciles back-to-back, and the second sync may read head_seq
        // before the first backfill's sends persist — which double-mirrors
        // the chat. head_seq==0 still guards across restarts.
        static BACKFILLED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
            std::sync::OnceLock::new();
        let backfilled = BACKFILLED.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
        for r in results.iter().filter(|r| !r.deleted && (r.created || r.head_seq == 0)) {
            {
                let mut seen = backfilled.lock().unwrap();
                if !seen.insert(r.chat_id.clone()) {
                    continue;
                }
            }
            let msgs = match state.store.get_recent_chat_messages(&r.chat_id, 50) {
                Ok(m) => m,
                Err(e) => {
                    warn!(target: "neboai_identity", chat_id = %r.chat_id, error = %e, "backfill: history load failed");
                    continue;
                }
            };
            let total = msgs.len();
            for m in msgs {
                if m.content.trim().is_empty() {
                    continue;
                }
                let mut meta = std::collections::HashMap::new();
                if m.role == "user" {
                    meta.insert("relay".to_string(), "true".to_string());
                    meta.insert("role".to_string(), "user".to_string());
                    meta.insert("senderName".to_string(), "You".to_string());
                } else {
                    meta.insert("senderName".to_string(), agent_display.clone());
                }
                let msg = comm::CommMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    from: String::new(),
                    to: String::new(),
                    topic: "agent_space".to_string(),
                    conversation_id: r.conversation_id.clone(),
                    msg_type: comm::CommMessageType::Message,
                    content: m.content,
                    metadata: meta,
                    timestamp: 0,
                    human_injected: m.role == "user",
                    human_id: None,
                    task_id: None,
                    correlation_id: None,
                    task_status: None,
                    artifacts: vec![],
                    attachments: vec![],
                    error: None,
                };
                if let Err(e) = state.comm_manager.send(msg).await {
                    warn!(target: "neboai_identity", chat_id = %r.chat_id, error = %e, "backfill: send failed — aborting this chat");
                    break;
                }
                // Light pacing so a multi-chat backfill doesn't trip the
                // gateway's send rate limit.
                tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            }
            info!(target: "neboai_identity", chat_id = %r.chat_id, count = total, "backfill: chat history mirrored");
        }
    }

    info!("agent reconciliation complete");
    Ok(())
}

/// Try to refresh the NeboAI OAuth token using the stored refresh_token.
/// Returns the new access_token if successful, or None.
async fn refresh_neboai_token(
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

    let api_url = &state.config.neboai.api_url;
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
    info!("NeboAI OAuth token refreshed successfully");

    Some(token_resp.access_token)
}

/// Core NEBO code redemption logic. Called by both:
/// - `handle_nebo_code()` (chat-based code interception)
/// - `connect_handler()` (HTTP POST /neboai/connect)
pub async fn redeem_nebo_code(state: &AppState, code: &str) -> Result<String, NeboError> {
    let bot_id = config::ensure_bot_id();
    let api_server = state.config.neboai.api_url.clone();

    // 1. Redeem code (pre-auth, standalone)
    let resp = comm::api::redeem_code(&api_server, code, "nebo-rs", "desktop", &bot_id)
        .await
        .map_err(|e| NeboError::Internal(format!("redeem failed: {e}")))?;

    // 2. Store bot_id + connection token
    if let Err(e) = config::write_bot_id(&bot_id) {
        warn!("failed to persist bot_id: {}", e);
    }

    // Store connection token as auth profile
    crate::handlers::neboai::store_neboai_profile(
        state,
        &api_server,
        &resp.id,                 // owner_id from redeem response
        &resp.owner_email,        // owner email from redeem response
        &resp.owner_display_name, // owner display name from redeem response
        &resp.connection_token,
        "",    // no refresh token from code redemption
        false, // not a janus provider
    )
    .map_err(|e| NeboError::Internal(format!("store profile: {e}")))?;

    // 3. Activate connection
    activate_neboai(state).await?;

    Ok(bot_id)
}

/// Register an agent in the owner's personal loop after role install/activate.
///
/// The gateway auto-creates an agent space conversation and subscribes
/// the bot to it. Errors are non-fatal — logged by callers.
pub(crate) async fn register_agent_in_loop(
    state: &AppState,
    name: &str,
) -> Result<(), NeboError> {
    // Canonical bot-scoped handle — the single source of truth (handle.rs).
    // Deriving it here keeps every caller from open-coding a divergent slug.
    let bot_id = config::read_bot_id().unwrap_or_default();
    let slug = comm::handle::secondary_handle(&bot_id, name);

    let api = build_api_client(state)?;
    let loops = api
        .list_bot_loops()
        .await
        .map_err(|e| NeboError::Internal(format!("list loops: {e}")))?;
    let personal = loops
        .first()
        .ok_or_else(|| NeboError::Internal("bot not in any loop".into()))?;

    // Store personal loop_id for session unification (memory + persisted; see
    // reconcile_agents for why persistence matters across restarts).
    *state.personal_loop_id.write().await = Some(personal.loop_id.clone());
    if let Err(e) = state
        .store
        .set_plugin_setting("neboai", "personal_loop_id", &personal.loop_id)
    {
        warn!(error = %e, "failed to persist personal_loop_id");
    }

    // register_agent upserts by slug (see reconcile_agents), so this is idempotent.
    api.register_agent(&personal.loop_id, name, &slug, None)
        .await
        .map_err(|e| NeboError::Internal(format!("register agent: {e}")))?;
    info!(agent = %name, slug = %slug, loop_id = %personal.loop_id, "registered agent in loop");
    Ok(())
}

/// Deregister an agent from the owner's personal loop.
pub(crate) async fn deregister_agent_from_loop(
    state: &AppState,
    name: &str,
) -> Result<(), NeboError> {
    // Same canonical handle as registration — match the remote agent on it.
    let bot_id = config::read_bot_id().unwrap_or_default();
    let slug = comm::handle::secondary_handle(&bot_id, name);

    let api = build_api_client(state)?;
    let loops = api
        .list_bot_loops()
        .await
        .map_err(|e| NeboError::Internal(format!("list loops: {e}")))?;
    let personal = loops
        .first()
        .ok_or_else(|| NeboError::Internal("bot not in any loop".into()))?;
    // NeboAI DELETE requires the agent UUID, not the slug.
    // Look up the remote agent by its canonical slug to get its UUID.
    let agents = api
        .list_agents(&personal.loop_id)
        .await
        .map_err(|e| NeboError::Internal(format!("list agents: {e}")))?;
    let remote = agents
        .iter()
        .find(|a| a.slug == slug)
        .ok_or_else(|| {
            NeboError::Internal(format!("agent '{}' not found on NeboAI", slug))
        })?;
    api.deregister_agent(&personal.loop_id, &remote.id)
        .await
        .map_err(|e| NeboError::Internal(format!("deregister agent: {e}")))?;
    info!(agent = %name, slug = %slug, remote_id = %remote.id, loop_id = %personal.loop_id, "deregistered agent from loop");
    Ok(())
}

/// Refresh license keys for sealed .napp files from NeboAI.
///
/// Called after NeboAI connection is established (in activate_neboai).
/// Fetches fresh keys for all sealed artifacts, stores them in the DB cache,
/// and triggers a skill reload so newly unlocked content becomes available.
pub(crate) async fn refresh_license_keys(state: &AppState) -> Result<(), NeboError> {
    // Renew keys for all sealed artifacts already in the cache. Rows are seeded at
    // install time (see tools::fetch_and_store_license_keys); this keeps them fresh
    // before their TTL expires.
    let artifact_ids: Vec<String> = state
        .store
        .list_license_key_artifact_ids()
        .unwrap_or_default();

    if artifact_ids.is_empty() {
        return Ok(());
    }

    let api = build_api_client(state)?;

    // Register bot (idempotent) before fetching keys
    let platform = napp::plugin::current_platform_key();
    let version = env!("CARGO_PKG_VERSION");
    if let Err(e) = api.register_bot(&platform, version).await {
        debug!(error = %e, "bot registration failed (non-fatal)");
    }

    let keys = tools::fetch_and_store_license_keys(&api, &state.store, &artifact_ids, "skill")
        .await
        .map_err(NeboError::Internal)?;

    if !keys.is_empty() {
        let refreshed = keys.len();
        info!(
            refreshed,
            total = artifact_ids.len(),
            "refreshed license keys"
        );
        // Hand the fresh keys to both loaders and reload so newly unlocked sealed
        // skills AND agents become available without a restart.
        state.skill_loader.set_license_keys(keys.clone()).await;
        state.agent_loader.set_license_keys(keys).await;
        state.skill_loader.load_all().await;
        state.agent_loader.load_all().await;
        state.hub.broadcast(
            "license_keys_refreshed",
            serde_json::json!({"count": refreshed}),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_code_valid() {
        assert!(matches!(
            detect_code("NEBO-A1B2-C3D4"),
            Some((CodeType::Nebo, _))
        ));
        assert!(matches!(
            detect_code("SKIL-0000-ZZZZ"),
            Some((CodeType::Skill, _))
        ));
        assert!(matches!(
            detect_code("WORK-1234-5678"),
            Some((CodeType::Work, _))
        ));
        assert!(matches!(
            detect_code("AGNT-9999-AAAA"),
            Some((CodeType::Agent, _))
        ));
        assert!(matches!(
            detect_code("LOOP-QRST-VWXY"),
            Some((CodeType::Loop, _))
        ));
        assert!(matches!(
            detect_code("PLUG-A1B2-C3D4"),
            Some((CodeType::Plugin, _))
        ));
        assert!(matches!(
            detect_code("COLL-A1B2-C3D4"),
            Some((CodeType::Collection, _))
        ));
        assert!(matches!(
            detect_code("CONN-A1B2-C3D4"),
            Some((CodeType::Connection, _))
        ));
    }

    #[test]
    fn test_detect_code_case_insensitive() {
        assert!(matches!(
            detect_code("nebo-a1b2-c3d4"),
            Some((CodeType::Nebo, _))
        ));
        assert!(matches!(
            detect_code("skil-0000-ZZZZ"),
            Some((CodeType::Skill, _))
        ));
    }

    #[test]
    fn test_detect_code_trimmed() {
        assert!(matches!(
            detect_code("  NEBO-A1B2-C3D4  "),
            Some((CodeType::Nebo, _))
        ));
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
