use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Json;
use tracing::{info, warn};

use crate::state::AppState;
use super::{to_error_response, HandlerResult};

/// Rebuild AI providers from auth_profiles and reload them on the runner.
async fn reload_providers(state: &AppState) {
    let profiles = match state.store.list_auth_profiles() {
        Ok(p) => p,
        Err(e) => {
            warn!("failed to load auth profiles for reload: {}", e);
            return;
        }
    };

    let models_cfg = config::ModelsConfig::load();

    let mut providers: Vec<Arc<dyn ai::Provider>> = Vec::new();
    for profile in &profiles {
        if profile.is_active.unwrap_or(0) == 0 {
            continue;
        }
        let provider: Option<Arc<dyn ai::Provider>> = match profile.provider.as_str() {
            "anthropic" => {
                let default_model = models_cfg.default_model_for_provider("anthropic")
                    .unwrap_or_else(|| "claude-sonnet-4-5-20250929".into());
                Some(Arc::new(ai::AnthropicProvider::new(
                    profile.api_key.clone(),
                    profile.model.clone().unwrap_or(default_model),
                )))
            }
            "openai" => {
                let default_model = models_cfg.default_model_for_provider("openai")
                    .unwrap_or_else(|| "gpt-5.2".into());
                Some(Arc::new(ai::OpenAIProvider::new(
                    profile.api_key.clone(),
                    profile.model.clone().unwrap_or(default_model),
                )))
            }
            "deepseek" => {
                let default_model = models_cfg.default_model_for_provider("deepseek")
                    .unwrap_or_else(|| "deepseek-chat".into());
                let mut p = ai::OpenAIProvider::with_base_url(
                    profile.api_key.clone(),
                    profile.model.clone().unwrap_or(default_model),
                    profile.base_url.clone().unwrap_or_else(|| "https://api.deepseek.com/v1".into()),
                );
                p.set_provider_id("deepseek");
                Some(Arc::new(p))
            }
            "google" => {
                let default_model = models_cfg.default_model_for_provider("google")
                    .unwrap_or_else(|| "gemini-3-flash".into());
                Some(Arc::new(ai::GeminiProvider::new(
                    profile.api_key.clone(),
                    profile.model.clone().unwrap_or(default_model),
                )))
            }
            "ollama" => {
                let default_model = models_cfg.default_model_for_provider("ollama")
                    .unwrap_or_else(|| "qwen3:4b".into());
                Some(Arc::new(ai::OllamaProvider::new(
                    profile.base_url.clone().unwrap_or_else(|| "http://localhost:11434".into()),
                    profile.model.clone().unwrap_or(default_model),
                )))
            }
            "neboloop" => {
                let metadata: Option<serde_json::Value> = profile
                    .metadata
                    .as_ref()
                    .and_then(|m| serde_json::from_str(m).ok());
                let is_janus = metadata
                    .as_ref()
                    .and_then(|m| m.get("janus_provider"))
                    .and_then(|v| v.as_str())
                    == Some("true");
                if is_janus && !profile.api_key.is_empty() {
                    let janus_url = &state.config.neboloop.janus_url;
                    let model = profile.model.clone().unwrap_or_else(|| "janus".into());
                    let bot_id = config::read_bot_id().unwrap_or_default();
                    let mut p = ai::OpenAIProvider::with_base_url(
                        profile.api_key.clone(),
                        model,
                        format!("{}/v1", janus_url),
                    );
                    p.set_provider_id("janus");
                    if !bot_id.is_empty() {
                        p.set_bot_id(bot_id);
                    }
                    Some(Arc::new(p))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(p) = provider {
            providers.push(p);
        }
    }

    // Add CLI providers from models.yaml config
    for cli_def in &models_cfg.cli_providers {
        if !cli_def.is_active() {
            continue;
        }
        let installed = match cli_def.command.as_str() {
            "claude" => state.cli_statuses.claude.installed,
            "codex" => state.cli_statuses.codex.installed,
            "gemini" => state.cli_statuses.gemini.installed,
            _ => false,
        };
        if !installed {
            continue;
        }
        let p: Arc<dyn ai::Provider> = match cli_def.command.as_str() {
            "claude" => Arc::new(ai::CLIProvider::new_claude_code(0, state.config.port)),
            "codex" => Arc::new(ai::CLIProvider::new_codex_cli()),
            "gemini" => Arc::new(ai::CLIProvider::new_gemini_cli()),
            _ => continue,
        };
        info!(cli = %cli_def.command, "reloaded CLI provider");
        providers.push(p);
    }

    info!(count = providers.len(), "reloading providers");
    state.runner.reload_providers(providers).await;
}

/// GET /api/v1/providers
pub async fn list_providers(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let profiles = state.store.list_auth_profiles().map_err(to_error_response)?;
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
    let provider = body["provider"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("provider required".into())))?;
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
    reload_providers(&state).await;

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
    let model = body["model"].as_str().or(existing.model.as_deref());
    let base_url = body["baseUrl"].as_str().or(existing.base_url.as_deref());
    let priority = body["priority"].as_i64().unwrap_or(existing.priority.unwrap_or(50));
    let auth_type = body["authType"].as_str().or(existing.auth_type.as_deref());
    let metadata = body
        .get("metadata")
        .map(|v| v.to_string())
        .or(existing.metadata.clone());

    state
        .store
        .update_auth_profile(&id, name, api_key, model, base_url, priority, auth_type, metadata.as_deref())
        .map_err(to_error_response)?;

    // Reload providers on the runner
    reload_providers(&state).await;

    let updated = state.store.get_auth_profile(&id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!(updated)))
}

/// DELETE /api/v1/providers/:id
pub async fn delete_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state.store.delete_auth_profile(&id).map_err(to_error_response)?;
    // Reload providers on the runner
    reload_providers(&state).await;
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
            let base_url = profile.base_url.clone().unwrap_or_else(|| "https://api.deepseek.com/v1".into());
            let provider = ai::OpenAIProvider::with_base_url(profile.api_key.clone(), model, base_url);
            test_provider_connection(&provider).await
        }
        "google" => {
            let provider = ai::GeminiProvider::new(profile.api_key.clone(), model);
            test_provider_connection(&provider).await
        }
        "ollama" => {
            let base_url = profile.base_url.clone().unwrap_or_else(|| "http://localhost:11434".into());
            let provider = ai::OllamaProvider::new(base_url, model);
            test_provider_connection(&provider).await
        }
        "neboloop" => {
            let janus_url = &state.config.neboloop.janus_url;
            let bot_id = config::read_bot_id().unwrap_or_default();
            let mut provider = ai::OpenAIProvider::with_base_url(
                profile.api_key.clone(),
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
        messages: vec![ai::Message {
            role: "user".into(),
            content: "Say OK".into(),
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 16,
        temperature: 0.0,
        system: String::new(),
        static_system: String::new(),
        model: String::new(),
        enable_thinking: false,
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

/// GET /api/v1/models — returns model catalog from DB + routing config from YAML.
pub async fn list_models(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    // Read models from the database (source of truth for model availability)
    let all_models = state.store.list_all_provider_models().map_err(to_error_response)?;

    // Group models by provider
    let mut models: std::collections::HashMap<String, Vec<serde_json::Value>> =
        std::collections::HashMap::new();

    for m in &all_models {
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

        let mut info = serde_json::json!({
            "id": m.model_id,
            "displayName": m.display_name,
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

        models
            .entry(m.provider.clone())
            .or_default()
            .push(info);
    }

    // Routing config comes from the YAML catalog (not per-model data)
    let cfg = &state.models_config;

    // Task routing
    let task_routing = cfg.task_routing.as_ref().map(|tr| {
        serde_json::json!({
            "vision": tr.vision,
            "audio": tr.audio,
            "reasoning": tr.reasoning,
            "code": tr.code,
            "general": tr.general,
            "fallbacks": tr.fallbacks,
        })
    });

    // Lane routing
    let lane_routing = cfg.lane_routing.as_ref().map(|lr| {
        let mut m = serde_json::Map::new();
        if !lr.heartbeat.is_empty() {
            m.insert("heartbeat".into(), serde_json::Value::String(lr.heartbeat.clone()));
        }
        if !lr.events.is_empty() {
            m.insert("events".into(), serde_json::Value::String(lr.events.clone()));
        }
        if !lr.comm.is_empty() {
            m.insert("comm".into(), serde_json::Value::String(lr.comm.clone()));
        }
        if !lr.subagent.is_empty() {
            m.insert("subagent".into(), serde_json::Value::String(lr.subagent.clone()));
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
    let active = body.get("active").and_then(|v| v.as_bool()).ok_or_else(|| {
        to_error_response(types::NeboError::Validation("active field is required".into()))
    })?;

    // CLI providers are config, stored in models.yaml
    let mut cfg = config::ModelsConfig::load();
    cfg.set_cli_provider_active(&cli_id, active).map_err(|e| {
        to_error_response(types::NeboError::Validation(e))
    })?;

    // Reload providers so the toggle takes effect immediately
    reload_providers(&state).await;

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

    cfg.save().map_err(|e| {
        to_error_response(types::NeboError::Server(e))
    })?;

    let primary = cfg.defaults.as_ref().map(|d| d.primary.as_str()).unwrap_or("");

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
    let tr = cfg.task_routing.get_or_insert_with(|| config::models::TaskRouting {
        vision: String::new(),
        audio: String::new(),
        reasoning: String::new(),
        code: String::new(),
        general: String::new(),
        fallbacks: std::collections::HashMap::new(),
    });
    if let Some(v) = body.get("vision").and_then(|v| v.as_str()) { tr.vision = v.to_string(); }
    if let Some(v) = body.get("audio").and_then(|v| v.as_str()) { tr.audio = v.to_string(); }
    if let Some(v) = body.get("reasoning").and_then(|v| v.as_str()) { tr.reasoning = v.to_string(); }
    if let Some(v) = body.get("code").and_then(|v| v.as_str()) { tr.code = v.to_string(); }
    if let Some(v) = body.get("general").and_then(|v| v.as_str()) { tr.general = v.to_string(); }
    if let Some(fallbacks) = body.get("fallbacks").and_then(|v| v.as_object()) {
        let mut fb = std::collections::HashMap::new();
        for (k, v) in fallbacks {
            if let Some(arr) = v.as_array() {
                fb.insert(k.clone(), arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());
            }
        }
        tr.fallbacks = fb;
    }

    // Update lane routing
    if let Some(lr_val) = body.get("laneRouting").and_then(|v| v.as_object()) {
        let lr = cfg.lane_routing.get_or_insert_with(|| config::models::LaneRouting {
            heartbeat: String::new(),
            events: String::new(),
            comm: String::new(),
            subagent: String::new(),
        });
        if let Some(v) = lr_val.get("heartbeat").and_then(|v| v.as_str()) { lr.heartbeat = v.to_string(); }
        if let Some(v) = lr_val.get("events").and_then(|v| v.as_str()) { lr.events = v.to_string(); }
        if let Some(v) = lr_val.get("comm").and_then(|v| v.as_str()) { lr.comm = v.to_string(); }
        if let Some(v) = lr_val.get("subagent").and_then(|v| v.as_str()) { lr.subagent = v.to_string(); }
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

    cfg.save().map_err(|e| {
        to_error_response(types::NeboError::Server(e))
    })?;

    Ok(Json(serde_json::json!({
        "message": "Task routing updated successfully",
    })))
}

/// GET /api/v1/local-models/status
pub async fn local_models_status() -> HandlerResult<serde_json::Value> {
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

    Ok(Json(serde_json::json!({
        "available": true,
        "models": model_names,
    })))
}
