use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Json;
use tracing::{info, warn};

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

/// Rebuild AI providers from auth_profiles and reload them on the harness,
/// through the ONE builder startup uses (`build_providers`) — so a reload
/// keeps what startup registers (the linked provider included). CLIs are
/// re-detected live, so toggling a CLI provider works even when the app was
/// launched from Finder/Start Menu with a minimal PATH.
pub(crate) async fn reload_providers(
    store: &Arc<db::Store>,
    cfg: &config::Config,
    harness: &agent::Harness,
    local_host: Option<&Arc<ai::LocalHost>>,
) {
    let providers = crate::build_providers(store, cfg, Some(&config::detect_all_clis()), local_host);
    info!(count = providers.len(), "reloading providers");
    harness.reload_providers(providers).await;

    // Refresh the DB-held models in the selector (they're not in yaml)
    crate::inject_db_models(store, harness.selector(), "ollama");
    crate::inject_db_models(store, harness.selector(), "janus");
}

/// GET /api/v1/providers
pub async fn list_providers(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let profiles = state
        .store
        .list_auth_profiles()
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"profiles": profiles})))
}

/// POST /api/v1/providers
pub async fn create_provider(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let name = body["name"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("name required".into())))?;
    let provider = body["provider"].as_str().ok_or_else(|| {
        to_error_response(types::NeboError::Validation("provider required".into()))
    })?;
    let api_key = body["apiKey"].as_str().unwrap_or("");
    let model = body["model"].as_str();
    let base_url = body["baseUrl"].as_str();
    let priority = body["priority"].as_i64().unwrap_or(50);
    let auth_type = body["authType"].as_str();
    let metadata = body.get("metadata").map(|v| v.to_string());

    let id = uuid::Uuid::new_v4().to_string();
    let profile = state
        .store
        .create_auth_profile(
            &id,
            name,
            provider,
            api_key,
            model,
            base_url,
            priority,
            1, // is_active
            auth_type,
            metadata.as_deref(),
        )
        .map_err(to_error_response)?;

    // Reload providers on the runner
    reload_providers(&state.store, &state.config, &state.harness, state.local_host.as_ref()).await;

    Ok(Json(serde_json::json!(profile)))
}

/// GET /api/v1/providers/:id
pub async fn get_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let profile = state
        .store
        .get_auth_profile(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(serde_json::json!(profile)))
}

/// PUT /api/v1/providers/:id
pub async fn update_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let existing = state
        .store
        .get_auth_profile(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let name = body["name"].as_str().unwrap_or(&existing.name);
    let api_key = body["apiKey"].as_str().unwrap_or(&existing.api_key);
    // Absent key = keep the stored value; explicit null or "" = CLEAR the
    // model pin (writes NULL). Before this, a model could be set but never
    // unset — a bot-profile pin would shadow account-level routing policy
    // forever (found 2026-09-01 clearing redundant glm pins).
    let model = match body.get("model") {
        None => existing.model.as_deref(),
        Some(v) => v.as_str().filter(|s| !s.trim().is_empty()),
    };
    let base_url = body["baseUrl"].as_str().or(existing.base_url.as_deref());
    let priority = body["priority"]
        .as_i64()
        .unwrap_or(existing.priority.unwrap_or(50));
    let auth_type = body["authType"].as_str().or(existing.auth_type.as_deref());
    // Merge incoming metadata into existing metadata (don't replace wholesale)
    let metadata = {
        let mut merged: serde_json::Map<String, serde_json::Value> = existing
            .metadata
            .as_ref()
            .and_then(|m| serde_json::from_str(m).ok())
            .unwrap_or_default();
        if let Some(incoming) = body.get("metadata").and_then(|v| v.as_object()) {
            for (k, v) in incoming {
                merged.insert(k.clone(), v.clone());
            }
        }
        if merged.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(merged).to_string())
        }
    };

    state
        .store
        .update_auth_profile(
            &id,
            name,
            api_key,
            model,
            base_url,
            priority,
            auth_type,
            metadata.as_deref(),
        )
        .map_err(to_error_response)?;

    // Handle isActive toggle (separate DB column, not part of update_auth_profile)
    if let Some(is_active) = body.get("isActive").and_then(|v| v.as_bool()) {
        state
            .store
            .toggle_auth_profile(&id, if is_active { 1 } else { 0 })
            .map_err(to_error_response)?;
    }

    // Reload providers on the runner
    reload_providers(&state.store, &state.config, &state.harness, state.local_host.as_ref()).await;

    let updated = state
        .store
        .get_auth_profile(&id)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!(updated)))
}

