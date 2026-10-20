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
    crate::inject_connection_models(store, harness.selector());
}

/// GET /api/v1/providers
///
/// Beside the profiles, how each is reached (`routes`, by profile id), the
/// way a turn reaches it: `janus` (a key for a provider Nebo knows by name,
/// through Janus, which needs a NeboAI sign-in: `signInRequired` when there
/// is none), `direct` (a URL the owner entered, Ollama, a key-less
/// connection) or `unsupported` (a URL the owner entered for a provider Nebo
/// has no direct client for: neither listed nor used,
/// `auth::credential::direct_url`). The page shows a connection chat would refuse as one, never
/// as healthy.
pub async fn list_providers(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let profiles = state
        .store
        .list_auth_profiles()
        .map_err(to_error_response)?;
    let signed_in = !auth::neboai_token(&state.store).unwrap_or_default().is_empty();
    let routes: serde_json::Map<String, serde_json::Value> = profiles
        .iter()
        .map(|p| {
            let route = match (auth::credential::own_key(p), auth::credential::direct_url(p)) {
                (Some(_), _) => serde_json::json!({ "via": "janus", "signInRequired": !signed_in }),
                (None, Some(Err(_))) => serde_json::json!({ "via": "unsupported", "signInRequired": false }),
                (None, _) => serde_json::json!({ "via": "direct", "signInRequired": false }),
            };
            (p.id.clone(), route)
        })
        .collect();
    Ok(Json(serde_json::json!({ "profiles": profiles, "routes": routes })))
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
    let api_key = auth::credential::encrypt(body["apiKey"].as_str().unwrap_or(""))
        .map_err(|e| to_error_response(types::NeboError::Internal(e)))?;
    let model = body["model"].as_str();
    let base_url = body["baseUrl"].as_str();
    // A compatible URL is the whole point of those two kinds.
    if matches!(provider, "openai_compatible" | "systemone_compatible")
        && base_url.map(str::trim).is_none_or(str::is_empty)
    {
        return Err(to_error_response(types::NeboError::Validation("This provider needs its base URL.".into())));
    }
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
            &api_key,
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
    let api_key = auth::credential::encrypt(body["apiKey"].as_str().unwrap_or(&existing.api_key))
        .map_err(|e| to_error_response(types::NeboError::Internal(e)))?;
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
            &api_key,
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

/// What the provider test says of a connection at a URL of the owner's that
/// Nebo has no direct client for (`auth::credential::direct_url`); the app
/// shows its own words for it (`routes`' `unsupported`).
const OWNER_URL_UNSUPPORTED: &str = "Your own URLs for this provider aren't supported yet. Remove the URL to use the key through NeboAI.";

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
    let api_key = auth::credential::profile_key(&profile);
    if api_key.is_empty() && profile.auth_type.as_deref() != Some("local") && profile.provider != "openai_compatible" {
        return Ok(Json(serde_json::json!({
            "success": false,
            "provider": profile.provider,
            "message": "Missing API key",
        })));
    }

    // Build a temporary provider and try a minimal API call.
    let model = test_model(&state.store, &profile);
    // A compatible server with no model yet: its own model list, read with
    // the key, is the check.
    if model.is_empty() && profile.provider == "openai_compatible" {
        let (success, message) = match super::connections::fetch_catalog(&profile).await {
            Ok(Some(list)) => (true, format!("Connection successful — the server lists {} models", list.len())),
            Ok(None) | Err(_) => (false, "The server didn't answer with its models. Check the base URL and the key.".to_string()),
        };
        return Ok(Json(serde_json::json!({ "success": success, "provider": profile.provider, "message": message })));
    }
    let test_result: Result<String, String> = match profile.provider.as_str() {
        // The same client a turn uses: a built-in key through Janus, a URL
        // the owner entered direct.
        kind @ ("anthropic" | "openai" | "deepseek" | "google" | "openrouter" | "xai" | "openai_compatible") => {
            match auth::credential::own_key(&profile) {
                Some(own) => {
                    let provider = crate::janus_chat_provider(&state.store, &state.config, kind, model, Some(own));
                    test_provider_connection(&provider).await
                }
                None => match auth::credential::direct_url(&profile) {
                    Some(Ok(url)) => {
                        let mut provider = ai::OpenAIProvider::with_base_url(api_key.clone(), model, url);
                        provider.set_provider_id(kind);
                        test_provider_connection(&provider).await
                    }
                    Some(Err(_)) => Err(OWNER_URL_UNSUPPORTED.to_string()),
                    None if kind == "openai_compatible" => Err("This provider needs its base URL.".to_string()),
                    None => Err("This connection has no key. Add the provider's API key.".to_string()),
                },
            }
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
            let provider = crate::janus_chat_provider(&state.store, &state.config, "janus", model, None);
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

/// The model a provider test asks: the one the connection pins, else the
/// first chat model it has on. Empty when it has none.
fn test_model(store: &db::Store, profile: &db::models::AuthProfile) -> String {
    profile.model.clone().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| {
        super::connections::connection_models(store, profile)
            .into_iter()
            .find(|m| m.is_active && m.kind == "chat")
            .map(|m| m.model_id)
            .unwrap_or_default()
    })
}

/// Test a provider by sending a minimal chat request.
async fn test_provider_connection(provider: &dyn ai::Provider) -> Result<String, String> {
    let req = ai::ChatRequest {
        tool_credential: None,
        chat_id: String::new(),
        ask_channels: None,
        permission_mode: None,
        linked_context: None,
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
        effort: None,
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
            "rank": m.rank,
            // A model Janus's catalog lists (not nebo-1 or a speed): pickers
            // of speeds and routes leave it out.
            "catalog": super::connections::is_catalog_row(m),
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

    // The bot's routes
    let task_routing = cfg.task_routing.as_ref().map(|tr| {
        serde_json::json!({
            "general": tr.general,
        })
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
                "active": cp.is_active(),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "models": models,
        "taskRouting": task_routing,
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

    if let Some(primary) = body.get("primary").and_then(|v| v.as_str()).filter(|p| !p.is_empty()) {
        cfg.defaults.get_or_insert_with(|| config::models::Defaults { primary: String::new() }).primary =
            primary.to_string();
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
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let mut cfg = config::ModelsConfig::load();

    // Update task routing
    let tr = cfg
        .task_routing
        .get_or_insert_with(Default::default);
    if let Some(v) = body.get("general").and_then(|v| v.as_str()) {
        tr.general = v.to_string();
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
    // The running bot takes the new routes now: the next turn runs on the
    // new default (a model or an intelligence pack), with no restart.
    state.harness.selector().set_task_routing(agent::selector::task_routes(&cfg));

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
            model_kind: "chat".into(),
            source: "catalog".into(),
            rank: None,
            card: None,
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
        // The profile's token is stored encrypted, as at startup. This test
        // used to pass only when another test in the same process had set
        // the encryptor first.
        auth::credential::init(mcp::crypto::Encryptor::generate());
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

// ── Intelligence packs (`types::packs`) ─────────────────────────────

/// What the app sends to make or change a pack.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackBody {
    pub name: String,
    #[serde(default)]
    pub levels: types::packs::PackLevels,
    #[serde(default = "fallback_on")]
    pub fallback: bool,
    #[serde(default)]
    pub lanes: types::packs::PackLanes,
    #[serde(default)]
    pub level_effort: types::packs::LevelEffort,
}

fn fallback_on() -> bool {
    true
}

/// The pack a body describes, or why it can't be one: a name, and every
/// level a `provider/model` (never another pack).
fn pack_of(id: String, body: PackBody) -> Result<types::packs::Pack, types::NeboError> {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(types::NeboError::Validation("A pack needs a name.".into()));
    }
    let l = &body.levels;
    for model in [&l.instant, &l.low, &l.medium, &l.high, &l.max, &l.vision, &l.voice, &l.decisions].into_iter().flatten() {
        let model = model.trim();
        let well_formed = model.split_once('/').is_some_and(|(p, m)| !p.is_empty() && !m.is_empty());
        if !model.is_empty() && (!well_formed || types::packs::parse_ref(model).is_some()) {
            return Err(types::NeboError::Validation(format!("{model} isn't a model (provider/model).")));
        }
    }
    // Auto is Janus's alone: a pack the owner makes runs at the level he set.
    let levels = types::packs::PackLevels { auto: None, ..body.levels };
    Ok(types::packs::Pack {
        id,
        name,
        levels,
        fallback: body.fallback,
        built_in: false,
        lanes: body.lanes,
        level_effort: body.level_effort,
    })
}

/// GET /api/v1/intelligence-packs — every pack, Nebo AI first.
pub async fn list_packs(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let packs = state.store.list_intelligence_packs().map_err(to_error_response)?;
    let default = state.harness.selector().configured();
    Ok(Json(serde_json::json!({ "packs": packs, "default": default })))
}

#[derive(serde::Deserialize)]
pub struct DefaultBody {
    pub value: String,
}

/// PUT /api/v1/intelligence-packs/default — the bot's default (a pack ref
/// `pack/<id>[/<level>]` or a model). Applies to the next turn.
pub async fn set_default_pack(
    State(state): State<AppState>,
    Json(body): Json<DefaultBody>,
) -> HandlerResult<serde_json::Value> {
    let value = body.value.trim().to_string();
    let known_pack = |id: &str| state.store.get_intelligence_pack(id).ok().flatten().is_some();
    let valid = match types::packs::parse_ref(&value) {
        Some((id, _)) => known_pack(id),
        None => value.split_once('/').is_some_and(|(p, m)| !p.is_empty() && !m.is_empty()),
    };
    if !valid {
        return Err(to_error_response(types::NeboError::Validation(format!("{value} isn't a pack or a model."))));
    }
    let mut cfg = config::ModelsConfig::load();
    cfg.task_routing
        .get_or_insert_with(Default::default)
        .general = value.clone();
    cfg.save().map_err(|e| to_error_response(types::NeboError::Server(e)))?;
    state.harness.selector().set_task_routing(agent::selector::task_routes(&cfg));
    Ok(Json(serde_json::json!({ "default": value })))
}

/// The label a provider kind reads as.
fn kind_label(kind: &str) -> &str {
    match kind {
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        "google" => "Google",
        "openrouter" => "OpenRouter",
        "deepseek" => "DeepSeek",
        "xai" => "xAI",
        "openai_compatible" => "OpenAI-compatible",
        "systemone_compatible" => "SystemOne-compatible",
        other => other,
    }
}

/// GET /api/v1/intelligence-packs/choices — every active model a pack slot
/// may name, grouped by kind: Nebo AI's speeds, each connection's models,
/// Ollama and the agents on this computer (`local`). Decisions always offers
/// Nebo AI's Jev first. Voice offers only what calls speak through today,
/// xAI's realtime protocol (`Harness::call_voice`): Nebo AI's Voice first,
/// then each xAI connection's realtime models.
pub async fn list_pack_choices(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let (mut chat, decision, voice) = store_choices(&state.store);
    let catalog = config::ModelsConfig::load();
    let clis = config::detect_all_clis();
    for cp in catalog.cli_providers.iter().filter(|c| c.is_active()) {
        let installed = match cp.command.as_str() {
            "claude" => clis.claude.installed,
            "codex" => clis.codex.installed,
            "gemini" => clis.gemini.installed,
            _ => false,
        };
        // The tool runs its own default model: Nebo names none.
        if installed {
            chat.push(serde_json::json!({ "value": format!("{}/", cp.id), "provider": cp.display_name, "connection": "", "modelId": "", "capabilities": [], "local": true }));
        }
    }
    Ok(Json(serde_json::json!({ "chat": chat, "decision": decision, "voice": voice })))
}

/// The pack choices the store holds (chat, decision, voice): Nebo AI's
/// speeds and the models it serves, each with its card; each connection's
/// models with theirs; Ollama's. The agents on this computer are the
/// handler's to add.
pub(crate) fn store_choices(store: &db::Store) -> (Vec<serde_json::Value>, Vec<serde_json::Value>, Vec<serde_json::Value>) {
    use types::packs::{NEBO_AI_VOICE, XAI_VOICE_MODEL, is_xai_connection, is_xai_voice_model};
    let choice = |value: String, provider: &str, connection: &str, model: &str, caps: &[String], local: bool| {
        serde_json::json!({ "value": value, "provider": provider, "connection": connection, "modelId": model, "capabilities": caps, "local": local })
    };
    let mut chat = Vec::new();
    let mut decision = vec![choice("janus/jev-latest".into(), "Nebo AI", "", "jev-latest", &[], false)];
    let nebo_voice = NEBO_AI_VOICE.split_once('/').map_or(NEBO_AI_VOICE, |(_, m)| m);
    let mut voice = vec![choice(NEBO_AI_VOICE.into(), "Nebo AI", "", nebo_voice, &[], false)];
    for m in store.list_active_provider_models("janus").unwrap_or_default() {
        let caps: Vec<String> = m.capabilities.as_deref().and_then(|c| serde_json::from_str(c).ok()).unwrap_or_default();
        if agent::selector::is_chat_model(&m.model_id, &caps, &[]) {
            // A Nebo AI speed reads by the levels it serves ("High and Max"),
            // never by its internal id. A model Nebo AI serves carries its
            // card (abilities, what it is good for, its retail price), and
            // its abilities decide the slots it may fill (Vision).
            let card = super::connections::card_of_row(&m);
            let abilities = card.as_ref().map(|c| c.abilities.list()).unwrap_or_default();
            let mut c = choice(format!("janus/{}", m.model_id), "Nebo AI", "", &m.model_id, &abilities, false);
            if !m.display_name.is_empty() && m.display_name != m.model_id {
                c["name"] = serde_json::Value::String(m.display_name.clone());
            }
            if let Some(card) = card {
                c["card"] = serde_json::to_value(card).unwrap_or_default();
            }
            if super::connections::is_catalog_row(&m) {
                c["catalog"] = serde_json::Value::Bool(true);
            }
            chat.push(c);
        }
    }
    for p in store.list_auth_profiles().unwrap_or_default() {
        // A URL of the owner's Nebo has no direct client for is never a
        // choice: chat could not run it (`auth::credential::direct_url`).
        if p.is_active.unwrap_or(0) == 0
            || matches!(p.provider.as_str(), "neboai" | "ollama")
            || matches!(auth::credential::direct_url(&p), Some(Err(_)))
        {
            continue;
        }
        let key = super::connections::connection_key(&p);
        let models: Vec<_> = super::connections::connection_models(store, &p).into_iter().filter(|m| m.is_active).collect();
        if is_xai_connection(&p.provider) {
            let mut names = vec![XAI_VOICE_MODEL];
            for m in models.iter().map(|m| m.model_id.as_str()).filter(|m| is_xai_voice_model(m)) {
                if !names.contains(&m) {
                    names.push(m);
                }
            }
            for m in names {
                voice.push(choice(format!("{key}/{m}"), kind_label(&p.provider), &p.name, m, &[], false));
            }
        }
        let via_janus = auth::credential::own_key(&p).is_some();
        for m in models {
            let m = super::connections::carded(store, m, via_janus);
            let mut c = choice(format!("{key}/{}", m.model_id), kind_label(&p.provider), &p.name, &m.model_id, &m.capabilities, false);
            c["card"] = serde_json::to_value(&m.card).unwrap_or_default();
            if m.kind == "decision" { decision.push(c) } else { chat.push(c) }
        }
    }
    for m in store.list_active_provider_models("ollama").unwrap_or_default() {
        chat.push(choice(format!("ollama/{}", m.model_id), "Ollama", "", &m.model_id, &[], true));
    }
    (chat, decision, voice)
}

/// POST /api/v1/intelligence-packs — make a pack.
pub async fn create_pack(
    State(state): State<AppState>,
    Json(body): Json<PackBody>,
) -> HandlerResult<serde_json::Value> {
    let pack = pack_of(uuid::Uuid::new_v4().to_string(), body).map_err(to_error_response)?;
    state.store.save_intelligence_pack(&pack).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "pack": pack })))
}

/// PUT /api/v1/intelligence-packs/{id} — change one of the owner's packs.
/// Nebo AI is built in: refused.
pub async fn update_pack(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PackBody>,
) -> HandlerResult<serde_json::Value> {
    if state.store.get_intelligence_pack(&id).map_err(to_error_response)?.is_none() {
        return Err(to_error_response(types::NeboError::NotFound));
    }
    let pack = pack_of(id, body).map_err(to_error_response)?;
    state.store.save_intelligence_pack(&pack).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "pack": pack })))
}

/// DELETE /api/v1/intelligence-packs/{id} — remove one of the owner's packs.
/// An employee still on it runs on Nebo AI, and is told so.
pub async fn delete_pack(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    if !state.store.delete_intelligence_pack(&id).map_err(to_error_response)? {
        return Err(to_error_response(types::NeboError::NotFound));
    }
    Ok(Json(serde_json::json!({ "deleted": id })))
}

#[cfg(test)]
mod test_model_tests {
    /// A connection added with no model pinned is tested on a chat model it
    /// has on, never on an empty model id.
    #[test]
    fn a_test_asks_the_pinned_model_else_a_chat_model_the_connection_has_on() {
        let store = db::Store::new(":memory:").unwrap();
        store.create_auth_profile("x", "xAI", "openai_compatible", "", None, Some("https://api.x.ai/v1"), 0, 1, None, None).unwrap();
        let p = store.get_auth_profile("x").unwrap().unwrap();
        assert_eq!(super::test_model(&store, &p), "", "nothing to ask yet");
        let key = super::super::connections::connection_key(&p);
        store.put_connection_model(&key, "jev", "Jev", "decision", None, "[]", "added", true).unwrap();
        store.put_connection_model(&key, "grok-off", "Grok off", "chat", None, "[]", "added", false).unwrap();
        store.put_connection_model(&key, "grok-5", "Grok 5", "chat", None, "[]", "added", true).unwrap();
        assert_eq!(super::test_model(&store, &p), "grok-5");
        store.create_auth_profile("y", "Pinned", "openai_compatible", "", Some("pinned-1"), Some("https://x/v1"), 0, 1, None, None).unwrap();
        let pinned = store.get_auth_profile("y").unwrap().unwrap();
        assert_eq!(super::test_model(&store, &pinned), "pinned-1");
    }
}