/// DELETE /api/v1/providers/:id
pub async fn delete_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .delete_auth_profile(&id)
        .map_err(to_error_response)?;
    // Reload providers on the runner
    reload_providers(&state.store, &state.config, &state.harness, state.local_host.as_ref()).await;
    Ok(Json(serde_json::json!({"success": true})))
}

/// POST /api/v1/providers/:id/test
pub async fn test_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let profile = state
        .store
        .get_auth_profile(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Basic validation first
    if profile.api_key.is_empty() && profile.auth_type.as_deref() != Some("local") {
        return Ok(Json(serde_json::json!({
            "success": false,
            "provider": profile.provider,
            "message": "Missing API key",
        })));
    }

    // Build a temporary provider and try a minimal API call
    let model = profile.model.clone().unwrap_or_default();
    let test_result: Result<String, String> = match profile.provider.as_str() {
        "anthropic" => {
            let provider = ai::AnthropicProvider::new(profile.api_key.clone(), model);
            test_provider_connection(&provider).await
        }
        "openai" => {
            let provider = ai::OpenAIProvider::new(profile.api_key.clone(), model);
            test_provider_connection(&provider).await
        }
        "deepseek" => {
            let base_url = profile
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.deepseek.com/v1".into());
            let provider =
                ai::OpenAIProvider::with_base_url(profile.api_key.clone(), model, base_url);
            test_provider_connection(&provider).await
        }
        "google" => {
            let provider = ai::GeminiProvider::new(profile.api_key.clone(), model);
            test_provider_connection(&provider).await
        }
        "ollama" => {
            let base_url = profile
                .base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434".into());
            let provider = ai::OllamaProvider::new(base_url, model);
            test_provider_connection(&provider).await
        }
        "neboai" => {
            let janus_url = &state.config.neboai.janus_url;
            let bot_id = config::read_bot_id().unwrap_or_default();
            let mut provider = ai::OpenAIProvider::with_base_url(
                crate::janus_api_key(state.store.clone()),
                model,
                format!("{}/v1", janus_url),
            );
            provider.set_provider_id("janus");
            if !bot_id.is_empty() {
                provider.set_bot_id(bot_id);
            }
            test_provider_connection(&provider).await
        }
        _ => Err(format!("Unknown provider type: {}", profile.provider)),
    };

    match test_result {
        Ok(msg) => Ok(Json(serde_json::json!({
            "success": true,
            "provider": profile.provider,
            "message": msg,
        }))),
        Err(msg) => Ok(Json(serde_json::json!({
            "success": false,
            "provider": profile.provider,
            "message": msg,
        }))),
    }
}

/// Test a provider by sending a minimal chat request.
async fn test_provider_connection(provider: &dyn ai::Provider) -> Result<String, String> {
    let req = ai::ChatRequest {
        tool_credential: None,
        chat_id: String::new(),
        ask_channels: None,
        permission_mode: None,
        tool_choice: Default::default(),
        messages: vec![ai::Message {
            role: "user".into(),
            content: "Say OK".into(),
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 16,
        temperature: 0.0,
        system: String::new(),
        model: String::new(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
        trace: ai::RequestTrace::new("provider_test"),
    };

    match tokio::time::timeout(std::time::Duration::from_secs(15), provider.stream(&req)).await {
        Ok(Ok(mut rx)) => {
            let mut got_text = false;
            while let Some(event) = rx.recv().await {
                if event.event_type == ai::StreamEventType::Text {
                    got_text = true;
                }
                if event.event_type == ai::StreamEventType::Error {
                    return Err(event.error.unwrap_or_else(|| "Unknown error".into()));
                }
            }
            if got_text {
                Ok("Connection successful — provider responded".into())
            } else {
                Err("No response received from provider".into())
            }
        }
        Ok(Err(e)) => Err(format!("Provider error: {}", e)),
        Err(_) => Err("Connection timed out after 15 seconds".into()),
    }
}

/// The models every picker reads (the composer's model menu, an employee's
/// model, Settings → Routing, the phone), grouped by provider: chat models
/// only. An embedding, audio or image model is never offered where a chat
/// model is chosen; the embedding code path reads its own model.
fn picker_models(
    all_models: &[db::models::ProviderModel],
) -> std::collections::HashMap<String, Vec<serde_json::Value>> {
    let mut models: std::collections::HashMap<String, Vec<serde_json::Value>> =
        std::collections::HashMap::new();

    for m in all_models {
        let capabilities: Vec<String> = m
            .capabilities
            .as_ref()
            .and_then(|c| serde_json::from_str(c).ok())
            .unwrap_or_default();
        let kind: Vec<String> = m
            .kind
            .as_ref()
            .and_then(|k| serde_json::from_str(k).ok())
            .unwrap_or_default();

        if !agent::selector::is_chat_model(&m.model_id, &capabilities, &kind) {
            continue;
        }
        let mut info = serde_json::json!({
            "id": m.model_id,
            "displayName": m.display_name,
            "description": m.description,
            "contextWindow": m.context_window.unwrap_or(0),
            "capabilities": capabilities,
            "kind": kind,
            "preferred": m.preferred.unwrap_or(0) == 1,
            "isActive": m.is_active.unwrap_or(0) == 1,
        });

        // Add pricing if available
        if m.input_price.is_some() || m.output_price.is_some() {
            info["pricing"] = serde_json::json!({
                "input": m.input_price.unwrap_or(0.0),
                "output": m.output_price.unwrap_or(0.0),
            });
        }

        models.entry(m.provider.clone()).or_default().push(info);
    }
    models
}

/// GET /api/v1/models — returns model catalog from DB + routing config from YAML.
pub async fn list_models(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    // Opening the list is the moment it has to be current: pull from Janus first.
    if let Err(e) = crate::sync_janus_models(&state.store, &state.config).await {
        warn!(error = %e, "Janus model list sync failed; showing the last copy");
    }
    // Read models from the database (source of truth for model availability)
    let all_models = state
        .store
        .list_all_provider_models()
        .map_err(to_error_response)?;

    let models = picker_models(&all_models);

    // Routing config comes from the YAML catalog (not per-model data).
    // Load fresh from disk so toggling CLI providers / models is reflected immediately.
    let cfg = config::ModelsConfig::load();
    // The synced speeds must be known to the selector, or a pin like
    // "janus/nebo-1-pro" fuzzy-matches to the nearest name it does know.
    crate::inject_db_models(&state.store, state.harness.selector(), "janus");
    let user_aliases: std::collections::HashMap<String, String> =
        cfg.aliases.iter().map(|a| (a.alias.clone(), a.model_id.clone())).collect();
    state.harness.selector().rebuild_fuzzy(&user_aliases);

    // Task routing
    let task_routing = cfg.task_routing.as_ref().map(|tr| {
        serde_json::json!({
            "vision": tr.vision,
            "audio": tr.audio,
            "reasoning": tr.reasoning,
            "code": tr.code,
            "general": tr.general,
            "aux": tr.aux,
            "fallbacks": tr.fallbacks,
        })
    });

    // Lane routing
    let lane_routing = cfg.lane_routing.as_ref().map(|lr| {
        let mut m = serde_json::Map::new();
        if !lr.heartbeat.is_empty() {
            m.insert(
                "heartbeat".into(),
                serde_json::Value::String(lr.heartbeat.clone()),
            );
        }
        if !lr.events.is_empty() {
            m.insert(
                "events".into(),
                serde_json::Value::String(lr.events.clone()),
            );
        }
        if !lr.comm.is_empty() {
            m.insert("comm".into(), serde_json::Value::String(lr.comm.clone()));
        }
        if !lr.subagent.is_empty() {
            m.insert(
                "subagent".into(),
                serde_json::Value::String(lr.subagent.clone()),
            );
        }
        serde_json::Value::Object(m)
    });

    // Aliases
    let aliases: Vec<serde_json::Value> = cfg
        .aliases
        .iter()
        .map(|a| serde_json::json!({ "alias": a.alias, "modelId": a.model_id }))
        .collect();

    // CLI availability
    let cli = &state.cli_statuses;
    let available_clis = serde_json::json!({
        "claude": cli.claude.installed,
        "codex": cli.codex.installed,
        "gemini": cli.gemini.installed,
    });

    // Detailed CLI statuses
    let cli_statuses = serde_json::json!({
        "claude": {
            "installed": cli.claude.installed,
            "authenticated": cli.claude.authenticated,
            "version": cli.claude.version,
        },
        "codex": {
            "installed": cli.codex.installed,
            "authenticated": cli.codex.authenticated,
            "version": cli.codex.version,
        },
        "gemini": {
            "installed": cli.gemini.installed,
            "authenticated": cli.gemini.authenticated,
            "version": cli.gemini.version,
        },
    });

    // CLI providers from config, enriched with install status
    let cli_providers: Vec<serde_json::Value> = cfg
        .cli_providers
        .iter()
        .map(|cp| {
            serde_json::json!({
                "id": cp.id,
                "displayName": cp.display_name,
                "command": cp.command,
                "installHint": cp.install_hint,
                "models": cp.models,
                "defaultModel": cp.default_model,
                "active": cp.is_active(),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "models": models,
        "taskRouting": task_routing,
        "laneRouting": lane_routing,
        "aliases": aliases,
        "availableCLIs": available_clis,
        "cliStatuses": cli_statuses,
        "cliProviders": cli_providers,
    })))
}

/// PUT /api/v1/models/{provider}/{modelId} — toggle model active/preferred in DB.
pub async fn update_model(
    State(state): State<AppState>,
    Path((provider, model_id)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    // Find the model in the DB
    let model = state
        .store
        .get_provider_model_by_model_id(&provider, &model_id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Update active status
    if let Some(active) = body.get("active").and_then(|v| v.as_bool()) {
        state
            .store
            .update_provider_model_active(&model.id, if active { 1 } else { 0 })
            .map_err(to_error_response)?;
    }

    // Update preferred status
    if let Some(preferred) = body.get("preferred").and_then(|v| v.as_bool()) {
        state
            .store
            .update_provider_model_preferred(&model.id, if preferred { 1 } else { 0 })
            .map_err(to_error_response)?;
    }

    // Janus cascade: when no chat-capable Janus models remain active,
    // disable ALL Janus models (embeddings cost money too).
    if provider == "janus" {
        let has_active_chat = state
            .store
            .list_active_provider_models("janus")
            .unwrap_or_default()
            .iter()
            .any(|m| {
                let caps: Vec<String> = m
                    .capabilities
                    .as_ref()
                    .and_then(|c| serde_json::from_str(c).ok())
                    .unwrap_or_default();
                caps.iter().any(|c| c == "streaming" || c == "tools")
            });

        if !has_active_chat {
            // No chat models active → disable ALL Janus models (embeddings too)
            if let Ok(all_janus) = state.store.list_provider_models("janus") {
                for m in &all_janus {
                    if m.is_active.unwrap_or(0) == 1 {
                        let _ = state.store.update_provider_model_active(&m.id, 0);
                    }
                }
            }
            info!("janus chat model disabled — cascade-disabled all janus models (embeddings)");
        }
    }

    // Reload providers so model toggle takes effect immediately
    // (e.g., disabling all Janus models removes the Janus provider)
    reload_providers(&state.store, &state.config, &state.harness, state.local_host.as_ref()).await;

    Ok(Json(serde_json::json!({
        "message": format!("Model {} updated", model_id),
    })))
}

/// PUT /api/v1/models/cli/{cliId} — toggle CLI provider in YAML config.
pub async fn update_cli_provider(
    State(state): State<AppState>,
    Path(cli_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let active = body
        .get("active")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| {
            to_error_response(types::NeboError::Validation(
                "active field is required".into(),
            ))
        })?;

    // CLI providers are config, stored in models.yaml
    let mut cfg = config::ModelsConfig::load();
    cfg.set_cli_provider_active(&cli_id, active)
        .map_err(|e| to_error_response(types::NeboError::Validation(e)))?;

    // Reload providers so the toggle takes effect immediately
    reload_providers(&state.store, &state.config, &state.harness, state.local_host.as_ref()).await;

    Ok(Json(serde_json::json!({
        "message": format!("CLI provider {} updated", cli_id),
    })))
}

/// PUT /api/v1/models/config — update default model selection in YAML config.
pub async fn update_model_config(
    State(_state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let mut cfg = config::ModelsConfig::load();

    if cfg.defaults.is_none() {
        cfg.defaults = Some(config::models::Defaults {
            // Routing settings do not edit the escalation model; keep what the file says.
            escalation: cfg.defaults.as_ref().map(|d| d.escalation.clone()).unwrap_or_default(),
            primary: String::new(),
            fallbacks: Vec::new(),
        });
    }

    if let Some(primary) = body.get("primary").and_then(|v| v.as_str()) {
        if !primary.is_empty() {
            cfg.defaults.as_mut().unwrap().primary = primary.to_string();
        }
    }

    if let Some(fallbacks) = body.get("fallbacks").and_then(|v| v.as_array()) {
        cfg.defaults.as_mut().unwrap().fallbacks = fallbacks
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }

    cfg.save()
        .map_err(|e| to_error_response(types::NeboError::Server(e)))?;

    let primary = cfg
        .defaults
        .as_ref()
        .map(|d| d.primary.as_str())
        .unwrap_or("");

    Ok(Json(serde_json::json!({
        "success": true,
        "primary": primary,
    })))
}

/// PUT /api/v1/models/task-routing — update routing config in YAML.
pub async fn update_task_routing(
    State(_state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let mut cfg = config::ModelsConfig::load();

    // Update task routing
    let tr = cfg
        .task_routing
        .get_or_insert_with(|| config::models::TaskRouting {
            vision: String::new(),
            audio: String::new(),
            reasoning: String::new(),
            code: String::new(),
            general: String::new(),
            aux: String::new(),
            fallbacks: std::collections::HashMap::new(),
        });
    if let Some(v) = body.get("vision").and_then(|v| v.as_str()) {
        tr.vision = v.to_string();
    }
    if let Some(v) = body.get("audio").and_then(|v| v.as_str()) {
        tr.audio = v.to_string();
    }
    if let Some(v) = body.get("reasoning").and_then(|v| v.as_str()) {
        tr.reasoning = v.to_string();
    }
    if let Some(v) = body.get("code").and_then(|v| v.as_str()) {
        tr.code = v.to_string();
    }
    if let Some(v) = body.get("general").and_then(|v| v.as_str()) {
        tr.general = v.to_string();
    }
    if let Some(v) = body.get("aux").and_then(|v| v.as_str()) {
        tr.aux = v.to_string();
    }
    if let Some(fallbacks) = body.get("fallbacks").and_then(|v| v.as_object()) {
        let mut fb = std::collections::HashMap::new();
        for (k, v) in fallbacks {
            if let Some(arr) = v.as_array() {
                fb.insert(
                    k.clone(),
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                );
            }
        }
        tr.fallbacks = fb;
    }

    // Update lane routing
    if let Some(lr_val) = body.get("laneRouting").and_then(|v| v.as_object()) {
        let lr = cfg
            .lane_routing
            .get_or_insert_with(|| config::models::LaneRouting {
                heartbeat: String::new(),
                events: String::new(),
                comm: String::new(),
                subagent: String::new(),
            });
        if let Some(v) = lr_val.get("heartbeat").and_then(|v| v.as_str()) {
            lr.heartbeat = v.to_string();
        }
        if let Some(v) = lr_val.get("events").and_then(|v| v.as_str()) {
            lr.events = v.to_string();
        }
        if let Some(v) = lr_val.get("comm").and_then(|v| v.as_str()) {
            lr.comm = v.to_string();
        }
        if let Some(v) = lr_val.get("subagent").and_then(|v| v.as_str()) {
            lr.subagent = v.to_string();
        }
    }

    // Update aliases
    if let Some(aliases) = body.get("aliases").and_then(|v| v.as_array()) {
        cfg.aliases = aliases
            .iter()
            .filter_map(|a| {
                let alias = a.get("alias")?.as_str()?.to_string();
                let model_id = a.get("modelId")?.as_str()?.to_string();
                Some(config::models::ModelAlias { alias, model_id })
            })
            .collect();
    }

    cfg.save()
        .map_err(|e| to_error_response(types::NeboError::Server(e)))?;

    Ok(Json(serde_json::json!({
        "message": "Task routing updated successfully",
    })))
}

/// GET /api/v1/local-models/status
/// Checks Ollama availability, syncs discovered models into the DB (inactive by
/// default), and removes stale models that Ollama no longer reports.
pub async fn local_models_status(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let available = ai::providers::ollama::check_ollama_available("").await;
    if !available {
        return Ok(Json(serde_json::json!({
            "available": false,
            "models": [],
        })));
    }

    let model_names = ai::providers::ollama::list_ollama_models("")
        .await
        .unwrap_or_default();

    // Sync discovered models into the DB so they appear in the model catalog.
    // New models default to is_active=false — the user must explicitly enable them.
    let existing = state
        .store
        .list_provider_models("ollama")
        .unwrap_or_default();
    let existing_ids: std::collections::HashSet<&str> =
        existing.iter().map(|m| m.model_id.as_str()).collect();

    for name in &model_names {
        if !existing_ids.contains(name.as_str()) {
            let id = format!("ollama/{}", name);
            // Derive a friendly display name: strip ":latest", title-case
            let display = name
                .trim_end_matches(":latest")
                .replace(':', " ")
                .replace('-', " ");
            let display = display
                .split_whitespace()
                .map(|w| {
                    let mut c = w.chars();
                    match c.next() {
                        Some(first) => {
                            let upper: String = first.to_uppercase().collect();
                            format!("{}{}", upper, c.as_str())
                        }
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            let _ = state.store.upsert_provider_model(
                &id,
                "ollama",
                name,
                &display,
                None,
                Some(128_000), // sensible default
                None,
                None,
                Some("[\"streaming\",\"tools\",\"code\"]"),
                None,
                None,
                false, // default_active = false
            );
        }
    }

    // Remove DB models that Ollama no longer reports
    let live: std::collections::HashSet<&str> = model_names.iter().map(|n| n.as_str()).collect();
    for m in &existing {
        if !live.contains(m.model_id.as_str()) {
            let _ = state.store.delete_provider_model(&m.id);
        }
    }

    Ok(Json(serde_json::json!({
        "available": true,
        "models": model_names,
    })))
}

#[cfg(test)]
mod picker_tests {
    use super::*;

    fn row(provider: &str, id: &str, caps: &str) -> db::models::ProviderModel {
        db::models::ProviderModel {
            id: format!("{provider}/{id}"),
            provider: provider.into(),
            model_id: id.into(),
            display_name: id.into(),
            description: None,
            is_active: Some(1),
            is_default: None,
            context_window: Some(200_000),
            input_price: None,
            output_price: None,
            capabilities: Some(caps.into()),
            kind: None,
            preferred: None,
            seeded_version: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// The owner's provider_models rows: the list every picker reads holds
    /// the chat speeds and never an embedding model.
    #[test]
    fn the_picker_list_holds_no_embedding_model() {
        let chat = r#"["vision","tools","streaming","code","reasoning"]"#;
        let rows = [
            row("janus", "nebo-1", chat),
            row("janus", "nebo-embed-small", r#"["embeddings"]"#),
            row("janus", "nebo-embed-large", r#"["embeddings"]"#),
            row("janus", "nebo-1-pro", chat),
            row("openai", "text-embedding-3-small", "[]"),
        ];
        let listed = picker_models(&rows);
        let ids: Vec<&str> = listed.values().flatten().filter_map(|m| m["id"].as_str()).collect();
        assert_eq!(listed["janus"].len(), 2, "{ids:?}");
        assert!(ids.iter().all(|id| !id.contains("embed")), "{ids:?}");
    }
}

#[cfg(test)]
mod reload_tests {
    use std::sync::Arc;

    /// Pairing by a NEBO code saves the account the way the redeem does
    /// (`janus_provider` not asked for) and reloads: Janus is live at once,
    /// as after OAuth, and the reload keeps the linked provider startup
    /// registers (it used to rebuild without it, so every provider change
    /// cut linked employees off until a restart).
    #[tokio::test]
    async fn a_code_pairing_leaves_janus_live_and_keeps_the_linked_provider() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("nebo.db").to_string_lossy()).unwrap());
        crate::seed_models_from_catalog(&store, &config::ModelsConfig::load());
        let cfg = config::Config::default();
        let harness = agent::Harness::new(
            store.clone(),
            Arc::new(tools::Registry::new(Arc::new(agent::Check::new(store.clone())))),
            Vec::new(),
            agent::selector::ModelSelector::new(Default::default()),
            Arc::new(agent::ConcurrencyController::new(Some(2))),
            Arc::new(napp::HookDispatcher::new()),
            None,
            Default::default(),
            None,
        );

        super::super::neboai::store_neboai_profile(
            &store,
            "https://api.example.com",
            "owner-1",
            "owner@example.com",
            "Owner",
            "connection-token",
            "",
            false,
        )
        .unwrap();
        super::reload_providers(&store, &cfg, &harness, None).await;

        let ids: Vec<String> = harness.providers().read().await.iter().map(|p| p.id().to_string()).collect();
        assert!(ids.iter().any(|id| id == "janus"), "Janus is live after a code pairing: {ids:?}");
        assert!(ids.iter().any(|id| id == ai::providers::linked::ID), "the reload keeps the linked provider: {ids:?}");
    }
}