#[cfg(test)]
mod pack_tests {
    use super::{PackBody, pack_of};
    use types::packs::PackLevels;

    fn body(name: &str, medium: &str) -> PackBody {
        PackBody {
            name: name.into(),
            levels: PackLevels { medium: Some(medium.into()), auto: Some("janus/nebo-1".into()), ..Default::default() },
            fallback: true,
            lanes: Default::default(),
            level_effort: Default::default(),
        }
    }

    #[test]
    fn a_pack_needs_a_name_and_real_models_and_never_gets_auto() {
        let pack = pack_of("p".into(), body(" Mine ", "anthropic/claude-x")).unwrap();
        assert_eq!(pack.name, "Mine");
        assert_eq!(pack.levels.auto, None, "Auto is Janus's alone");
        assert!(!pack.built_in);
        assert!(pack_of("p".into(), body("  ", "anthropic/claude-x")).is_err());
        assert!(pack_of("p".into(), body("Mine", "claude-x")).is_err(), "provider/model");
        assert!(pack_of("p".into(), body("Mine", "pack/other")).is_err(), "never another pack");
        assert!(pack_of("p".into(), body("Mine", "")).is_ok(), "an empty level is empty");
    }
}

/// A user's own provider key is taken from the owner's apps (the phone
/// through the tunnel included) and goes from this computer to its provider:
/// never back out of the API.
#[cfg(test)]
mod key_egress_tests {
    /// A sentinel no real provider issues; any appearance is a leak.
    const SENTINEL: &str = "sk-SENTINEL-byo-key-0000000000000000";

    /// `GET /api/v1/providers` answers the phone over the tunnel too: the
    /// NeboAI account's refresh token (and its access token) never go with it.
    #[test]
    fn no_profile_response_carries_the_neboai_refresh_token() {
        const REFRESH: &str = "rt-SENTINEL-neboai-refresh-0000";
        auth::credential::init(mcp::crypto::Encryptor::generate());
        let store = db::Store::new(":memory:").unwrap();
        super::super::neboai::store_neboai_profile(
            &store, "https://api.example.com", "owner-1", "owner@example.com", "Owner", SENTINEL, REFRESH, true,
        )
        .unwrap();
        let list = serde_json::json!({"profiles": store.list_auth_profiles().unwrap()}).to_string();
        assert!(!list.contains(REFRESH), "the refresh token reached an API response: {list}");
        assert!(!list.contains(SENTINEL), "the access token reached an API response: {list}");
        assert!(list.contains("janus_provider"), "what the app reads still goes: {list}");
    }

    /// Every response that carries a profile (list, get, create, update) is
    /// this struct serialized: the key is left out, not masked.
    #[test]
    fn no_profile_response_carries_the_key() {
        let store = db::Store::new(":memory:").unwrap();
        for provider in ["anthropic", "openai", "google", "xai", "openrouter", "deepseek", "search-serpapi"] {
            store
                .create_auth_profile(&format!("p-{provider}"), provider, provider, SENTINEL, None, None, 0, 1, Some("api_key"), Some("{}"))
                .unwrap();
        }
        let list = serde_json::json!({"profiles": store.list_auth_profiles().unwrap()}).to_string();
        let one = serde_json::to_string(&store.get_auth_profile("p-xai").unwrap()).unwrap();
        for out in [list, one] {
            assert!(!out.contains(SENTINEL), "a provider key reached an API response: {out}");
            assert!(!out.contains("apiKey"), "an apiKey field reached an API response: {out}");
        }
    }
}

/// Provider keys are stored encrypted and opened only where a client that
/// presents them is built: the provider still sends the key the owner typed —
/// through Janus for a provider Nebo knows by name, direct to a URL he
/// entered.
#[cfg(test)]
mod key_at_rest_tests {
    use std::sync::Arc;

    const SENTINEL: &str = "sk-SENTINEL-at-rest-provider-0000";

    /// A loopback stub that answers every request 401 and keeps each one.
    async fn stub() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 16384];
                let mut got = Vec::new();
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = sock.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                }
                log.lock().unwrap().push(String::from_utf8_lossy(&got).to_string());
                let _ = sock.write_all(b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
            }
        });
        (base, seen)
    }

    /// One header's value in a request head, by its lowercase name.
    fn header(request: &str, name: &str) -> Option<String> {
        request.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim().eq_ignore_ascii_case(name)).then(|| v.trim().to_string())
        })
    }

    #[tokio::test]
    async fn a_built_in_key_rides_to_janus_and_an_owner_url_gets_it_direct() {
        auth::credential::init(mcp::crypto::Encryptor::generate());
        let (janus, janus_seen) = stub().await;
        let (own, own_seen) = stub().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("nebo.db").to_string_lossy()).unwrap());
        let sealed = auth::credential::encrypt(SENTINEL).unwrap();
        store.create_auth_profile("nebo", "NeboAI", "neboai", "nebo-token", None, None, 0, 1, Some("token"), None).unwrap();
        for kind in ["anthropic", "openai", "google", "deepseek", "openrouter", "xai"] {
            store.create_auth_profile(kind, "Mine", kind, &sealed, Some("m-1"), None, 0, 1, None, None).unwrap();
        }
        // URLs the owner entered, public-looking or not, are his own
        // endpoints (the stub stands in for each).
        for (id, kind) in [("groq", "openai_compatible"), ("typed-xai", "xai"), ("lab", "openai_compatible")] {
            store.create_auth_profile(id, id, kind, &sealed, Some("m-2"), Some(&format!("{own}/v1")), 0, 1, None, None).unwrap();
        }
        assert!(!store.get_auth_profile("anthropic").unwrap().unwrap().api_key.contains(SENTINEL));

        let mut cfg = config::Config::default();
        cfg.neboai.janus_url = janus.clone();
        let providers = crate::build_providers(&store, &cfg, None, None);
        for id in [
            "anthropic@anthropic", "openai@openai", "google@google", "deepseek@deepseek", "openrouter@openrouter", "xai@xai",
            "openai_compatible@groq", "xai@typed-xai", "openai_compatible@lab",
        ] {
            let p = providers.iter().find(|p| p.id() == id).unwrap_or_else(|| panic!("{id} is built"));
            let _ = super::test_provider_connection(p.as_ref()).await;
        }

        let to_janus = janus_seen.lock().unwrap().clone();
        let mut kinds = Vec::new();
        for request in &to_janus {
            assert!(request.starts_with("POST /v1/chat/completions "), "not Janus's chat door");
            assert_eq!(header(request, "x-provider-api-key").as_deref(), Some(SENTINEL), "the key the owner typed rides along");
            assert!(header(request, "authorization").is_some_and(|a| !a.contains(SENTINEL)), "his key is never the Janus bearer");
            assert!(header(request, "x-provider-base-url").is_none(), "no URL of his is ever sent to Janus");
            kinds.push(header(request, "x-provider").unwrap());
        }
        kinds.sort();
        assert_eq!(kinds, ["anthropic", "deepseek", "google", "openai", "openrouter", "xai"], "every built-in key, and only those");

        let direct = own_seen.lock().unwrap().clone();
        assert_eq!(direct.len(), 3, "every URL he entered is reached direct");
        for request in &direct {
            assert_eq!(header(request, "authorization").as_deref(), Some(format!("Bearer {SENTINEL}").as_str()));
            assert!(header(request, "x-provider").is_none() && header(request, "x-provider-api-key").is_none());
        }
    }

    /// A URL the owner entered for Anthropic or Google has no direct client:
    /// chat never builds it, the packs never offer it, and the listing never
    /// asks it, so nothing shows a connection chat cannot run.
    #[test]
    fn an_owner_url_with_no_direct_client_is_neither_used_nor_offered() {
        auth::credential::init(mcp::crypto::Encryptor::generate());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("nebo.db").to_string_lossy()).unwrap());
        let sealed = auth::credential::encrypt(SENTINEL).unwrap();
        for kind in ["anthropic", "google"] {
            store.create_auth_profile(kind, "Gateway", kind, &sealed, Some("m-1"), Some("https://gateway.example/v1"), 0, 1, None, None).unwrap();
            let p = store.get_auth_profile(kind).unwrap().unwrap();
            let key = super::super::connections::connection_key(&p);
            store
                .create_provider_model(&format!("{key}-m1"), &key, "m-1", "m-1", 1, 0, None, None, None, Some(r#"["tools"]"#), Some(r#"["chat"]"#), 0, None)
                .unwrap();
            assert!(super::super::connections::catalog_request(&p).is_none(), "{kind}: not listed");
        }
        let providers = crate::build_providers(&store, &config::Config::default(), None, None);
        for id in ["anthropic@anthropic", "google@google"] {
            assert!(!providers.iter().any(|p| p.id() == id), "{id}: chat has no client for it");
        }
        let (chat, _, _) = super::store_choices(&store);
        assert!(
            !chat.iter().any(|c| c["connection"] == "Gateway"),
            "never offered to a pack: {chat:?}"
        );
    }

}
