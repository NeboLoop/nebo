pub mod a2ui;
pub mod a2ui_actions;
pub mod app_lifecycle;
mod artifact_updates;
mod channel_dispatch;
pub mod chat_dispatch;
pub mod codes;
pub mod deps;
pub mod entity_config;
pub mod handlers;
mod heartbeat;
pub mod middleware;
mod migration;
mod plugin_commands;
mod plugin_provider;
mod redact;
pub mod routes;
pub mod run_registry;
mod scheduler;
mod spa;
mod state;
pub mod workflow_manager;

/// Truncate a string to at most `max_bytes` bytes without splitting a multi-byte
/// UTF-8 character.
pub(crate) fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

use std::net::TcpListener;
use std::sync::Arc;

use axum::Router;
use axum::http::Method;
use axum::response::Json;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

use config::Config;
use handlers::ws::ClientHub;
use middleware::JwtSecret;
use state::AppState;
use types::NeboError;
use types::api::HealthResponse;

pub use state::AppState as ServerState;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Matches a web-composer mention chip `<@id>` where id is a bot_id or a loop
/// agent UUID. Captures the inner id. Used by the loop channel branch to route
/// a mention to the specific exposed agent it addresses.
static MENTION_TOKEN_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"<@([0-9a-fA-F._-]+)>").unwrap());

/// Conservative detection of an explicit "produce one joint result together"
/// ask. Used by the loop channel branch to choose coordination mode (one lead
/// synthesizes) over independent fan-out (each agent replies for itself).
/// Deliberately tight — independent replies are the default; only clear
/// collaboration phrasing routes to a single lead.
fn wants_coordination(text: &str) -> bool {
    let t = text.to_lowercase();
    const PHRASES: &[&str] = &[
        "work together",
        "working together",
        "work as a team",
        "collaborate",
        "collaboration",
        "as a team",
        "as a group",
        "team up",
        "join forces",
        "jointly",
        "one combined",
        "single combined",
        "combined answer",
        "joint plan",
        "one plan together",
    ];
    PHRASES.iter().any(|p| t.contains(p))
}

/// Best-effort claim extraction from a JWT payload — no signature verification.
/// Only used on the provisioner-injected NEBO_BOT_TOKEN to mirror its ownerId
/// into profile metadata the same way the OAuth pathway records owner_id; the
/// token itself is verified by the services that consume it.
fn jwt_claim(token: &str, claim: &str) -> Option<String> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get(claim)?.as_str().map(|s| s.to_string())
}

/// Seed the provider_models table from the embedded models.yaml catalog.
/// - New models are inserted with is_active=1
/// - Existing models get metadata updated (pricing, capabilities, context_window)
/// - Models in DB but NOT in the current catalog get marked is_active=0
/// User's is_active, is_default, and preferred choices are preserved for existing models.
fn seed_models_from_catalog(store: &db::Store, models_cfg: &config::ModelsConfig) {
    let version = VERSION;

    for (provider_name, models) in &models_cfg.providers {
        let mut seeded_model_ids: Vec<String> = Vec::new();

        for model in models {
            let id = format!("{}/{}", provider_name, model.id);
            let capabilities = if model.capabilities.is_empty() {
                None
            } else {
                serde_json::to_string(&model.capabilities).ok()
            };
            let kind = if model.kind.is_empty() {
                None
            } else {
                serde_json::to_string(&model.kind).ok()
            };
            let (input_price, output_price) = match &model.pricing {
                Some(p) => (Some(p.input), Some(p.output)),
                None => (None, None),
            };
            let context_window = if model.context_window > 0 {
                Some(model.context_window)
            } else {
                None
            };

            if let Err(e) = store.upsert_provider_model(
                &id,
                provider_name,
                &model.id,
                &model.display_name,
                context_window,
                input_price,
                output_price,
                capabilities.as_deref(),
                kind.as_deref(),
                Some(version),
                model.is_active(),
            ) {
                warn!(
                    provider = %provider_name,
                    model = %model.id,
                    error = %e,
                    "failed to seed model"
                );
            }

            seeded_model_ids.push(model.id.clone());
        }

        // Mark models that are no longer in the catalog as inactive
        if let Err(e) = store.deactivate_stale_models(provider_name, &seeded_model_ids) {
            warn!(
                provider = %provider_name,
                error = %e,
                "failed to deactivate stale models"
            );
        }
    }
}

/// Inject Ollama models from DB into the selector's runtime models.
/// Ollama models are auto-discovered and stored in the DB, not in models.yaml,
/// so the selector needs them injected separately.
pub fn inject_ollama_models(store: &db::Store, selector: &agent::ModelSelector) {
    if let Ok(ollama_models) = store.list_active_provider_models("ollama") {
        if !ollama_models.is_empty() {
            let infos: Vec<agent::selector::ModelInfo> = ollama_models
                .iter()
                .map(|m| agent::selector::ModelInfo {
                    id: m.model_id.clone(),
                    display_name: m.display_name.clone(),
                    context_window: m.context_window.unwrap_or(128_000) as i32,
                    input_price: 0.0,
                    output_price: 0.0,
                    capabilities: m
                        .capabilities
                        .as_ref()
                        .and_then(|c| serde_json::from_str(c).ok())
                        .unwrap_or_default(),
                    kind: m
                        .kind
                        .as_ref()
                        .and_then(|k| serde_json::from_str(k).ok())
                        .unwrap_or_default(),
                    preferred: false,
                    active: true,
                })
                .collect();
            selector.inject_provider_models("ollama", infos);
        }
    }
}

/// Build a map of "provider/model_id" → is_active from the DB provider_models table.
/// Used to override the yaml catalog defaults so the selector respects user toggles.
pub fn build_model_overrides(store: &db::Store) -> std::collections::HashMap<String, bool> {
    let mut overrides = std::collections::HashMap::new();
    if let Ok(all_models) = store.list_all_provider_models() {
        for m in &all_models {
            let key = format!("{}/{}", m.provider, m.model_id);
            overrides.insert(key, m.is_active.unwrap_or(0) == 1);
        }
    }
    overrides
}

/// Build an embedding provider from auth profiles.
/// Prefers OpenAI (text-embedding-3-small), falls back to Ollama if available.
fn build_embedding_provider(
    store: &Arc<db::Store>,
    cfg: &Config,
) -> Option<Arc<dyn ai::EmbeddingProvider>> {
    let profiles = store.list_auth_profiles().ok()?;
    for profile in &profiles {
        if profile.is_active.unwrap_or(0) == 0 {
            continue;
        }
        match profile.provider.as_str() {
            "openai" => {
                let ep = ai::OpenAIEmbeddingProvider::new(profile.api_key.clone());
                let cached = ai::CachedEmbeddingProvider::new(Box::new(ep), store.clone());
                info!("embedding provider: OpenAI text-embedding-3-small (cached)");
                return Some(Arc::new(cached));
            }
            "ollama" => {
                let base_url = profile
                    .base_url
                    .clone()
                    .unwrap_or_else(|| "http://localhost:11434".into());
                let ep = ai::OllamaEmbeddingProvider::new(base_url, "nomic-embed-text".into(), 768);
                let cached = ai::CachedEmbeddingProvider::new(Box::new(ep), store.clone());
                info!("embedding provider: Ollama nomic-embed-text (cached)");
                return Some(Arc::new(cached));
            }
            "neboai" => {
                let metadata: Option<serde_json::Value> = profile
                    .metadata
                    .as_ref()
                    .and_then(|m| serde_json::from_str(m).ok());
                let is_janus = metadata
                    .as_ref()
                    .and_then(|m| m.get("janus_provider"))
                    .and_then(|v| v.as_str())
                    == Some("true");
                if is_janus {
                    let janus_url = &cfg.neboai.janus_url;
                    let bot_id = config::read_bot_id().unwrap_or_default();
                    let api_key = if profile.api_key.is_empty() {
                        bot_id.clone()
                    } else {
                        profile.api_key.clone()
                    };
                    let ep = ai::OpenAIEmbeddingProvider::with_base_url(
                        api_key,
                        format!("{}/v1", janus_url),
                    )
                    .with_model("neboloop/nebo-embed-small".into(), 1536)
                    .with_headers(vec![("X-Bot-ID".into(), bot_id)]);
                    let cached = ai::CachedEmbeddingProvider::new(Box::new(ep), store.clone());
                    info!("embedding provider: Janus neboloop/nebo-embed-small (cached)");
                    return Some(Arc::new(cached));
                }
            }
            _ => {}
        }
    }
    None
}

/// Build AI providers from auth_profiles in the database.
/// Config is needed for NeboAI's Janus URL (not stored in auth_profile).
pub fn build_providers(
    store: &db::Store,
    cfg: &Config,
    cli_statuses: Option<&config::AllCliStatuses>,
) -> Vec<Arc<dyn ai::Provider>> {
    let profiles = match store.list_auth_profiles() {
        Ok(p) => p,
        Err(e) => {
            warn!("failed to load auth profiles: {}", e);
            return Vec::new();
        }
    };

    let models_cfg = config::ModelsConfig::load();

    let mut providers: Vec<Arc<dyn ai::Provider>> = Vec::new();
    let mut gateway_providers: Vec<Arc<dyn ai::Provider>> = Vec::new();
    for profile in &profiles {
        if profile.is_active.unwrap_or(0) == 0 {
            continue;
        }
        let provider: Option<Arc<dyn ai::Provider>> = match profile.provider.as_str() {
            "anthropic" => {
                let default_model = models_cfg
                    .default_model_for_provider("anthropic")
                    .unwrap_or_default();
                Some(Arc::new(ai::AnthropicProvider::new(
                    profile.api_key.clone(),
                    profile.model.clone().unwrap_or(default_model),
                )))
            }
            "openai" => {
                let default_model = models_cfg
                    .default_model_for_provider("openai")
                    .unwrap_or_default();
                Some(Arc::new(ai::OpenAIProvider::new(
                    profile.api_key.clone(),
                    profile.model.clone().unwrap_or(default_model),
                )))
            }
            "deepseek" => {
                let default_model = models_cfg
                    .default_model_for_provider("deepseek")
                    .unwrap_or_default();
                let mut p = ai::OpenAIProvider::with_base_url(
                    profile.api_key.clone(),
                    profile.model.clone().unwrap_or(default_model),
                    profile
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://api.deepseek.com/v1".into()),
                );
                p.set_provider_id("deepseek");
                Some(Arc::new(p))
            }
            "google" => {
                let default_model = models_cfg
                    .default_model_for_provider("google")
                    .unwrap_or_default();
                Some(Arc::new(ai::GeminiProvider::new(
                    profile.api_key.clone(),
                    profile.model.clone().unwrap_or(default_model),
                )))
            }
            "ollama" => {
                let default_model = models_cfg
                    .default_model_for_provider("ollama")
                    .unwrap_or_default();
                Some(Arc::new(ai::OllamaProvider::new(
                    profile
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "http://localhost:11434".into()),
                    profile.model.clone().unwrap_or(default_model),
                )))
            }
            "neboai" => {
                // Only create Janus provider if metadata has janus_provider=true
                let metadata: Option<serde_json::Value> = profile
                    .metadata
                    .as_ref()
                    .and_then(|m| serde_json::from_str(m).ok());
                let is_janus = metadata
                    .as_ref()
                    .and_then(|m| m.get("janus_provider"))
                    .and_then(|v| v.as_str())
                    == Some("true");
                if is_janus {
                    // Skip Janus if user has disabled all Janus chat models.
                    // Only count chat-capable models (not embedding-only).
                    // Fail-safe: if DB query fails, skip Janus (don't burn tokens).
                    let has_active_chat = store
                        .list_active_provider_models("janus")
                        .map(|models| {
                            models.iter().any(|m| {
                                let caps: Vec<String> = m
                                    .capabilities
                                    .as_ref()
                                    .and_then(|c| serde_json::from_str(c).ok())
                                    .unwrap_or_default();
                                caps.iter().any(|c| c == "streaming" || c == "tools")
                            })
                        })
                        .unwrap_or(false);
                    if !has_active_chat {
                        info!("janus provider has no active models in catalog, skipping");
                        None
                    } else {
                        // Janus URL comes from config (NeboAI.JanusURL), NOT auth_profile base_url
                        let janus_url = &cfg.neboai.janus_url;
                        let model = profile.model.clone().unwrap_or_else(|| "nebo-1".into());
                        let bot_id = config::read_bot_id().unwrap_or_default();
                        // Janus authenticates via X-Bot-ID header; api_key (OAuth token) is optional
                        let api_key = if profile.api_key.is_empty() {
                            bot_id.clone()
                        } else {
                            profile.api_key.clone()
                        };
                        info!(
                            model = %model,
                            janus_url = %janus_url,
                            bot_id = %bot_id,
                            "loaded Janus provider via NeboAI"
                        );
                        let mut p = ai::OpenAIProvider::with_base_url(
                            api_key,
                            model,
                            format!("{}/v1", janus_url),
                        );
                        p.set_provider_id("janus");
                        if !bot_id.is_empty() {
                            p.set_bot_id(bot_id);
                        }
                        Some(Arc::new(p))
                    }
                } else {
                    info!(
                        profile_id = %profile.id,
                        has_metadata = metadata.is_some(),
                        "neboai profile found but janus_provider not enabled, skipping AI provider"
                    );
                    None
                }
            }
            _ => {
                warn!(provider = %profile.provider, "unknown provider type, skipping");
                None
            }
        };
        if let Some(p) = provider {
            info!(
                provider = %profile.provider,
                model = %profile.model.as_deref().unwrap_or("-"),
                "loaded AI provider"
            );
            // Defer gateway providers (Janus) to end of the list so CLI
            // providers and direct API keys take priority.
            if profile.provider == "neboai" {
                gateway_providers.push(p);
            } else {
                providers.push(p);
            }
        }
    }

    // Auto-create Ollama provider if Ollama is running and has active models,
    // even without an auth_profile (Ollama needs no API key).
    let has_ollama_profile = profiles
        .iter()
        .any(|p| p.provider == "ollama" && p.is_active.unwrap_or(0) == 1);
    if !has_ollama_profile {
        if let Ok(active_models) = store.list_active_provider_models("ollama") {
            if !active_models.is_empty() {
                let model = active_models[0].model_id.clone();
                info!(model = %model, "auto-creating Ollama provider (no auth profile needed)");
                providers.push(Arc::new(ai::OllamaProvider::new(
                    "http://localhost:11434".into(),
                    model,
                )));
            }
        }
    }

    // Add CLI providers from models.yaml config
    if let Some(statuses) = cli_statuses {
        let models_cfg_ref = config::ModelsConfig::load();
        for cli_def in &models_cfg_ref.cli_providers {
            if !cli_def.is_active() {
                continue;
            }
            let installed = match cli_def.command.as_str() {
                "claude" => statuses.claude.installed,
                "codex" => statuses.codex.installed,
                "gemini" => statuses.gemini.installed,
                _ => false,
            };
            if !installed {
                continue;
            }
            let p: Arc<dyn ai::Provider> = match cli_def.command.as_str() {
                "claude" => Arc::new(ai::CLIProvider::new_claude_code(0, cfg.port)),
                "codex" => Arc::new(ai::CLIProvider::new_codex_cli()),
                "gemini" => Arc::new(ai::CLIProvider::new_gemini_cli()),
                _ => continue,
            };
            info!(
                cli = %cli_def.command,
                name = %cli_def.display_name,
                "loaded CLI provider"
            );
            providers.push(p);
        }
    }

    // Gateway providers (Janus) go last — they consume Nebo credits and
    // should only be used when no direct API key or CLI provider is available.
    providers.extend(gateway_providers);

    if providers.is_empty() {
        warn!(
            "no active AI providers configured — agent will be unavailable until providers are added"
        );
    }

    providers
}

/// Run the Nebo HTTP server.
/// Handle a comm "installs" event. tool_installed/tool_updated route through the
/// canonical install pathway (`codes::handle_code`), resolving the artifact from
/// its id — reusing the store/code install's robust download (JSON-indirection
/// follow + retries + NAPP-envelope/sealed/license handling) and type-correct
/// persistence, instead of a separate naive downloader. Uninstall/revoke stay on
/// the napp registry (they don't download). One install pathway, CODE_AUDITOR
/// Rule 8. Note: the artifact id (`tool_id`) is resolved here, so the event's
/// `payload.download_url` is intentionally ignored.
async fn handle_comm_install_event(
    state: &AppState,
    event: napp::InstallEvent,
) -> Result<(), String> {
    match event.event_type.as_str() {
        "tool_installed" | "tool_updated" => {
            let api = codes::build_api_client(state).map_err(|e| e.to_string())?;
            let detail = api
                .get_skill(&event.tool_id)
                .await
                .map_err(|e| format!("fetch artifact {}: {e}", event.tool_id))?;
            let item = &detail.item;
            let artifact_type = item.artifact_type.as_deref().unwrap_or("skill");
            // Dedup the self-echo: a fresh "tool_installed" for something already
            // present (e.g. the device that just installed it) skips the redundant
            // re-download. Updates always re-install.
            if event.event_type == "tool_installed"
                && crate::handlers::store::is_installed(
                    &item.slug,
                    &item.name,
                    artifact_type,
                    &state.store,
                )
            {
                tracing::debug!(tool_id = %event.tool_id, slug = %item.slug, "install event: already installed, skipping");
                return Ok(());
            }
            let code = item
                .code
                .as_deref()
                .filter(|c| !c.is_empty())
                .ok_or_else(|| format!("artifact {} has no install code", event.tool_id))?;
            let (code_type, validated) =
                codes::detect_code(code).ok_or_else(|| format!("invalid install code: {code}"))?;
            codes::handle_code(
                state,
                code_type,
                validated,
                &format!("install-event-{}", event.tool_id),
            )
            .await;
            Ok(())
        }
        _ => {
            // tool_uninstalled / tool_revoked — no download; registry path.
            state
                .napp_registry
                .handle_install_event(event)
                .await
                .map_err(|e| e.to_string())
        }
    }
}

pub async fn run(cfg: Config, quiet: bool) -> Result<(), NeboError> {
    let port = cfg.port;
    let host = cfg.host.clone();
    let bind_addr = format!("{host}:{port}");

    // Check port availability
    TcpListener::bind(&bind_addr).map_err(|_| NeboError::PortInUse(port))?;

    if !quiet {
        println!("Starting server on http://localhost:{port}");
    }

    // Reap any orphan plugin/agent processes left over from a prior crashed
    // or SIGKILL'd Nebo. Without this, hot-reload restarts accumulate orphans
    // that hold sockets and post duplicate channel placeholders.
    let orphans = napp::child_guard::cleanup_orphans_at_startup();
    if orphans > 0 {
        info!(orphans_killed = orphans, "startup: reaped orphan child processes from previous run");
    }

    // Install SIGTERM/SIGINT/SIGHUP handler so children die with us on shutdown.
    napp::child_guard::install_signal_handler();

    // Initialize database
    let store = Arc::new(db::Store::new(&cfg.database.sqlite_path)?);

    // Ensure FTS5 index for memories is healthy (auto-rebuild if corrupted)
    if let Err(e) = store.ensure_fts_healthy() {
        warn!(error = %e, "FTS health check failed — memory search may be degraded");
    }

    // Clean up orphaned workflow runs from previous shutdown
    match store.cleanup_orphaned_runs() {
        Ok(0) => {}
        Ok(n) => info!(
            count = n,
            "cancelled orphaned workflow runs from previous session"
        ),
        Err(e) => warn!(error = %e, "failed to clean up orphaned workflow runs"),
    }

    // Cloud/container deploys are provisioned as a specific bot up front, so
    // there is no interactive pairing flow to run. NEBO_BOT_ID seeds the same
    // file the pairing flow writes, so the chain below is unchanged.
    if config::read_bot_id().is_none()
        && let Ok(id) = std::env::var("NEBO_BOT_ID")
        && id.len() == 36
    {
        config::write_bot_id(&id)?;
        info!(bot_id = %id, "seeded bot_id from NEBO_BOT_ID");
    }

    // Ensure bot_id exists: env → file → DB (Go migration) → generate new
    if config::read_bot_id().is_none() {
        // Check DB for bot_id set by the Go version (plugin_settings table)
        let from_db = store
            .get_plugin_setting("neboai", "bot_id")
            .unwrap_or(None)
            .filter(|id| id.len() == 36);

        if let Some(id) = from_db {
            config::write_bot_id(&id)?;
            info!(bot_id = %id, "migrated bot_id from database");
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            config::write_bot_id(&id)?;
            info!(bot_id = %id, "generated new bot_id");
        }
    }
    // Sync bot_id to DB for backward compatibility
    if let Some(bot_id) = config::read_bot_id() {
        let _ = store.set_plugin_setting("neboai", "bot_id", &bot_id);
    }

    // Seed the NeboAI credential for env-provisioned (cloud) deploys as an active
    // auth profile — the counterpart to NEBO_BOT_ID above. This is the SAME store
    // the OAuth callback writes and `activate_neboai`/`neboai_token` read, so the
    // pod auto-connects to the loop on boot with NO interactive OAuth (a cloud bot's
    // identity is provisioned up front, not paired in a browser). Seed ONLY when no
    // active profile exists: the token rotates at runtime and persists back to this
    // profile, so re-seeding the original env value on restart would push a stale
    // token and fail auth.
    if let Ok(tok) = std::env::var("NEBO_BOT_TOKEN")
        && !tok.is_empty()
    {
        // The profile metadata MUST carry janus_provider — build_providers only
        // constructs the Janus LLM provider when it's present (same key the
        // OAuth pathway writes in store_neboai_profile). Without it a cloud pod
        // connects to the loop but has NO providers and rejects every run.
        let mut meta = serde_json::Map::new();
        meta.insert("janus_provider".into(), "true".into());
        if let Some(owner) = jwt_claim(&tok, "ownerId") {
            meta.insert("owner_id".into(), owner.into());
        }
        let meta_json = serde_json::Value::Object(meta).to_string();

        let existing = store
            .list_all_active_auth_profiles_by_provider("neboai")
            .unwrap_or_default();
        if existing.is_empty() {
            // Seed failure means the pod can't authenticate to the loop, so surface it
            // rather than discarding — the cloud bot would silently never connect.
            let id = uuid::Uuid::new_v4().to_string();
            match store.create_auth_profile(
                &id,
                "NeboAI",
                "neboai",
                &tok,
                None,
                Some(&cfg.neboai.api_url),
                0,
                1,
                Some("token"),
                Some(&meta_json),
            ) {
                Ok(_) => info!("seeded NeboAI auth profile from NEBO_BOT_TOKEN (first boot)"),
                Err(e) => warn!(error = %e, "failed to seed NeboAI auth profile from NEBO_BOT_TOKEN"),
            }
        } else if let Some(profile) = existing.first() {
            // Repair pods seeded before the metadata fix: patch janus_provider in
            // (merging any existing metadata) WITHOUT touching the api_key, which
            // rotates at runtime. Desktop installs never set NEBO_BOT_TOKEN, so
            // their OAuth-managed profiles are never touched here.
            let mut merged = profile
                .metadata
                .as_deref()
                .and_then(|m| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(m).ok())
                .unwrap_or_default();
            if !merged.contains_key("janus_provider") {
                for (k, v) in serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&meta_json)
                    .unwrap_or_default()
                {
                    merged.entry(k).or_insert(v);
                }
                let merged_json = serde_json::Value::Object(merged).to_string();
                match store.update_auth_profile_metadata(&profile.id, &merged_json) {
                    Ok(_) => info!("repaired NeboAI auth profile metadata (janus_provider seeded)"),
                    Err(e) => warn!(error = %e, "failed to repair NeboAI auth profile metadata"),
                }
            }
        }
    }

    // NOTE: setup/onboarding completion is driven ONLY by the user finishing
    // the onboarding flow (POST /api/v1/setup/complete -> mark_setup_complete()).
    // We must NOT auto-mark it here just because a bot_id exists: bot_id is
    // generated automatically on first boot (above), so auto-marking would fire
    // on a brand-new install before the user ever sees onboarding, causing the
    // app to skip straight into the main view.
    //
    // The ONE exception is server mode: a provisioned cloud pod has no human at
    // the machine to click through desktop onboarding (permissions, Full Access,
    // account linking) — its identity and credentials are injected at provision
    // time above. Without this, opening the bot through the tunnel lands on an
    // onboarding flow that can never be completed.
    if tools::server_mode() && !config::is_setup_complete().unwrap_or(false) {
        match config::mark_setup_complete() {
            Ok(()) => info!("server mode: marked setup complete (provisioned deploy, no interactive onboarding)"),
            Err(e) => warn!(error = %e, "server mode: failed to mark setup complete"),
        }
    }

    // Initialize auth service
    let auth_service = Arc::new(auth::AuthService::new(store.clone(), cfg.clone()));

    // Initialize client hub for WebSocket broadcasts
    let hub = Arc::new(ClientHub::new());

    // Detect installed CLI tools (needed for build_providers and AppState)
    let cli_statuses = Arc::new(config::detect_all_clis());
    info!(
        claude = cli_statuses.claude.installed,
        codex = cli_statuses.codex.installed,
        gemini = cli_statuses.gemini.installed,
        "CLI detection complete"
    );

    // Build AI providers from database auth profiles + active CLI providers
    let mut providers = build_providers(&store, &cfg, Some(&cli_statuses));

    // Build tool registry with default tools
    let mut policy = tools::Policy::new();
    policy.level = tools::PolicyLevel::Full;
    policy.ask_mode = tools::AskMode::Off;
    // No-op: Nebo uses the platform-native data directory (see config::data_dir).
    migration::migrate_data_dir();

    let data_dir = config::data_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // Restart recovery: in-flight runs aren't resumed (the agent loop's state is in memory and
    // research runs aren't checkpointed-to-resume). So on startup, mark orphaned research runs
    // as failed and leave a one-line note in any chat that was mid-turn when we stopped, so it
    // doesn't hang silently. The note is persisted; the user sees it on next chat open.
    {
        let n = tools::research::mark_orphaned_runs(&data_dir.join("research"));
        if n > 0 {
            info!(count = n, "restart recovery: marked orphaned research runs failed");
        }
        let cutoff = chrono::Utc::now().timestamp() - 30 * 60;
        match store.find_interrupted_chats(cutoff) {
            Ok(chats) => {
                for (chat_id, _session_name) in &chats {
                    let msg_id = uuid::Uuid::new_v4().to_string();
                    let text = "I was interrupted by a restart before I could finish — want me to \
                                pick up where I left off? Just say \"continue\".";
                    let _ = store.create_chat_message(&msg_id, chat_id, "assistant", text, None);
                }
                if !chats.is_empty() {
                    info!(count = chats.len(), "restart recovery: noted interrupted chats");
                }
            }
            Err(e) => warn!(error = %e, "restart recovery: failed to scan for interrupted chats"),
        }
    }

    let tool_registry = Arc::new(tools::Registry::new(policy));

    // Create empty orchestrator handle (filled after Runner is built)
    let orch_handle = tools::new_handle();

    // Initialize browser manager (with built-in ExtensionBridge for Chrome extension relay).
    // Pass the Nebo data root: the Manager appends `browser/<profile>` for managed-Chrome
    // profiles itself (config.rs `resolve_profile`) and resolves the bundled Obscura binary at
    // `<root>/bin/obscura` (cdp_bridge `find_obscura`). Passing `data_dir/browser` here would
    // double-nest profiles AND hide the Obscura binary, disabling the tier-2 CDP fallback.
    let browser_config = browser::BrowserConfig::default();
    let browser_data_dir = data_dir.to_string_lossy().to_string();
    let browser_manager = Arc::new(browser::Manager::new(browser_config, browser_data_dir));
    let extension_bridge = browser_manager.bridge();

    // Install/update native messaging host manifest for Chrome extension.
    // The manifest must point to the `nebo` CLI binary (which has the relay code),
    // NOT `nebo-desktop` (the Tauri GUI). When running as `nebo-desktop`, we find
    // the sibling `nebo` binary in the same directory.
    {
        let nebo_binary = std::env::current_exe()
            .map(|p| {
                if p.file_name().and_then(|n| n.to_str()) == Some("nebo-desktop") {
                    let sibling = p.with_file_name("nebo");
                    if sibling.exists() {
                        return sibling.to_string_lossy().to_string();
                    }
                }
                p.to_string_lossy().to_string()
            })
            .unwrap_or_else(|_| "nebo".to_string());
        let local_ext_id = cfg.browser_extension_id.as_deref().unwrap_or("");
        if browser::native_host::needs_manifest_update(&nebo_binary, local_ext_id) {
            if let Err(e) = browser::native_host::install_manifest(&nebo_binary, local_ext_id) {
                warn!("failed to install native messaging manifest: {}", e);
            }
        }
        // Generate the per-install relay secret so it exists before any relay
        // (launched by the browser) connects to /ws/extension.
        if let Err(e) = config::ensure_extension_secret() {
            warn!("failed to prepare extension relay secret: {}", e);
        }
    }

    // Ensure artifact directory structure exists (nebo/ and user/ namespaces)
    if let Err(e) = config::ensure_artifact_dirs() {
        warn!("failed to create artifact directories: {}", e);
    }

    // Run one-time migration from old layout to nebo/user split
    migration::migrate_if_needed(&data_dir);

    // Seed bundled .napp files from app resources (re-runs on app version upgrade)
    migration::seed_bundled_napps(&data_dir);

    // Extract sealed .napp archives to sibling directories (one-time)
    // Must run AFTER seeding so newly seeded .napp files are picked up.
    migration::migrate_napp_extraction(&data_dir);

    // Initialize plugin store for shared binary management
    let plugins_dir = data_dir.join("nebo").join("plugins");
    let _ = std::fs::create_dir_all(&plugins_dir);
    let user_plugins_dir = data_dir.join("user").join("plugins");
    let _ = std::fs::create_dir_all(&user_plugins_dir);
    let plugin_store = Arc::new(napp::plugin::PluginStore::new(
        plugins_dir,
        user_plugins_dir,
        None,
    ));

    // Recover plugin installs interrupted mid-swap by a prior crash/hot-reload
    // SIGKILL (orphaned `<version>.staging` dirs). Must run before the plugin
    // scan / skill load below so resumed plugins are picked up.
    plugin_store.reconcile_orphaned_staging().await;

    // Populate plugin env var cache from DB (stored API keys, tokens, etc.)
    {
        let installed = plugin_store.list_installed();
        for (slug, _, _, _) in &installed {
            if let Ok(settings) = store.list_plugin_settings_by_slug(slug) {
                let vars: std::collections::HashMap<String, String> = settings
                    .into_iter()
                    .filter(|s| !s.setting_value.is_empty())
                    .map(|s| (s.setting_key, s.setting_value))
                    .collect();
                if !vars.is_empty() {
                    plugin_store.set_env_vars(slug, vars);
                }
            }
        }
    }

    // Append plugin-provided AI providers (e.g., openrouter, local model servers)
    {
        let installed = plugin_store.list_installed();
        let mut seen = std::collections::HashSet::new();
        for (slug, _version, _path, _source) in &installed {
            if !seen.insert(slug.clone()) {
                continue;
            }
            if let Some(manifest) = plugin_store.get_manifest(slug) {
                if let Some(ref caps) = manifest.capabilities {
                    for pdef in &caps.providers {
                        if let Some(binary) = plugin_store.resolve(slug, "*") {
                            providers.push(Arc::new(plugin_provider::PluginProvider::new(
                                pdef,
                                slug,
                                binary,
                                plugin_store.clone(),
                            )));
                            info!(plugin = %slug, provider = %pdef.id, "registered plugin provider");
                        }
                    }
                }
            }
        }
    }

    // Initialize skill loader (embedded bundled + marketplace nebo/skills/ + user/skills/)
    let installed_skills_dir = data_dir.join("nebo").join("skills");
    let user_skills_dir = data_dir.join("user").join("skills");
    let skill_loader = Arc::new(
        tools::skills::Loader::new(installed_skills_dir, user_skills_dir)
            .with_plugin_store(plugin_store.clone())
            .with_db_store(store.clone()),
    );

    // Load cached license keys from DB for sealed .napp decryption.
    // Keys were fetched from NeboAI on a previous startup and cached with TTL.
    // Shared by both the skill loader (below) and the agent loader (later) so
    // sealed skills AND sealed agents decrypt in memory.
    let cached_license_keys: std::collections::HashMap<String, [u8; 32]> = {
        use base64::Engine;
        let cached_keys = store.list_license_key_artifact_ids().unwrap_or_default();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let mut keys = std::collections::HashMap::new();
        for artifact_id in &cached_keys {
            if let Ok(Some(row)) = store.get_license_key(artifact_id) {
                if row.expires_at > now {
                    // Decrypt the stored key with keyring master key
                    if let Ok(plaintext) = auth::credential::decrypt(&row.encrypted_key) {
                        if let Ok(key_bytes) =
                            base64::engine::general_purpose::STANDARD.decode(&plaintext)
                        {
                            if key_bytes.len() == 32 {
                                let mut key = [0u8; 32];
                                key.copy_from_slice(&key_bytes);
                                keys.insert(artifact_id.clone(), key);
                            }
                        }
                    }
                }
            }
        }
        keys
    };
    if !cached_license_keys.is_empty() {
        info!(
            count = cached_license_keys.len(),
            "loaded cached license keys for sealed .napp files"
        );
        skill_loader
            .set_license_keys(cached_license_keys.clone())
            .await;
    }

    skill_loader.load_all().await;
    skill_loader.watch();

    // Background: verify skill manifest hashes + re-check dependencies.
    // On warm start this catches skills that changed while the server was down.
    {
        let bg_loader = skill_loader.clone();
        tokio::spawn(async move {
            bg_loader.verify_and_refresh_manifest().await;
        });
    }

    // Initialize advisor loader and runner (ADVISOR.md + DB advisors, LLM deliberation)
    let advisors_dir = data_dir.join("advisors");
    let advisor_loader = Arc::new(agent::advisors::Loader::new(advisors_dir, store.clone()));
    advisor_loader.load_all().await;
    advisor_loader.watch();

    // Build a second provider set for advisor deliberation (includes CLI providers)
    let advisor_providers = build_providers(&store, &cfg, Some(&cli_statuses));
    let shared_providers = Arc::new(advisor_providers);
    let advisor_runner: Option<Arc<dyn tools::AdvisorDeliberator>> = if shared_providers.is_empty() {
        None
    } else {
        Some(Arc::new(agent::advisors::Runner::new(
            advisor_loader,
            shared_providers.clone(),
        )))
    };

    // Structured-output sub-agent runner for the deep-research harness. Shares the same
    // provider set; absent when no provider can force tool calls.
    let structured_agent: Option<Arc<dyn tools::bot_tool::StructuredAgent>> =
        if shared_providers.is_empty() {
            None
        } else {
            Some(Arc::new(agent::structured_agent::StructuredRunner::new(
                shared_providers.clone(),
                tool_registry.clone(),
            )))
        };

    // Build embedding provider for vector search (memory embedding + transcript indexing)
    let embedding_provider = build_embedding_provider(&store, &cfg);

    // Create hybrid search adapter (FTS5 + vector similarity for memory search)
    // TurboVec indexes are lazy-loaded per user_id on first search.
    let hybrid_searcher: Arc<dyn tools::HybridSearcher> = Arc::new(
        agent::search_adapter::HybridSearchAdapter::new(store.clone(), embedding_provider.clone()),
    );

    // Initialize napp package registry
    let napp_config = napp::RegistryConfig {
        installed_tools_dir: data_dir.join("nebo").join("tools"),
        user_tools_dir: data_dir.join("user").join("tools"),
        neboai_url: Some(cfg.neboai.api_url.clone()),
    };
    let napp_registry = Arc::new(napp::Registry::new(napp_config, port));

    // Plan tier — updated by NeboAI AUTH_OK handler, read by ExecuteTool
    let plan_tier = Arc::new(tokio::sync::RwLock::new("free".to_string()));

    // Initialize OS-level sandbox for script execution (macOS Seatbelt / Linux bubblewrap)
    let sandbox_manager = {
        let mut mgr = sandbox_runtime::SandboxManager::new();
        if mgr.is_supported_platform() {
            match mgr
                .initialize(
                    sandbox_runtime::SandboxRuntimeConfig::default_config(),
                    None,
                    false,
                )
                .await
            {
                Ok(()) => {
                    info!("sandbox runtime initialized");
                    Some(Arc::new(mgr))
                }
                Err(e) => {
                    warn!("sandbox init failed, scripts will run unsandboxed: {e}");
                    None
                }
            }
        } else {
            None
        }
    };

    // Create shared agent registry — multiple agents can be active concurrently, each with isolated persona
    let active_role_state: tools::AgentRegistry =
        std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));

    // Create broadcaster closure for tools to emit WS events
    let hub_for_tools = hub.clone();
    let broadcaster: tools::web_tool::Broadcaster = Arc::new(move |event_type, payload| {
        hub_for_tools.broadcast(event_type, payload);
    });

    // Create a late-binding handle for run visibility from tools → RunRegistry.
    // The OnceLock is set after AppState is constructed (which owns the RunRegistry).
    let run_querier_handle = tools::run_querier::new_handle();

    // The NeboAI comm plugin handle exists from startup; its `is_connected()`
    // reflects live state. The loop tool holds this same handle, so it becomes
    // functional the moment the connection comes up — no registry rebuild needed.
    // (Also registered with the comm manager below.)
    let neboai_plugin: Arc<dyn comm::CommPlugin> = Arc::new(comm::NeboAIPlugin::new());

    tool_registry.set_plugin_store(plugin_store.clone());
    tool_registry
        .register_all_with_permissions(
            store.clone(),
            Some(browser_manager),
            orch_handle.clone(),
            Some(skill_loader.clone()),
            advisor_runner,
            Some(hybrid_searcher),
            structured_agent,
            None, // workflow_manager registered separately after Runner is created
            None,
            Some(plan_tier.clone()),
            sandbox_manager,
            Some(neboai_plugin.clone()),
            Some(active_role_state.clone()),
            Some(broadcaster),
            Some(run_querier_handle.clone()),
        )
        .await;

    // ToolSearch meta-tool — always active, lets LLM discover deferred tools on demand.
    // Must be registered after register_all_with_permissions since it needs Arc<Registry>.
    tool_registry
        .register(Box::new(tools::ToolSearchTool::new(tool_registry.clone())))
        .await;

    // Initialize encryption: try OS keyring → file key → generate new
    let encryptor = if let Some(key_hex) = auth::keyring::get() {
        // Keyring has the master key
        if key_hex.len() == 64 {
            // Hex-encoded 32-byte key
            let mut key = [0u8; 32];
            if hex::decode_to_slice(&key_hex, &mut key).is_ok() {
                mcp::crypto::Encryptor::new(key)
            } else {
                mcp::crypto::Encryptor::from_passphrase(&key_hex)
            }
        } else {
            mcp::crypto::Encryptor::from_passphrase(&key_hex)
        }
    } else {
        // Resolve from env/file or generate new
        let enc = mcp::crypto::resolve_encryption_key(&data_dir);
        // Try to store in keyring for next time
        if auth::keyring::available() {
            let key_hex = hex::encode(enc.key_bytes());
            if let Err(e) = auth::keyring::set(&key_hex) {
                warn!("failed to store master key in keyring: {}", e);
            } else {
                info!("stored master encryption key in OS keyring");
            }
        }
        enc
    };

    // Initialize credential system with the resolved key
    auth::credential::init(mcp::crypto::Encryptor::new(*encryptor.key_bytes()));

    let encryptor = Arc::new(encryptor);
    let mcp_client = Arc::new(mcp::McpClient::new(encryptor));
    let bridge = Arc::new(mcp::Bridge::new(mcp_client, tool_registry.clone()));
    tool_registry.set_bridge(bridge.clone());
    // Store is needed by MCP proxy tools for OAuth token refresh during calls.
    tool_registry.set_store(store.clone());
    // Wire owner alerts (message notify/alert) to the frontend: the message tool
    // broadcasts a `notification` event through this callback, which the bell + the
    // desktop HUD pick up. Boundary-clean — crates/tools never touches ClientHub.
    let hub_for_notify = hub.clone();
    let alert_notify_fn: tools::message_tool::NotifyFn = Arc::new(move |event_type, payload| {
        hub_for_notify.broadcast(event_type, payload);
    });
    tool_registry.set_notify_fn(alert_notify_fn);

    // Register the MCP enumeration tool — mcp(action:"list") lists connected servers.
    // Each server's tools are exposed as their own mcp__<server>__<tool> proxy tools.
    let mcp_tool = tools::mcp_tool::McpTool::new(bridge.clone());
    tool_registry.register(Box::new(mcp_tool)).await;

    // Sync MCP integrations from DB — reconnect with stored OAuth tokens
    let bridge_init = bridge.clone();
    let store_init = store.clone();
    tokio::spawn(async move {
        match store_init.list_mcp_integrations() {
            Ok(integrations) => {
                for i in &integrations {
                    if i.is_enabled.unwrap_or(0) == 0 {
                        continue;
                    }
                    // Remote servers need a URL; stdio servers carry a command in metadata.
                    let server_url = i.server_url.clone().unwrap_or_default();
                    let is_stdio =
                        crate::handlers::integrations::metadata_is_stdio(i.metadata.as_deref());
                    if server_url.is_empty() && !is_stdio {
                        continue;
                    }
                    // Skip OAuth integrations that haven't completed auth yet
                    if i.auth_type == "oauth" && i.connection_status.is_none() {
                        continue;
                    }
                    // Resolve the OAuth token (refresh if expired). On failure, surface
                    // needs_reauth and skip — never reconnect with a stale token (the
                    // bug that silently dropped servers to 401 on restart).
                    let access_token = match tools::mcp_tool::resolve_mcp_token(
                        &store_init,
                        bridge_init.client(),
                        i,
                    )
                    .await
                    {
                        tools::mcp_tool::TokenResolution::Ready(t) => t,
                        tools::mcp_tool::TokenResolution::NeedsReauth => {
                            let _ =
                                store_init.set_mcp_connection_status(&i.id, "needs_reauth", 0);
                            warn!(name = %i.name, "MCP token needs reauth on startup — skipping connect");
                            continue;
                        }
                    };
                    let tool_prefix = i
                        .name
                        .to_lowercase()
                        .chars()
                        .map(|c| if c.is_alphanumeric() { c } else { '_' })
                        .collect::<String>()
                        .trim_matches('_')
                        .to_string();
                    match bridge_init
                        .connect(
                            &i.id,
                            &tool_prefix,
                            &server_url,
                            access_token.as_deref(),
                            i.metadata.as_deref(),
                        )
                        .await
                    {
                        Ok(tools) => {
                            let _ = store_init.set_mcp_connection_status(
                                &i.id,
                                "connected",
                                tools.len() as i64,
                            );
                            info!(name = %i.name, tools = tools.len(), "MCP reconnected on startup");
                        }
                        Err(e) => {
                            let _ = store_init.set_mcp_connection_status(&i.id, "error", 0);
                            warn!(name = %i.name, error = %e, "MCP reconnect failed on startup");
                        }
                    }
                }
            }
            Err(e) => {
                warn!("failed to load MCP integrations for sync: {}", e);
            }
        }
    });

    // Discover and launch installed tools (best-effort, don't block startup)
    {
        let reg = napp_registry.clone();
        tokio::spawn(async move {
            if let Err(e) = reg.discover_and_launch().await {
                warn!("tool discovery failed: {}", e);
            }
        });
    }

    // Auth cache is populated later (awaited before agent workers start, see below).

    // Set quarantine handler to broadcast via hub
    {
        let hub = hub.clone();
        napp_registry
            .set_quarantine_handler(move |event| {
                hub.broadcast(
                    "tool_quarantined",
                    serde_json::json!({
                        "toolId": event.tool_id,
                        "reason": event.reason,
                    }),
                );
            })
            .await;
    }

    // Spawn tool supervisor (15s health check)
    {
        let registry = napp_registry.clone();
        let hub_ref = hub.clone();
        tokio::spawn(async move {
            let supervisor = napp::supervisor::Supervisor::new();
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
            loop {
                interval.tick().await;
                let tools = registry.list_processes().await;
                for tool in &tools {
                    if tool.running {
                        continue;
                    }
                    if supervisor.should_restart(&tool.id).await {
                        supervisor.record_restart(&tool.id).await;
                        hub_ref.broadcast(
                            "tool_error",
                            serde_json::json!({
                                "toolId": tool.id,
                                "error": "process died",
                            }),
                        );
                    }
                }
            }
        });
    }

    // Create comm plugin manager
    let comm_manager = Arc::new(comm::PluginManager::new());
    {
        let loopback_plugin = Arc::new(comm::LoopbackPlugin::new());
        comm_manager.register(neboai_plugin.clone()).await;
        comm_manager.register(loopback_plugin).await;
        // The incoming-message handler (broadcast + install-event routing) is wired
        // AFTER AppState is built so install events can route through the canonical
        // install pathway (codes::handle_code) — see set_message_handler below.
    }

    // NeboAI auto-connect and reconnect watcher are spawned after AppState construction
    // (see below) so they can use codes::activate_neboai(&state).

    // Create lane manager and start pumps
    let lanes = Arc::new(agent::LaneManager::new());
    lanes.start_pumps();

    // Create adaptive concurrency controller and spawn resource monitor
    let concurrency = Arc::new(agent::ConcurrencyController::new(
        cfg.max_concurrent_runs(),
    ));
    agent::concurrency::spawn_monitor(concurrency.clone());

    // Load models catalog from embedded models.yaml (needed for selector before runner)
    let models_cfg = config::ModelsConfig::load();
    let model_count: usize = models_cfg.providers.values().map(|v| v.len()).sum();
    info!(
        providers = models_cfg.providers.len(),
        models = model_count,
        "loaded models catalog"
    );

    // Collect active provider IDs from auth profiles
    let active_provider_ids: Vec<String> = providers.iter().map(|p| p.id().to_string()).collect();

    // Build DB model overrides so the selector respects user toggles
    let model_overrides = build_model_overrides(&store);

    // Build real routing config from models catalog
    let routing_config = agent::selector::ModelRoutingConfig::from_models_config(
        &models_cfg,
        &active_provider_ids,
        &model_overrides,
    );
    let selector = agent::ModelSelector::new(routing_config);

    // Inject Ollama models from DB (they're auto-discovered, not in the yaml)
    inject_ollama_models(&store, &selector);

    // Set loaded providers and rebuild fuzzy with user aliases
    selector.set_loaded_providers(active_provider_ids);
    let user_aliases: std::collections::HashMap<String, String> = models_cfg
        .aliases
        .iter()
        .map(|a| (a.alias.clone(), a.model_id.clone()))
        .collect();
    selector.rebuild_fuzzy(&user_aliases);

    let hooks = Arc::new(napp::HookDispatcher::new());

    // Create shared MCP context for CLI provider tool calls
    let mcp_context = Arc::new(tokio::sync::Mutex::new(tools::ToolContext {
        origin: tools::Origin::Mcp,
        user_id: "mcp-client".into(),
        session_key: "mcp".into(),
        ..Default::default()
    }));

    let ask_channels: tools::AskChannels =
        Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let approval_channels: tools::ApprovalChannels =
        Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));

    let mut runner_builder = agent::Runner::new(
        store.clone(),
        tool_registry.clone(),
        providers,
        selector,
        concurrency.clone(),
        hooks.clone(),
        Some(mcp_context.clone()),
        active_role_state.clone(),
        Some(skill_loader.clone()),
    )
    .set_ask_channels(ask_channels.clone())
    .set_approval_channels(approval_channels.clone());

    if let Some(ep) = embedding_provider {
        runner_builder = runner_builder.set_embedding_provider(ep);
    }

    let runner = Arc::new(runner_builder);

    // Spawn background memory consolidation sweep (30-min interval, per-scope dedup/prune)
    agent::memory_consolidation::spawn_sweep(store.clone(), runner.providers());

    // Create event bus and dispatcher for workflow-to-workflow events
    let (event_bus, event_rx) = tools::EventBus::new();
    let event_dispatcher = Arc::new(workflow::events::EventDispatcher::new());

    // Register EmitTool so it appears in tools list and is available to all origins
    tool_registry
        .register(Box::new(tools::EmitTool::new(event_bus.clone())))
        .await;

    // Create workflow manager (needs runner's shared providers for background execution)
    let workflow_manager = Arc::new(workflow_manager::WorkflowManagerImpl::new(
        store.clone(),
        runner.providers(),
        tool_registry.clone(),
        hub.clone(),
        cfg.clone(),
        Some(event_bus.clone()),
        Some(skill_loader.clone()),
    ));
    // Register WorkTool now that the manager exists
    tool_registry
        .register(Box::new(tools::WorkTool::new(
            workflow_manager.clone() as Arc<dyn tools::WorkflowManager>
        )))
        .await;

    // Create agent loader — embedded bundled + nebo/agents/ + user/agents/
    let agent_loader = Arc::new(
        napp::AgentLoader::new(
            data_dir.join("nebo").join("agents"),
            data_dir.join("user").join("agents"),
        )
        .with_bundled(tools::skills::bundled::BUNDLED_AGENTS),
    );
    // Provide cached license keys so sealed (paid) agents decrypt in memory.
    if !cached_license_keys.is_empty() {
        agent_loader
            .set_license_keys(cached_license_keys.clone())
            .await;
    }
    agent_loader.load_all().await;
    let (_watcher_handle, agent_fs_rx) = agent_loader.watch();
    tool_registry.set_agent_loader(agent_loader.clone());

    // Sync filesystem agent content → DB (keeps DB content columns fresh + recovers missing records)
    // Collect frontmatter of newly created agents for dependency cascade after AppState is ready.
    let mut agents_needing_cascade: Vec<String> = Vec::new();
    {
        let fs_agents = agent_loader.list().await;
        let mut synced = 0usize;
        let mut created = 0usize;
        for loaded in &fs_agents {
            // Match by manifest ID first (marketplace agents), then by name
            let db_agent = loaded
                .id
                .as_deref()
                .and_then(|id| store.get_agent(id).ok().flatten())
                .or_else(|| {
                    store
                        .get_agent_by_name(&loaded.agent_def.name)
                        .ok()
                        .flatten()
                });

            let agent_id_for_bindings;
            if let Some(db_agent) = db_agent {
                // Refresh filesystem-owned content.
                let _ = store.sync_agent_content(
                    &db_agent.id,
                    &loaded.agent_md,
                    &loaded.frontmatter,
                );
                // Sync display name/description from manifest.
                let _ = store.sync_agent_identity(
                    &db_agent.id,
                    &loaded.agent_def.name,
                    &loaded.description,
                );
                agent_id_for_bindings = db_agent.id.clone();
                synced += 1;
            } else {
                // Agent on filesystem but not in DB — create DB record so it appears in UI
                let agent_id = loaded
                    .id
                    .clone()
                    .unwrap_or_else(|| loaded.agent_def.name.clone());
                let kind = match loaded.source {
                    napp::AgentSource::Installed => Some("installed"),
                    napp::AgentSource::User => Some("user"),
                };
                match store.create_agent(
                    &agent_id,
                    kind,
                    &loaded.agent_def.name,
                    &loaded.description,
                    &loaded.agent_md,
                    &loaded.frontmatter,
                    None,
                    None,
                ) {
                    Ok(_) => {
                        // The primary agent ("Nebo") is exposed to the loop by
                        // default. Set it once at row creation — the migration
                        // only covers installs that predate this row, so fresh
                        // installs (where the row is created after migrations
                        // run) need it here. The user can still turn it off.
                        if agent_id == "assistant" {
                            let _ = store.set_loop_exposed(&agent_id, true);
                        }
                        agent_id_for_bindings = agent_id;
                        created += 1;
                        // Queue for dependency cascade if agent has frontmatter
                        if !loaded.frontmatter.is_empty() {
                            agents_needing_cascade.push(loaded.frontmatter.clone());
                        }
                    }
                    Err(_) => continue,
                }
            }

            // Sync app fields (ui path, binary path, window config) to DB
            if loaded.is_app {
                let _ = store.set_agent_app_fields(
                    &agent_id_for_bindings,
                    true,
                    loaded.app_ui_path.as_ref().and_then(|p| p.to_str()),
                    loaded.app_binary_path.as_ref().and_then(|p| p.to_str()),
                    loaded
                        .app_window_config
                        .as_ref()
                        .and_then(|wc| serde_json::to_string(wc).ok())
                        .as_deref(),
                );
            }

            // Sync workflow bindings from agent.json
            if let Some(ref config) = loaded.config {
                sync_agent_workflows(&store, &agent_id_for_bindings, config);
            }
        }
        // Filesystem is the source of truth for which agents are active.
        // Soft-deactivate any DB agent not on the filesystem — same policy as
        // the fs-watcher's Removed branch. Do NOT delete: the listing may be
        // incomplete (partial load_all after transient IO), and the user may
        // re-add the directory; chats/sessions/memories must survive.
        let fs_ids: std::collections::HashSet<String> = fs_agents
            .iter()
            .map(|a| a.id.clone().unwrap_or_else(|| a.agent_def.name.clone()))
            .collect();
        if let Ok(db_agents) = store.list_agents(1000, 0) {
            let mut deactivated = 0usize;
            for db_agent in &db_agents {
                if !fs_ids.contains(&db_agent.id) && db_agent.is_enabled != 0 {
                    match store.set_agent_enabled(&db_agent.id, false) {
                        Ok(()) => {
                            deactivated += 1;
                            info!(id = %db_agent.id, name = %db_agent.name, "deactivated orphan agent missing from filesystem (data preserved)");
                        }
                        Err(e) => {
                            warn!(id = %db_agent.id, error = %e, "failed to deactivate orphan agent");
                        }
                    }
                }
            }
            if deactivated > 0 {
                info!(
                    deactivated,
                    "deactivated orphan agents missing from filesystem"
                );
            }
        }

        if synced > 0 || created > 0 {
            info!(
                synced,
                created, "synced agent content from filesystem to DB"
            );
        }
    }

    // Create agent worker registry — manages autonomous trigger lifecycle for each agent
    let hub_for_workers = hub.clone();
    let worker_notify_fn: agent::agent_worker::NotifyFn = Arc::new(move |event_type, payload| {
        hub_for_workers.broadcast(event_type, payload);
    });
    let agent_workers = Arc::new(agent::AgentWorkerRegistry::new(
        store.clone(),
        workflow_manager.clone() as Arc<dyn tools::WorkflowManager>,
        event_dispatcher.clone(),
        plugin_store.clone(),
        event_bus.clone(),
        Some(worker_notify_fn),
    ));

    // Late-wire the worker registry into the workflow manager (circular dep:
    // the registry was just built FROM the manager). Lets `work create` restart
    // an agent's worker so a new binding's live triggers register immediately.
    workflow_manager.set_agent_workers(agent_workers.clone());

    // Auth cache is populated lazily on first access (check_auth_lazy).
    // Watch processes handle auth failures at runtime via stderr detection,
    // so they don't need the cache pre-populated. This eliminates ~61s of
    // spawning 137 plugin binaries at startup.

    // Parse agent configs once, then reuse for both worker startup and registry population.
    // This eliminates 3x redundant parse_agent_config calls (and their duplicate warnings).
    {
        if let Ok(agents) = store.list_agents(1000, 0) {
            // Build config cache: parse each enabled agent's frontmatter once
            let agent_configs: std::collections::HashMap<String, napp::agent::AgentConfig> = agents
                .iter()
                .filter(|a| a.is_enabled != 0 && !a.frontmatter.is_empty())
                .filter_map(|a| {
                    napp::agent::parse_agent_config(&a.frontmatter)
                        .ok()
                        .map(|cfg| (a.id.clone(), cfg))
                })
                .collect();

            // Start workers for all enabled agents (pass pre-parsed config)
            let mut started = 0usize;
            for agent in &agents {
                if agent.is_enabled == 0 {
                    continue;
                }
                agent_workers
                    .start_agent(
                        &agent.id,
                        &agent.name,
                        agent_configs.get(&agent.id).cloned(),
                    )
                    .await;
                started += 1;
            }
            if started > 0 {
                info!(count = started, "started agent workers for enabled agents");
            }

            // Populate agent_registry from same cache (sidebar + runtime lookups)
            let mut registry = active_role_state.write().await;
            for agent in &agents {
                if agent.is_enabled == 0 {
                    continue;
                }
                registry.insert(
                    agent.id.clone(),
                    tools::ActiveAgent {
                        agent_id: agent.id.clone(),
                        name: agent.name.clone(),
                        agent_md: agent.agent_md.clone(),
                        config: agent_configs.get(&agent.id).cloned(),
                        channel_id: None,
                        degraded: None,
                        soul: agent.soul.clone(),
                        rules: agent.rules.clone(),
                    },
                );
            }
            if !registry.is_empty() {
                info!(count = registry.len(), "restored active agents from DB");
            }
        }
    }

    // Validate agent→skill dependencies — mark agents with missing skills as degraded
    tools::validate_agent_dependencies(&active_role_state, &skill_loader).await;

    // Spawn event dispatcher loop (matches events to role-owned subscriptions)
    event_dispatcher.clone().spawn(
        event_rx,
        workflow_manager.clone() as Arc<dyn tools::WorkflowManager>,
    );

    // Create orchestrator and fill the late-binding handle
    let orchestrator = agent::Orchestrator::new(runner.clone(), store.clone(), concurrency.clone())
        .with_lanes(lanes.clone());
    if orch_handle
        .set(Box::new(orchestrator) as Box<dyn tools::SubAgentOrchestrator>)
        .is_err()
    {
        panic!("orchestrator handle set twice");
    }

    // Recover incomplete sub-agent tasks from previous crash
    orch_handle.get().unwrap().recover().await;

    // Seed provider_models table from the models catalog loaded earlier
    seed_models_from_catalog(&store, &models_cfg);
    info!("seeded provider_models from embedded catalog");
    let models_config = Arc::new(models_cfg);

    // Create snapshot store for browser accessibility snapshots
    let snapshot_store = Arc::new(browser::SnapshotStore::new());

    // A2UI surface manager
    let a2ui_catalog = Arc::new(a2ui::NeboCatalogProvider::new());
    let a2ui_manager = Arc::new(a2ui::A2UIManager::new(
        hub.clone(),
        store.clone(),
        a2ui_catalog,
    ));
    a2ui_manager.restore_surfaces().await;
    tool_registry
        .register(Box::new(tools::A2UIDomainTool::new(
            a2ui_manager.clone() as Arc<dyn tools::A2UIHost>
        )))
        .await;

    let jwt_secret = JwtSecret(cfg.auth.access_secret.clone());

    // Seed from the persisted value so the personal-loop unification branch
    // is deterministic from the first inbound DM after a restart (reconcile
    // refreshes it once the connection is up). Read before `store` moves
    // into AppState below.
    let personal_loop_id_seed = store
        .get_plugin_setting("neboai", "personal_loop_id")
        .ok()
        .flatten();

    let state = AppState {
        config: cfg.clone(),
        store,
        auth: auth_service,
        hub,
        runner,
        tools: tool_registry,
        bridge,
        napp_registry,
        workflow_manager: workflow_manager.clone(),
        models_config,
        cli_statuses,
        lanes,
        snapshot_store,
        extension_bridge,
        comm_manager,
        approval_channels: approval_channels.clone(),
        ask_channels: ask_channels.clone(),
        pending_comm_asks: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        update_pending: Arc::new(tokio::sync::Mutex::new(None)),
        hooks,
        mcp_context,
        event_bus,
        event_dispatcher,
        plan_tier,
        skill_loader: skill_loader.clone(),
        agent_registry: active_role_state,
        agent_workers,
        janus_usage: Arc::new(tokio::sync::RwLock::new(None)),
        plugin_store,
        agent_loader,
        presence: Arc::new(agent::PresenceTracker::new()),
        proactive_inbox: Arc::new(agent::ProactiveInbox::new()),
        run_registry: run_registry::RunRegistry::new(),
        personal_loop_id: Arc::new(tokio::sync::RwLock::new(personal_loop_id_seed)),
        channel_providers: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        channel_bridges: tools::new_channel_bridge_registry(),
        a2ui: a2ui_manager,
        app_lifecycles: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        voice: Arc::new(voice::VoicePipeline::new(
            voice::VoicePipelineConfig::default(),
        )),
        channel_context: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        channel_engagement: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        store_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };

    // Install the chat-title sink: the runner generates+persists titles for every
    // run path, then this broadcasts the change + propagates it to the loop. One
    // generator, set once (CODE_AUDITOR Rule 8).
    state
        .runner
        .set_title_sink(std::sync::Arc::new(chat_dispatch::TitleBroadcaster::new(
            state.clone(),
        )));

    // Wire the comm incoming-message handler now that AppState exists. Install
    // events route through the SAME canonical install pathway as store/code
    // installs (codes::handle_code, resolved from the artifact id) instead of a
    // separate naive downloader — robust download + envelope/sealed handling +
    // type-correct install, one pathway (CODE_AUDITOR Rule 8).
    {
        let handler_state = state.clone();
        state
            .comm_manager
            .set_message_handler(Arc::new(move |msg: comm::CommMessage| {
                let st = handler_state.clone();
                if msg.topic == "installs" {
                    if let Ok(event) = serde_json::from_str::<napp::InstallEvent>(&msg.content) {
                        tokio::spawn(async move {
                            if let Err(e) = handle_comm_install_event(&st, event).await {
                                tracing::warn!("install event handling failed: {}", e);
                                st.hub.broadcast(
                                    "tool_error",
                                    serde_json::json!({"error": e}),
                                );
                            } else {
                                st.hub
                                    .broadcast("tool_event", serde_json::json!({"status": "ok"}));
                            }
                        });
                        return;
                    }
                }
                // Default: broadcast to clients
                st.hub.broadcast(
                    "comm_message",
                    serde_json::json!({
                        "from": msg.from,
                        "to": msg.to,
                        "content": msg.content,
                        "type": msg.msg_type,
                    }),
                );
            }))
            .await;
    }

    // Wire RunRegistry into the tool-layer run querier (late binding via OnceLock)
    let _ = run_querier_handle.set(Box::new(state.run_registry.clone()));

    // Wire the channel-bridge registry into the tools crate so plugin_tool and
    // agent_worker can reach the same registry without an AppState back-reference.
    tools::set_channel_bridges(state.channel_bridges.clone());

    // Keep OAuth MCP tokens fresh continuously so they never expire and drop a
    // server on reconnect/restart (renew proactively, not reactively at connect).
    crate::handlers::integrations::spawn_mcp_token_refresher(state.clone());

    // Same guarantee for OAuth plugin accounts (e.g. Google Workspace): probe each
    // connected account's auth periodically so its token stays fresh, and surface a
    // "Reconnect" prompt the moment a token can no longer be refreshed.
    crate::handlers::integrations::spawn_plugin_token_refresher(state.clone());

    // Wire channel dispatcher into agent workers (late binding via OnceLock).
    // Workers started before this point have channel_dispatch = None, so channels
    // don't start yet. We restart workers that declare channels below.
    state.agent_workers.set_channel_dispatch(Arc::new(
        channel_dispatch::ChannelDispatchImpl::new(state.clone()),
    ));

    // Wire the canonical marketplace-code installer into the agent's `registry` install
    // action (late, like the channel dispatcher above — both need `AppState`, built
    // after tool registration). With this set, `agent(resource:"registry",
    // action:"install", code:"<ANY>")` routes through `codes::handle_code`, so skills,
    // plugins (binary + re-registration), agents, apps, and collections all install AND
    // cascade through the ONE canonical pathway — no per-type bypass.
    state
        .tools
        .set_code_installer(Arc::new(channel_dispatch::CodeInstallerImpl::new(
            state.clone(),
        )));

    // Restart workers that have DB channel bindings (they were started before the
    // channel dispatcher was wired, so channels didn't start).
    {
        let bindings = state.store.list_enabled_channel_bindings().unwrap_or_default();
        // Collect unique agent IDs that have channel bindings
        let mut channel_agents: std::collections::HashSet<String> = std::collections::HashSet::new();
        for b in &bindings {
            channel_agents.insert(b.agent_id.clone());
        }
        for agent_id in &channel_agents {
            if let Ok(Some(agent)) = state.store.get_agent(agent_id) {
                let cfg = napp::agent::parse_agent_config(&agent.frontmatter).ok();
                info!(
                    agent = %agent_id,
                    "restarting agent worker to enable channel bindings"
                );
                state
                    .agent_workers
                    .start_agent(agent_id, &agent.name, cfg)
                    .await;
            }
        }
    }

    // Register structured tools + hooks for all installed plugins (startup recovery).
    {
        let installed = state.plugin_store.list_installed();
        let mut seen = std::collections::HashSet::new();
        for (slug, _version, _path, _source) in &installed {
            if !seen.insert(slug.clone()) {
                continue;
            }
            // Plugin command tools are discovered via the `plugin` STRAP tool (lookup),
            // not registered individually (13K+ tools overwhelm the LLM context).
            // Hooks
            if let Some(manifest) = state.plugin_store.get_manifest(slug) {
                if let Some(binary) = state.plugin_store.resolve(slug, "*") {
                    let count = napp::register_plugin_hooks(&manifest, &binary, &state.hooks, state.plugin_store.clone());
                    if count > 0 {
                        info!(plugin = %slug, hooks = count, "registered plugin hooks at startup");
                    }
                }
            }
        }
    }

    // Launch sidecars for enabled app agents (restore after restart).
    // Spawned as a background task so sidecar timeouts don't block server startup.
    {
        let startup_state = state.clone();
        tokio::spawn(async move {
            let agents = match startup_state.store.list_agents(1000, 0) {
                Ok(a) => a,
                Err(_) => return,
            };
            let mut launched = 0usize;
            for agent in &agents {
                if agent.is_enabled == 0 || agent.is_app.unwrap_or(0) == 0 {
                    continue;
                }
                if let Some(tool_dir) = handlers::agents::app_tool_dir(agent) {
                    let mut lifecycle = app_lifecycle::AppLifecycle::new(
                        agent.id.clone(),
                        tool_dir,
                        startup_state.hub.clone(),
                        startup_state.tools.clone(),
                        startup_state.skill_loader.clone(),
                        startup_state.config.port,
                    );
                    match lifecycle.launch().await {
                        Ok(()) => {
                            startup_state
                                .app_lifecycles
                                .write()
                                .await
                                .insert(agent.id.clone(), lifecycle);
                            launched += 1;
                        }
                        Err(e) => {
                            warn!(agent = %agent.id, error = %e, "failed to launch app sidecar at startup");
                        }
                    }
                }
            }
            if launched > 0 {
                info!(count = launched, "launched app sidecars at startup");
                // Re-validate now that app skills are loaded — clears degraded
                // flags set during early validation before sidecars were up.
                tools::validate_agent_dependencies(
                    &startup_state.agent_registry,
                    &startup_state.skill_loader,
                )
                .await;
            }
        });
    }

    // Replace comm message handler with full version that routes chat/DM to agent runner
    {
        let handler_state = state.clone();
        state
            .comm_manager
            .set_message_handler({
                Arc::new(move |msg: comm::CommMessage| {
                    let st = handler_state.clone();
                    tokio::spawn(async move {
                        handle_comm_message(st, msg).await;
                    });
                })
            })
            .await;
    }

    // Resolve dependency cascade for agents that were just created from filesystem.
    // This is the IMPLICIT boot-time reconcile — gated by the `auto_install_deps`
    // setting (default OFF) so we don't auto-pull deps for every agent on launch.
    if !agents_needing_cascade.is_empty() && crate::deps::auto_install_deps_enabled(&state) {
        let cascade_state = state.clone();
        tokio::spawn(async move {
            for frontmatter in agents_needing_cascade {
                let deps = crate::deps::extract_agent_deps_from_frontmatter(&frontmatter);
                if !deps.is_empty() {
                    let mut visited = std::collections::HashSet::new();
                    crate::deps::resolve_cascade(&cascade_state, deps, &mut visited).await;
                }
            }
        });
    }

    // Spawn filesystem agent watcher → DB + registry + WS sync
    {
        let fs_state = state.clone();
        tokio::spawn(async move {
            handle_agent_fs_events(fs_state, agent_fs_rx).await;
        });
    }

    // Spawn filesystem plugin watcher → log changes, notify via WS
    {
        let (_plugin_watcher_handle, mut plugin_fs_rx) = state.plugin_store.watch();
        let ps_state = state.clone();
        tokio::spawn(async move {
            while let Some(event) = plugin_fs_rx.recv().await {
                match event {
                    napp::plugin::PluginFsEvent::Added { slug, binary_path } => {
                        info!(slug = %slug, path = %binary_path.display(), "plugin hot-loaded (added)");
                        ps_state.hub.broadcast(
                            "plugin_changed",
                            serde_json::json!({"slug": slug, "action": "added"}),
                        );
                    }
                    napp::plugin::PluginFsEvent::Changed { slug, binary_path } => {
                        info!(slug = %slug, path = %binary_path.display(), "plugin hot-loaded (changed)");
                        ps_state.hub.broadcast(
                            "plugin_changed",
                            serde_json::json!({"slug": slug, "action": "changed"}),
                        );
                    }
                    napp::plugin::PluginFsEvent::Removed { slug } => {
                        info!(slug = %slug, "plugin removed from filesystem");
                        ps_state.hub.broadcast(
                            "plugin_changed",
                            serde_json::json!({"slug": slug, "action": "removed"}),
                        );
                    }
                }
            }
        });
    }

    // Auto-connect NeboAI if enabled and credentials exist
    if cfg.is_neboai_enabled() {
        let auto_state = state.clone();
        tokio::spawn(async move {
            match codes::activate_neboai(&auto_state).await {
                Ok(()) => info!("neboai: connected to gateway"),
                Err(e) => info!("neboai: auto-connect skipped: {}", e),
            }
        });
    }

    // Reconnect watcher with exponential backoff + wall-clock drift detection.
    // Uses dual select: periodic poll OR instant notification from wait_disconnect().
    // Wall-clock drift detects system sleep/wake (tokio timers freeze during sleep).
    if cfg.is_neboai_enabled() {
        let reconnect_state = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let mut backoff_secs: u64 = 30;
            loop {
                let before_sleep = std::time::SystemTime::now();

                tokio::select! {
                    // Branch 1: periodic backoff poll
                    _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
                    // Branch 2: instant notification when read loop exits unexpectedly
                    _ = reconnect_state.comm_manager.wait_disconnect() => {
                        info!("neboai: disconnect notification received, will reconnect");
                    }
                }

                // Detect wall-clock drift — if elapsed >> expected, system was asleep
                let elapsed_wall = std::time::SystemTime::now()
                    .duration_since(before_sleep)
                    .unwrap_or_default();
                let expected = std::time::Duration::from_secs(backoff_secs);
                let drift = elapsed_wall.saturating_sub(expected);
                let was_asleep = drift > std::time::Duration::from_secs(10);

                if was_asleep {
                    info!(
                        drift_secs = drift.as_secs(),
                        "neboai: detected system sleep, forcing reconnect"
                    );
                    // Tear down stale connection (read/write loops may still be blocked)
                    reconnect_state.comm_manager.shutdown().await;
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                } else if reconnect_state.comm_manager.is_connected().await {
                    backoff_secs = 30;
                    continue;
                }

                match codes::activate_neboai(&reconnect_state).await {
                    Ok(()) => {
                        info!("neboai: reconnected to gateway");
                        // Persist rotated JWT so next reconnect uses the fresh token
                        if let Some(new_token) =
                            reconnect_state.comm_manager.take_rotated_token().await
                        {
                            if let Err(e) = reconnect_state
                                .store
                                .update_auth_profile_token_by_provider("neboai", &new_token)
                            {
                                warn!("neboai: failed to persist rotated token: {}", e);
                            }
                        }
                        backoff_secs = 30;
                    }
                    Err(_) => {
                        backoff_secs = (backoff_secs * 2).min(600);
                    }
                }
            }
        });
    }

    // Management-tunnel watcher — dials the hub and keeps the tunnel alive
    // (docs/plans/nebo-cloud-architecture.md, Plane B). Every bot — desktop
    // and cloud — is reached through its outbound tunnel, so this runs
    // whenever NeboAI is enabled; until credentials exist it just idles.
    if cfg.is_neboai_enabled() && !cfg.neboai.tunnel_url.is_empty() {
        let tunnel_state = state.clone();
        let local_addr = format!("127.0.0.1:{}", cfg.port);
        tokio::spawn(async move {
            let mut backoff_secs: u64 = 30;
            loop {
                let Some(token) = codes::neboai_token(&tunnel_state) else {
                    // Not activated yet — poll until credentials appear.
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    continue;
                };
                let started = std::time::Instant::now();
                let hub_url = tunnel_state.config.neboai.tunnel_url.clone();
                match comm::tunnel::run(&hub_url, &token, &local_addr).await {
                    Ok(()) => info!("tunnel: closed by hub, redialing"),
                    Err(e) => info!("tunnel: {e}"),
                }
                // A connection that lived a while earns a quick redial;
                // repeated fast failures back off like the comms watcher.
                let delay = if started.elapsed() > std::time::Duration::from_secs(60) {
                    backoff_secs = 30;
                    5
                } else {
                    backoff_secs = (backoff_secs * 2).min(600);
                    backoff_secs
                };
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            }
        });
    }

    // Spawn background update checker (skip in debug/dev builds)
    if cfg!(debug_assertions) {
        tracing::debug!("skipping background update checker in dev build");
    } else {
        let update_hub = state.hub.clone();
        let download_hub = state.hub.clone();
        let update_store = state.store.clone();
        let update_pending = state.update_pending.clone();
        tokio::spawn(async move {
            let checker = updater::BackgroundChecker::new(
                VERSION.to_string(),
                std::time::Duration::from_secs(3600),
                move |result| {
                    // Check user preference before auto-downloading
                    let auto_update_enabled = update_store
                        .get_settings()
                        .ok()
                        .flatten()
                        .map(|s| s.auto_update != 0)
                        .unwrap_or(true);

                    update_hub.broadcast(
                        "update_available",
                        serde_json::json!({
                            "latestVersion": result.latest_version,
                            "currentVersion": result.current_version,
                            "installMethod": result.install_method,
                            "canAutoUpdate": result.can_auto_update,
                            "autoUpdateEnabled": auto_update_enabled,
                        }),
                    );

                    // Auto-download for direct installs only when preference is ON
                    if result.can_auto_update && auto_update_enabled {
                        let tag = result.latest_version.clone();
                        let hub = download_hub.clone();
                        let progress_hub = download_hub.clone();
                        let pending = update_pending.clone();
                        tokio::spawn(async move {
                            let progress_fn: updater::ProgressFn =
                                Box::new(move |downloaded, total| {
                                    let percent = if total > 0 {
                                        (downloaded * 100) / total
                                    } else {
                                        0
                                    };
                                    progress_hub.broadcast(
                                        "update_progress",
                                        serde_json::json!({
                                            "downloaded": downloaded,
                                            "total": total,
                                            "percent": percent,
                                        }),
                                    );
                                });
                            match updater::download(&tag, Some(progress_fn)).await {
                                Ok(path) => {
                                    // Verify checksum before staging
                                    match updater::verify_checksum(&path, &tag).await {
                                        Ok(()) => {
                                            pending.lock().await.replace((path, tag.clone()));
                                            hub.broadcast(
                                                "update_ready",
                                                serde_json::json!({ "version": tag }),
                                            );
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                "update checksum verification failed: {}",
                                                e
                                            );
                                            let _ = std::fs::remove_file(&path);
                                            hub.broadcast(
                                                "update_error",
                                                serde_json::json!({ "error": e.to_string() }),
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    hub.broadcast(
                                        "update_error",
                                        serde_json::json!({ "error": e.to_string() }),
                                    );
                                }
                            }
                        });
                    }
                },
            );
            let cancel = tokio_util::sync::CancellationToken::new();
            checker.run(cancel).await;
        });
    } // end if !debug_assertions

    // Spawn cron scheduler. Pass the channel-bridge registry so jobs that
    // captured their originating channel context can route the response back
    // via the bridge when they fire (e.g. "set 1-min timer" from Slack →
    // alert lands in the same Slack thread).
    scheduler::spawn(
        state.store.clone(),
        state.runner.clone(),
        state.hub.clone(),
        state.snapshot_store.clone(),
        state.workflow_manager.clone(),
        state.run_registry.clone(),
        state.clone(),
    );

    // Spawn heartbeat scheduler for per-entity heartbeats
    heartbeat::spawn(state.clone());

    // Spawn marketplace artifact update checker (6h default, staggered API calls)
    artifact_updates::spawn(state.clone());

    // Spawn periodic agent_progress broadcaster — broadcasts active run snapshots
    // to all connected clients every 5 seconds so the frontend stays in sync.
    {
        let hub = state.hub.clone();
        let registry = state.run_registry.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                interval.tick().await;
                let runs = registry.list_top_level().await;
                if !runs.is_empty() {
                    hub.broadcast("agent_progress", serde_json::json!({ "runs": runs }));
                }
            }
        });
    }

    // Build router
    // WebSocket routes are kept outside CompressionLayer — compression corrupts
    // the upgraded socket since it wraps the response body stream.
    let http_routes = Router::new()
        .route("/health", axum::routing::get(health_handler))
        .route("/server.json", axum::routing::get(spa::server_json))
        // MCP endpoint for CLI providers (Claude Code, Codex, Gemini)
        .route(
            "/agent/mcp",
            axum::routing::post(handlers::mcp_server::agent_mcp_handler)
                .layer(axum::middleware::from_fn(middleware::mcp_api_key_auth)),
        )
        // NeboAI OAuth callback — top-level because the browser navigates here directly
        .route(
            "/auth/neboai/callback",
            axum::routing::get(handlers::neboai::oauth_callback),
        )
        .nest(
            "/api/v1",
            routes::api_routes(jwt_secret)
                .layer(axum::middleware::from_fn(middleware::api_security_headers)),
        )
        .fallback(spa::spa_handler)
        .layer(CompressionLayer::new());

    let app = Router::new()
        .route("/ws", axum::routing::get(handlers::ws::client_ws_handler))
        .route("/ws/app/{agent_id}", axum::routing::get(handlers::ws::app_ws_handler))
        .route("/ws/extension", axum::routing::get(handlers::ws::extension_ws_handler))
        // [VOICE DISABLED] .route("/ws/voice/dictation", axum::routing::get(handlers::voice::dictation_ws_handler))
        // [VOICE DISABLED] .route("/ws/voice/conversation", axum::routing::get(handlers::voice::conversation_ws_handler))
        .route("/apps/{agent_id}/ui/{*path}", axum::routing::get(handlers::apps::serve_app_ui))
        .route("/sdk/nebo.global.js", axum::routing::get(handlers::apps::serve_sdk_iife))
        .merge(http_routes)
        .layer(axum::middleware::from_fn(middleware::security_headers))
        .layer(cors_layer())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    tracing::info_span!("http", method = %request.method(), uri = %request.uri())
                })
                .on_failure(|error: tower_http::classify::ServerErrorsFailureClass, latency: std::time::Duration, _span: &tracing::Span| {
                    tracing::error!(%error, latency_ms = latency.as_millis(), "request failed");
                })
        )
        .with_state(state.clone());

    // Clone comm_manager for the shutdown handler — needs to disconnect NeboAI
    // before the process exits so the gateway sees a clean WebSocket Close frame.
    let shutdown_comm = state.comm_manager.clone();
    let shutdown_lifecycles = state.app_lifecycles.clone();

    if !quiet {
        info!("Server ready at http://localhost:{port}");
    }

    // Block non-loopback binding unless explicitly opted in
    if host != "127.0.0.1" && host != "localhost" && host != "::1" {
        if std::env::var("NEBO_ALLOW_REMOTE").as_deref() != Ok("true") {
            return Err(NeboError::Server(format!(
                "Refusing to bind to {bind_addr} — Nebo is designed for localhost-only access. \
                 Set NEBO_ALLOW_REMOTE=true to override."
            )));
        }
        eprintln!("WARNING: Server binding to {bind_addr} — remote access enabled");
        if std::env::var("NEBO_MCP_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .is_none()
        {
            eprintln!(
                "WARNING: MCP endpoint is UNAUTHENTICATED. Set NEBO_MCP_API_KEY to secure it."
            );
        }
    }

    // Preconnect to AI provider to warm TCP+TLS (saves ~200ms on first call)
    {
        let api_url = cfg.neboai.janus_url.clone();
        if !api_url.is_empty() {
            tokio::spawn(async move {
                let client = reqwest::Client::new();
                let _ = client.head(&api_url).send().await;
            });
        }
    }

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| NeboError::Server(format!("failed to bind: {e}")))?;

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            info!("shutdown signal received, draining in-flight extractions...");
            agent::memory_flush::drain_extractions().await;
            info!("extractions drained, stopping app sidecars...");
            {
                let mut lifecycles = shutdown_lifecycles.write().await;
                for (id, lifecycle) in lifecycles.iter_mut() {
                    if let Err(e) = lifecycle.shutdown().await {
                        warn!(agent = %id, error = %e, "failed to stop sidecar on shutdown");
                    }
                }
                lifecycles.clear();
            }
            info!("app sidecars stopped, disconnecting comm plugins...");
            shutdown_comm.shutdown().await;
            // Brief pause for write_loop to send the WebSocket Close frame
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            info!("comm plugins disconnected");
        })
        .await
        .map_err(|e| NeboError::Server(format!("server error: {e}")))?;

    Ok(())
}

/// Wait for a shutdown signal (SIGTERM on Unix, Ctrl+C everywhere).
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");

        tokio::select! {
            _ = ctrl_c => { info!("received Ctrl+C"); }
            _ = sigterm.recv() => { info!("received SIGTERM"); }
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
        info!("received Ctrl+C");
    }
}

/// Process filesystem agent change events: sync DB, update registry, broadcast WS.
async fn handle_agent_fs_events(
    state: AppState,
    mut rx: tokio::sync::mpsc::Receiver<napp::AgentFsEvent>,
) {
    while let Some(event) = rx.recv().await {
        match event {
            napp::AgentFsEvent::Added(loaded) => {
                // Look up DB by manifest ID first, then by name
                let db_agent = loaded
                    .id
                    .as_deref()
                    .and_then(|id| state.store.get_agent(id).ok().flatten())
                    .or_else(|| {
                        state
                            .store
                            .get_agent_by_name(&loaded.agent_def.name)
                            .ok()
                            .flatten()
                    });

                let final_id = if let Some(ref existing) = db_agent {
                    // Update existing record with fresh filesystem content
                    let _ = state.store.update_agent(
                        &existing.id,
                        &loaded.agent_def.name,
                        &loaded.description,
                        &loaded.agent_md,
                        &loaded.frontmatter,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                    );
                    existing.id.clone()
                } else {
                    // Create new DB record
                    let agent_id = loaded
                        .id
                        .clone()
                        .unwrap_or_else(|| loaded.agent_def.name.clone());
                    let kind = match loaded.source {
                        napp::AgentSource::Installed => Some("installed"),
                        napp::AgentSource::User => Some("user"),
                    };
                    match state.store.create_agent(
                        &agent_id,
                        kind,
                        &loaded.agent_def.name,
                        &loaded.description,
                        &loaded.agent_md,
                        &loaded.frontmatter,
                        None,
                        None,
                    ) {
                        Ok(_) => {
                            // Implicit reconcile cascade for an agent newly discovered
                            // on disk — gated by `auto_install_deps` (default OFF).
                            if !loaded.frontmatter.is_empty()
                                && crate::deps::auto_install_deps_enabled(&state)
                            {
                                let cascade_state = state.clone();
                                let fm = loaded.frontmatter.clone();
                                tokio::spawn(async move {
                                    let deps =
                                        crate::deps::extract_agent_deps_from_frontmatter(&fm);
                                    if !deps.is_empty() {
                                        let mut visited = std::collections::HashSet::new();
                                        crate::deps::resolve_cascade(
                                            &cascade_state,
                                            deps,
                                            &mut visited,
                                        )
                                        .await;
                                    }
                                });
                            }
                            agent_id
                        }
                        Err(e) => {
                            warn!(name = %loaded.agent_def.name, error = %e,
                                  "fs watcher: failed to create agent in DB");
                            continue;
                        }
                    }
                };

                // Sync app fields (ui path, binary path, window config) to DB
                if loaded.is_app {
                    let _ = state.store.set_agent_app_fields(
                        &final_id,
                        true,
                        loaded.app_ui_path.as_ref().and_then(|p| p.to_str()),
                        loaded.app_binary_path.as_ref().and_then(|p| p.to_str()),
                        loaded
                            .app_window_config
                            .as_ref()
                            .and_then(|wc| serde_json::to_string(wc).ok())
                            .as_deref(),
                    );
                }

                // Sync workflow bindings
                if let Some(ref config) = loaded.config {
                    sync_agent_workflows(&state.store, &final_id, config);
                }

                // If agent was previously enabled, restore to registry + start worker
                if let Ok(Some(db)) = state.store.get_agent(&final_id) {
                    if db.is_enabled != 0 {
                        let config = if !db.frontmatter.is_empty() {
                            napp::agent::parse_agent_config(&db.frontmatter).ok()
                        } else {
                            None
                        };
                        state.agent_registry.write().await.insert(
                            final_id.clone(),
                            tools::ActiveAgent {
                                agent_id: final_id.clone(),
                                name: db.name.clone(),
                                agent_md: db.agent_md.clone(),
                                config,
                                channel_id: None,
                                degraded: None,
                                soul: db.soul.clone(),
                                rules: db.rules.clone(),
                            },
                        );
                        state.agent_workers.start_agent(&final_id, &db.name, None).await;
                    }
                }

                info!(name = %loaded.agent_def.name, id = %final_id, "fs watcher: agent added");
                state.hub.broadcast(
                    "agent_installed",
                    serde_json::json!({ "agentId": final_id, "name": loaded.agent_def.name }),
                );
            }

            napp::AgentFsEvent::Changed(loaded) => {
                // Find DB record
                let db_agent = loaded
                    .id
                    .as_deref()
                    .and_then(|id| state.store.get_agent(id).ok().flatten())
                    .or_else(|| {
                        state
                            .store
                            .get_agent_by_name(&loaded.agent_def.name)
                            .ok()
                            .flatten()
                    });

                let Some(db_agent) = db_agent else {
                    warn!(name = %loaded.agent_def.name, "fs watcher: changed agent not in DB, skipping");
                    continue;
                };

                // Refresh filesystem-owned content.
                let _ = state.store.sync_agent_content(
                    &db_agent.id,
                    &loaded.agent_md,
                    &loaded.frontmatter,
                );
                // Sync display name/description from manifest.
                let _ = state.store.sync_agent_identity(
                    &db_agent.id,
                    &loaded.agent_def.name,
                    &loaded.description,
                );

                // Sync app fields on change (manifest may have flipped artifact_type)
                if loaded.is_app {
                    let _ = state.store.set_agent_app_fields(
                        &db_agent.id,
                        true,
                        loaded.app_ui_path.as_ref().and_then(|p| p.to_str()),
                        loaded.app_binary_path.as_ref().and_then(|p| p.to_str()),
                        loaded
                            .app_window_config
                            .as_ref()
                            .and_then(|wc| serde_json::to_string(wc).ok())
                            .as_deref(),
                    );
                }

                // Re-sync workflow bindings
                if let Some(ref config) = loaded.config {
                    sync_agent_workflows(&state.store, &db_agent.id, config);
                }

                // Patch in-memory registry content only; identity stays DB-owned.
                {
                    let mut registry = state.agent_registry.write().await;
                    if let Some(active) = registry.get_mut(&db_agent.id) {
                        active.agent_md = loaded.agent_md.clone();
                        active.config = loaded.config.clone();
                    }
                }

                // Restart worker if running (picks up new triggers)
                if db_agent.is_enabled != 0 {
                    state.agent_workers.stop_agent(&db_agent.id).await;
                    state
                        .agent_workers
                        .start_agent(&db_agent.id, &db_agent.name, None)
                        .await;
                }

                info!(name = %db_agent.name, id = %db_agent.id, "fs watcher: agent content updated");
                state.hub.broadcast(
                    "agent_updated",
                    serde_json::json!({
                        "agentId": db_agent.id,
                        "name": db_agent.name,
                        "description": db_agent.description,
                    }),
                );
            }

            napp::AgentFsEvent::Removed { name_key: _, agent } => {
                // Find DB record
                let db_agent = agent
                    .id
                    .as_deref()
                    .and_then(|id| state.store.get_agent(id).ok().flatten())
                    .or_else(|| {
                        state
                            .store
                            .get_agent_by_name(&agent.agent_def.name)
                            .ok()
                            .flatten()
                    });

                let Some(db_agent) = db_agent else {
                    info!(name = %agent.agent_def.name, "fs watcher: removed agent not in DB, nothing to do");
                    continue;
                };

                // Soft-deactivate (do NOT delete — user may re-add directory)
                let _ = state.store.set_agent_enabled(&db_agent.id, false);

                // Stop worker and remove from registry
                state.agent_workers.stop_agent(&db_agent.id).await;
                state.agent_registry.write().await.remove(&db_agent.id);

                info!(name = %agent.agent_def.name, id = %db_agent.id, "fs watcher: agent removed from filesystem");
                state.hub.broadcast(
                    "agent_deactivated",
                    serde_json::json!({ "agentId": db_agent.id, "name": agent.agent_def.name }),
                );
            }
        }
    }

    warn!("agent filesystem event channel closed");
}

/// Sync workflow bindings from an AgentConfig into the agent_workflows table.
fn sync_agent_workflows(store: &db::Store, agent_id: &str, config: &napp::agent::AgentConfig) {
    for (binding_name, binding) in &config.workflows {
        let (trigger_type, trigger_config) = match &binding.trigger {
            napp::agent::AgentTrigger::Schedule { cron, .. } => {
                ("schedule", tools::PersonaTool::normalize_cron(cron))
            }
            napp::agent::AgentTrigger::Heartbeat { interval, window } => {
                let cfg = match window {
                    Some(w) => format!("{}|{}", interval, w),
                    None => interval.clone(),
                };
                ("heartbeat", cfg)
            }
            napp::agent::AgentTrigger::Event { sources } => ("event", sources.join(",")),
            napp::agent::AgentTrigger::Watch {
                plugin,
                command,
                event,
                restart_delay_secs,
            } => {
                let mut cfg = serde_json::json!({
                    "plugin": plugin,
                    "command": command,
                    "restart_delay_secs": restart_delay_secs
                });
                if let Some(ev) = event {
                    cfg["event"] = serde_json::json!(ev);
                }
                ("watch", cfg.to_string())
            }
            napp::agent::AgentTrigger::Folder {
                path,
                extensions,
                recursive,
                debounce_secs,
            } => {
                let cfg = serde_json::json!({
                    "path": path,
                    "extensions": extensions,
                    "recursive": recursive,
                    "debounce_secs": debounce_secs
                });
                ("folder", cfg.to_string())
            }
            napp::agent::AgentTrigger::Manual => ("manual", String::new()),
        };
        let inputs_json = if binding.inputs.is_empty() {
            None
        } else {
            serde_json::to_string(&binding.inputs).ok()
        };
        let desc = if binding.description.is_empty() {
            None
        } else {
            Some(binding.description.as_str())
        };
        let activities_json = if binding.activities.is_empty() {
            None
        } else {
            serde_json::to_string(&binding.activities).ok()
        };
        let connections_json = if binding.connections.is_empty() {
            None
        } else {
            serde_json::to_string(&binding.connections).ok()
        };
        let _ = store.upsert_agent_workflow(
            agent_id,
            binding_name,
            trigger_type,
            &trigger_config,
            desc,
            inputs_json.as_deref(),
            binding.emit.as_deref(),
            activities_json.as_deref(),
            connections_json.as_deref(),
        );
    }
}

/// Handle an incoming NeboAI message with full access to runner/lanes/comm.
/// Control handling for inbound comm messages, before any run starts.
/// Returns true when the message was consumed:
/// - metadata kind=stop → cancel the conversation's running session (the
///   loop UI's Stop button — desktop parity with the local stop event).
/// - a pending agent question for this session → the message IS the answer;
///   resolve the blocked ask instead of starting a new run.
async fn try_handle_comm_control(
    state: &AppState,
    session_key: &str,
    answer: &str,
    metadata: &std::collections::HashMap<String, String>,
) -> bool {
    if metadata.get("kind").map(String::as_str) == Some("stop") {
        let cancelled = state.run_registry.cancel_by_session(session_key).await;
        tracing::info!(session = %session_key, cancelled, "inbound comm stop command");
        return true;
    }
    let pending = state.pending_comm_asks.lock().await.remove(session_key);
    if let Some(request_id) = pending {
        if let Some(tx) = state.ask_channels.lock().await.remove(&request_id) {
            let _ = tx.send(answer.to_string());
            tracing::info!(session = %session_key, "inbound comm message resolved pending ask");
            return true;
        }
        // Asker already gone (timeout/cancel) — treat as a normal message.
    }
    false
}

/// Origin for an inbound NeboLoop message. The owner operating their own machine
/// from the mobile app (their personal loop — `is_personal`) is a present,
/// interactive session over a different transport, so it gets full capability
/// like the desktop (`Origin::User`): no shell-deny, interactive prompt style,
/// and the ask tool available (the loop already forwards AskRequest to the
/// conversation). Shared-loop / third-party traffic stays `Origin::Comm`
/// (autonomous, shell-restricted). See docs/plans/owner-full-access-from-comm.md.
fn comm_origin(is_personal: bool) -> tools::Origin {
    if is_personal {
        tools::Origin::User
    } else {
        tools::Origin::Comm
    }
}

async fn handle_comm_message(state: AppState, msg: comm::CommMessage) {
    tracing::info!(
        target: "neboai_identity",
        topic = %msg.topic,
        from = %msg.from,
        conv_id = %msg.conversation_id,
        content_len = msg.content.len(),
        meta_agent_slug = ?msg.metadata.get("agent_slug"),
        meta_agent_id = ?msg.metadata.get("agent_id"),
        human_injected = msg.human_injected,
        "INBOUND handle_comm_message — what the loop says about the sender"
    );

    // Route account stream messages (plan changes, token refresh)
    if msg.topic == "account" {
        if let Ok(event) = serde_json::from_str::<serde_json::Value>(&msg.content) {
            if event.get("type").and_then(|t| t.as_str()) == Some("tokenRefresh") {
                if let Some(token) = event.get("token").and_then(|t| t.as_str()) {
                    let plan = event.get("plan").and_then(|p| p.as_str()).unwrap_or("free");
                    tracing::info!(plan = plan, "Account: plan updated via tokenRefresh");

                    // Persist fresh JWT to SQLite auth_profiles — next Janus request uses it
                    if let Ok(profiles) = state
                        .store
                        .list_all_active_auth_profiles_by_provider("neboai")
                    {
                        if let Some(profile) = profiles.first() {
                            let _ = state.store.update_auth_profile(
                                &profile.id,
                                &profile.name,
                                token,
                                profile.model.as_deref(),
                                profile.base_url.as_deref(),
                                profile.priority.unwrap_or(0),
                                profile.auth_type.as_deref(),
                                profile.metadata.as_deref(),
                            );
                        }
                    }

                    // Update in-memory plan tier so account_status reads the fresh value
                    *state.plan_tier.write().await = plan.to_string();

                    // Notify UI
                    state
                        .hub
                        .broadcast("plan_changed", serde_json::json!({"plan": plan}));
                }
            }
        }
        return;
    }

    // Route install events to napp registry
    if msg.topic == "installs" {
        if let Ok(event) = serde_json::from_str::<napp::InstallEvent>(&msg.content) {
            let reg = state.napp_registry.clone();
            let hub = state.hub.clone();
            match reg.handle_install_event(event).await {
                Ok(()) => hub.broadcast("tool_event", serde_json::json!({"status": "ok"})),
                Err(e) => {
                    tracing::warn!("install event handling failed: {}", e);
                    hub.broadcast("tool_error", serde_json::json!({"error": e.to_string()}));
                }
            }
            return;
        }
    }

    // Skip echoed messages: when we forward a local user prompt to NeboAI
    // (human_injected=true), the gateway may echo it back — don't re-process.
    if msg.human_injected {
        tracing::debug!(
            topic = %msg.topic,
            conv_id = %msg.conversation_id,
            "skipping echoed human_injected message"
        );
        return;
    }

    // Skip self-echo: NeboAI deliveries always set human_injected=false,
    // but the sender_id (msg.from) matches our bot_id when we sent the message.
    // Without this, a local user prompt forwarded to NeboAI comes back as a
    // new delivery and triggers a duplicate agent run on the same session.
    if !msg.from.is_empty() {
        if let Some(bot_id) = config::read_bot_id() {
            if msg.from == bot_id {
                tracing::debug!(
                    topic = %msg.topic,
                    conv_id = %msg.conversation_id,
                    sender = %msg.from,
                    "skipping self-echo (sender_id matches bot_id)"
                );
                return;
            }
        }
    }

    // Route agent space messages to the correct role
    if msg.topic == "agent_space" {
        let text = extract_message_text(&msg.content);
        if text.is_empty() {
            tracing::warn!(conv_id = %msg.conversation_id, "agent_space message with empty text, skipping");
            return;
        }

        let agent_slug = msg.metadata.get("agent_slug").cloned().unwrap_or_default();
        // Resolve to a stable local agent id. Never drops: bot_* handles and
        // unresolved slugs both fall back to the primary bot.
        let (agent_id, is_default_bot) =
            resolve_inbound_agent(&state, &agent_slug, &msg.conversation_id, &msg.metadata).await;

        // Check if this is the owner's personal loop → unify session with local agent chat
        let space_loop_id = state
            .comm_manager
            .agent_space_loop_id(&msg.conversation_id)
            .await;
        let personal_id = state.personal_loop_id.read().await.clone();
        let is_personal = if is_default_bot {
            // Default bot is always personal
            space_loop_id.is_some() && (personal_id.is_none() || space_loop_id == personal_id)
        } else {
            space_loop_id.is_some() && space_loop_id == personal_id
        };
        tracing::info!(
            agent_slug = %agent_slug,
            agent_id = %agent_id,
            text_len = text.len(),
            is_personal = is_personal,
            space_loop_id = ?space_loop_id,
            personal_loop_id = ?personal_id,
            "agent_space: routing to role"
        );

        let session_key = if is_personal && is_default_bot {
            // Default bot: use the companion chat's actual session key
            resolve_companion_session_key(&state)
        } else if is_personal {
            // Custom agent: use agent-scoped session key (matches frontend's agent:{id}:web)
            agent::keyparser::build_agent_session_key(&agent_id, "web")
        } else {
            // External loop: separate session
            agent::keyparser::build_session_key(
                "neboai",
                "agent_space",
                &format!("{}:{}", agent_slug, msg.conversation_id),
            )
        };

        if handle_comm_slash_command(
            &state,
            &text,
            &session_key,
            "agent_space",
            &msg.conversation_id,
        )
        .await
        .is_some()
        {
            return;
        }

        // Pre-create chat with friendly title (agent name, not raw session key)
        let agent_name = if is_default_bot {
            "Nebo".to_string()
        } else {
            let registry = state.agent_registry.read().await;
            registry
                .get(&agent_id)
                .map(|r| r.name.clone())
                // Exposed-but-not-enabled agents aren't in the registry — use the
                // stored display name, never the raw slug.
                .or_else(|| state.store.get_agent(&agent_id).ok().flatten().map(|a| a.name))
                .unwrap_or_else(|| agent_slug.clone())
        };
        if !is_default_bot {
            let _ = state
                .store
                .create_chat(&session_key, &format!("Agent: {}", agent_name));
        }

        let preview = if text.len() > 80 {
            format!("{}...", truncate_str(&text, 80))
        } else {
            text.clone()
        };
        notify_crate::send(&format!("Agent space: {}", agent_name), &preview);

        // Broadcast inbound user message to local frontend for real-time display
        if is_personal {
            state.hub.broadcast(
                "chat_inbound",
                serde_json::json!({
                    "session_id": session_key,
                    "content": text,
                    "agentId": agent_id,
                    "source": "neboai",
                }),
            );
        }

        // A resolved agent always uses ITS persona/config — otherwise a DM to a
        // secondary agent answers with the primary "Nebo" persona. (Mirrors the
        // channel branch; the is_personal gate dropped the agent config when the
        // agent_space loop_id didn't match the personal loop.)
        let entity_config = if !agent_id.is_empty() {
            entity_config::resolve_for_chat(&state.store, "agent", &agent_id)
        } else if is_personal {
            entity_config::resolve_for_chat(&state.store, "main", "main")
        } else {
            entity_config::resolve_for_chat(&state.store, "channel", "agent_space")
        };

        let mut prompt = text;
        let images = process_comm_attachments(&state, &msg.attachments, &mut prompt).await;

        if try_handle_comm_control(&state, &session_key, &prompt, &msg.metadata).await {
            return;
        }

        // Webhook-originated events always run shell-restricted (Origin::Comm),
        // even in the personal loop: the nbwh_ API key lives in external
        // systems, so it must never confer owner-level (shell) privileges.
        // See neboloop docs/PRD_WEBHOOKS.md §10.
        let is_webhook = serde_json::from_str::<serde_json::Value>(&msg.content)
            .map(|v| {
                v.get("platformData")
                    .and_then(|p| p.get("channel"))
                    .and_then(|c| c.as_str())
                    == Some("webhook")
            })
            .unwrap_or(false);

        let config = chat_dispatch::ChatConfig {
            session_key,
            prompt,
            system: String::new(),
            user_id: String::new(),
            channel: "neboai".to_string(),
            origin: comm_origin(is_personal && !is_webhook),
            agent_id,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            lane: types::constants::lanes::COMM.to_string(),
            comm_reply: Some(chat_dispatch::CommReplyConfig {
                provider: "neboai".to_string(),
                topic: "agent_space".to_string(),
                conversation_id: msg.conversation_id.clone(),
            }),
            entity_config,
            images,
            entity_name: agent_name.clone(),
            origin_agent_id: None,
            mention_context: None,
            tool_scope: None, plan_mode: false,
            channel_ctx: None,
        };

        chat_dispatch::run_chat(&state, config).await;

        state.event_bus.emit(tools::events::Event {
            source: format!("neboai.agent_space.{}", agent_slug),
            payload: serde_json::json!({
                "from": msg.from,
                "content": msg.content,
                "conversation_id": msg.conversation_id,
                "agent_slug": agent_slug,
            }),
            origin: "neboai".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
        return;
    }

    // Route embed conversations — a publisher product (e.g. Monument) embeds a
    // chat widget where the user talks to this bot. `context` in the content is
    // host-page-asserted (advisory); `embed` is stamped by the NeboLoop gateway
    // (trusted app identity). Replies go back on the same conversation.
    if msg.topic == "embed" {
        let text = extract_message_text(&msg.content);
        if text.is_empty() {
            tracing::warn!(conv_id = %msg.conversation_id, "embed message with empty text, skipping");
            return;
        }

        // Embed conversations belong to the BOT — default to the primary agent,
        // but honor an explicit `<@id>` mention chip (like the channel branch).
        let bot_id = config::read_bot_id().unwrap_or_default();
        let mut agent_id = String::new();
        for cap in MENTION_TOKEN_RE.captures_iter(&text) {
            let id = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            if !bot_id.is_empty() && id == bot_id {
                break; // primary bot — keep agent_id empty
            }
            if let Ok(Some(a)) = state.store.get_agent_by_loop_agent_id(id) {
                if a.loop_exposed != 0 {
                    agent_id = a.id;
                    break;
                }
            }
        }

        let session_key =
            agent::keyparser::build_session_key("neboai", "embed", &msg.conversation_id);

        if handle_comm_slash_command(&state, &text, &session_key, "embed", &msg.conversation_id)
            .await
            .is_some()
        {
            return;
        }

        // Parse the full content JSON for the host-page context (advisory) and
        // the gateway-stamped app identity (trusted).
        let content_json: serde_json::Value =
            serde_json::from_str(&msg.content).unwrap_or(serde_json::Value::Null);
        let ctx = content_json.get("context").filter(|v| v.is_object());
        let embed_info = content_json.get("embed").filter(|v| v.is_object());
        let mention_context = build_embed_context(ctx, embed_info);

        let app_label = ctx
            .and_then(|c| c.get("app"))
            .and_then(|v| v.as_str())
            .or_else(|| embed_info.and_then(|e| e.get("app")).and_then(|v| v.as_str()))
            .unwrap_or("embed")
            .to_string();

        tracing::info!(
            conv_id = %msg.conversation_id,
            app = %app_label,
            agent_id = %agent_id,
            text_len = text.len(),
            has_context = mention_context.is_some(),
            "embed: routing to bot"
        );

        let _ = state
            .store
            .create_chat(&session_key, &format!("Embed: {}", app_label));

        let preview = if text.len() > 80 {
            format!("{}...", truncate_str(&text, 80))
        } else {
            text.clone()
        };
        notify_crate::send(&format!("Embed chat: {}", app_label), &preview);

        // A mentioned agent uses ITS persona/config; otherwise the bot's main
        // persona (embed conversations belong to the bot, not a specific agent).
        let entity_config = if !agent_id.is_empty() {
            entity_config::resolve_for_chat(&state.store, "agent", &agent_id)
        } else {
            entity_config::resolve_for_chat(&state.store, "main", "main")
        };

        let mut prompt = text;
        let images = process_comm_attachments(&state, &msg.attachments, &mut prompt).await;

        if try_handle_comm_control(&state, &session_key, &prompt, &msg.metadata).await {
            return;
        }

        let config = chat_dispatch::ChatConfig {
            session_key,
            prompt,
            system: String::new(),
            user_id: String::new(),
            channel: "neboai".to_string(),
            origin: tools::Origin::Comm,
            agent_id,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            lane: types::constants::lanes::COMM.to_string(),
            comm_reply: Some(chat_dispatch::CommReplyConfig {
                provider: "neboai".to_string(),
                topic: "embed".to_string(),
                conversation_id: msg.conversation_id.clone(),
            }),
            entity_config,
            images,
            entity_name: String::new(),
            origin_agent_id: None,
            mention_context,
            tool_scope: None,
            plan_mode: false,
            channel_ctx: None,
        };

        chat_dispatch::run_chat(&state, config).await;

        state.event_bus.emit(tools::events::Event {
            source: "neboai.embed".to_string(),
            payload: serde_json::json!({
                "from": msg.from,
                "content": msg.content,
                "conversation_id": msg.conversation_id,
            }),
            origin: "neboai".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
        return;
    }

    // Route chat and DM messages to the agent runner via unified chat pipeline
    if msg.topic == "chat" || msg.topic == "dm" {
        // Check if this conversation is actually an agent_space (gateway sends
        // stream=dm for these). ConvMaps is in-memory and rebuilt from JOIN
        // updates after every reconnect, so fall back to the persisted
        // agents.loop_conv_id mapping when it has no entry yet — otherwise a DM
        // arriving in that window forks into a new `neboai:dm:` conversation.
        let convmap_slug = state
            .comm_manager
            .agent_slug_for_conv(&msg.conversation_id)
            .await;
        let durable_agent_id = if convmap_slug.is_none() {
            state
                .store
                .get_agent_id_by_loop_conv_id(&msg.conversation_id)
                .ok()
                .flatten()
        } else {
            None
        };
        if convmap_slug.is_some() || durable_agent_id.is_some() {
            let text = extract_message_text(&msg.content);
            // Control frames (e.g. the loop Stop button) are intentionally empty
            // and carry a metadata `kind` — do NOT drop them here. Dropping the
            // empty stop frame before try_handle_comm_control (below) is why Stop
            // did nothing. Only genuinely empty chat messages are discarded.
            let is_control = msg.metadata.get("kind").map(String::as_str) == Some("stop");
            if text.is_empty() && !is_control {
                return;
            }
            // Resolve to a stable local agent id. Never drops: bot_* handles and
            // unresolved slugs both fall back to the primary bot.
            let (agent_id, is_default_bot) = if let Some(ref slug) = convmap_slug {
                resolve_inbound_agent(&state, slug, &msg.conversation_id, &msg.metadata).await
            } else {
                let aid = durable_agent_id.clone().unwrap_or_default();
                if aid == "assistant" {
                    (String::new(), true)
                } else {
                    (aid, false)
                }
            };

            // Write through the conv↔agent association so the durable fallback
            // can resolve this conversation after a restart.
            if convmap_slug.is_some() {
                let row_id = if is_default_bot { "assistant" } else { agent_id.as_str() };
                if let Err(e) = state
                    .store
                    .set_agent_loop_conv_id(row_id, &msg.conversation_id)
                {
                    tracing::warn!(error = %e, conv_id = %msg.conversation_id, "failed to persist loop_conv_id");
                }
            }

            // Check if this is the owner's personal loop → unify session with
            // local agent chat. A durable-mapping hit is personal by
            // construction: loop_conv_id is only written for conversations of
            // agents registered on the owner's personal loop.
            let space_loop_id = state
                .comm_manager
                .agent_space_loop_id(&msg.conversation_id)
                .await;
            let personal_id = state.personal_loop_id.read().await.clone();
            let is_personal = if durable_agent_id.is_some() {
                true
            } else if is_default_bot {
                space_loop_id.is_some() && (personal_id.is_none() || space_loop_id == personal_id)
            } else {
                space_loop_id.is_some() && space_loop_id == personal_id
            };
            let agent_slug = convmap_slug.clone().unwrap_or_default();
            tracing::info!(
                agent_slug = %agent_slug,
                agent_id = %agent_id,
                conv_id = %msg.conversation_id,
                is_personal = is_personal,
                durable_fallback = durable_agent_id.is_some(),
                space_loop_id = ?space_loop_id,
                personal_loop_id = ?personal_id,
                "dm→agent_space reroute: conv belongs to agent space"
            );

            // Per-chat agent spaces: the gateway binds each conversation to ONE
            // desktop chat (chatId/chatTitle on the JOIN). Route strictly to
            // that chat's session — never "whatever chat is active". The
            // 'general' chat (or a pre-chats server) keeps the legacy
            // unification so the agent's main conversation stays continuous.
            let conv_chat = state
                .comm_manager
                .chat_for_conv(&msg.conversation_id)
                .await;
            let strict_chat = match conv_chat {
                Some((ref cid, _)) if !cid.is_empty() && cid != "general" => true,
                _ => false,
            };
            let session_key = if is_personal && strict_chat {
                let (chat_id, chat_title) = conv_chat.clone().unwrap_or_default();
                let row_id = if is_default_bot { "assistant" } else { agent_id.as_str() };
                match state.store.get_chat(&chat_id) {
                    Ok(Some(chat)) => chat
                        .session_name
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| {
                            agent::keyparser::build_agent_session_key(
                                row_id,
                                &format!("thread:{}", chat_id),
                            )
                        }),
                    _ => {
                        // Loop-created chat the desktop hasn't materialized yet —
                        // create it as a thread of this agent (the same shape the
                        // desktop Threads tab creates).
                        let key = agent::keyparser::build_agent_session_key(
                            row_id,
                            &format!("thread:{}", chat_id),
                        );
                        let title = if chat_title.is_empty() {
                            // "New Chat" (capital C) to match the default-title gate the
                            // auto-namer uses — lowercase here blocked loop chats from
                            // ever being LLM-named.
                            "New Chat".to_string()
                        } else {
                            chat_title.clone()
                        };
                        if let Err(e) = state
                            .store
                            .create_chat_for_session(&chat_id, &key, &title, None)
                        {
                            tracing::warn!(error = %e, chat_id = %chat_id, "failed to create desktop chat for loop chat");
                        }
                        key
                    }
                }
            } else if is_personal && is_default_bot {
                resolve_companion_session_key(&state)
            } else if is_personal {
                resolve_agent_session_key(&state, &agent_id)
            } else {
                agent::keyparser::build_session_key(
                    "neboai",
                    "agent_space",
                    &format!("{}:{}", agent_slug, msg.conversation_id),
                )
            };

            if handle_comm_slash_command(
                &state,
                &text,
                &session_key,
                &msg.topic,
                &msg.conversation_id,
            )
            .await
            .is_some()
            {
                return;
            }

            let agent_name = if is_default_bot {
                "Nebo".to_string()
            } else {
                let registry = state.agent_registry.read().await;
                registry
                    .get(&agent_id)
                    .map(|r| r.name.clone())
                    // Exposed-but-not-enabled agents aren't in the registry — use
                    // the stored display name, never the raw slug.
                    .or_else(|| state.store.get_agent(&agent_id).ok().flatten().map(|a| a.name))
                    .unwrap_or_else(|| agent_slug.clone())
            };
            if !is_default_bot && !strict_chat {
                let _ = state
                    .store
                    .create_chat(&session_key, &format!("Agent: {}", agent_name));
            }

            let preview = if text.len() > 80 {
                format!("{}...", truncate_str(&text, 80))
            } else {
                text.clone()
            };
            notify_crate::send(&format!("Agent space: {}", agent_name), &preview);

            // Broadcast inbound user message to local frontend for real-time display
            if is_personal {
                state.hub.broadcast(
                    "chat_inbound",
                    serde_json::json!({
                        "session_id": session_key,
                        "content": text,
                        "agentId": agent_id,
                        "source": "neboai",
                    }),
                );
            }

            // A resolved agent always uses ITS persona/config (mirrors the channel
            // branch) — otherwise a DM to a secondary agent answers as primary "Nebo".
            let entity_config = if !agent_id.is_empty() {
                entity_config::resolve_for_chat(&state.store, "agent", &agent_id)
            } else if is_personal {
                entity_config::resolve_for_chat(&state.store, "main", "main")
            } else {
                entity_config::resolve_for_chat(&state.store, "channel", "agent_space")
            };

            let mut prompt = text;
            let images = process_comm_attachments(&state, &msg.attachments, &mut prompt).await;

            if try_handle_comm_control(&state, &session_key, &prompt, &msg.metadata).await {
                return;
            }

            let config = chat_dispatch::ChatConfig {
                session_key,
                prompt,
                system: String::new(),
                user_id: String::new(),
                channel: "neboai".to_string(),
                origin: comm_origin(is_personal),
                agent_id,
                cancel_token: tokio_util::sync::CancellationToken::new(),
                lane: types::constants::lanes::COMM.to_string(),
                comm_reply: Some(chat_dispatch::CommReplyConfig {
                    provider: "neboai".to_string(),
                    topic: msg.topic.clone(),
                    conversation_id: msg.conversation_id.clone(),
                }),
                entity_config,
                images,
                entity_name: agent_name.clone(),
                origin_agent_id: None,
                mention_context: None,
                tool_scope: None, plan_mode: false,
                channel_ctx: None,
            };

            chat_dispatch::run_chat(&state, config).await;

            state.event_bus.emit(tools::events::Event {
                source: format!("neboai.agent_space.{}", agent_slug),
                payload: serde_json::json!({
                    "from": msg.from,
                    "content": msg.content,
                    "conversation_id": msg.conversation_id,
                    "agent_slug": agent_slug,
                }),
                origin: "neboai".to_string(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            });
            return;
        }

        let text = extract_message_text(&msg.content);
        if text.is_empty() {
            return;
        }

        // Notify the user about the inbound message
        let preview = if text.len() > 80 {
            format!("{}...", truncate_str(&text, 80))
        } else {
            text.clone()
        };
        notify_crate::send(&format!("Message from {}", msg.from), &preview);

        let session_key =
            agent::keyparser::build_session_key("neboai", &msg.topic, &msg.conversation_id);

        if handle_comm_slash_command(
            &state,
            &text,
            &session_key,
            &msg.topic,
            &msg.conversation_id,
        )
        .await
        .is_some()
        {
            return;
        }

        // Resolve entity config for the channel
        let entity_config = entity_config::resolve_for_chat(&state.store, "channel", &msg.topic);

        // Check for @mention routing — if agent_slug is present, resolve to agent_id
        let agent_id = {
            let agent_slug = msg.metadata.get("agent_slug").cloned().unwrap_or_default();
            resolve_agent_id_from_slug(&state, &agent_slug).await
        };

        // Pre-create chat with @mention context if applicable
        if !agent_id.is_empty() {
            let agent_slug = msg.metadata.get("agent_slug").cloned().unwrap_or_default();
            let _ = state
                .store
                .create_chat(&session_key, &format!("@{} (channel)", agent_slug));
        }

        let mut prompt = text;
        let images = process_comm_attachments(&state, &msg.attachments, &mut prompt).await;

        if try_handle_comm_control(&state, &session_key, &prompt, &msg.metadata).await {
            return;
        }

        let config = chat_dispatch::ChatConfig {
            session_key,
            prompt,
            system: String::new(),
            user_id: String::new(),
            channel: "neboai".to_string(),
            origin: tools::Origin::Comm,
            agent_id,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            lane: types::constants::lanes::COMM.to_string(),
            comm_reply: Some(chat_dispatch::CommReplyConfig {
                provider: "neboai".to_string(),
                topic: msg.topic.clone(),
                conversation_id: msg.conversation_id.clone(),
            }),
            entity_config,
            images,
            entity_name: String::new(),
            origin_agent_id: None,
            mention_context: None,
            tool_scope: None, plan_mode: false,
            channel_ctx: None,
        };

        chat_dispatch::run_chat(&state, config).await;

        // Also emit into event bus so role event triggers can fire
        state.event_bus.emit(tools::events::Event {
            source: format!("neboai.{}", msg.topic),
            payload: serde_json::json!({
                "from": msg.from,
                "content": msg.content,
                "conversation_id": msg.conversation_id,
            }),
            origin: "neboai".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
        return;
    }

    // Route loop CHANNEL messages. Unlike DMs/agent_spaces (which respond to
    // every message), in a channel the bot only answers when explicitly
    // @mentioned: the web embeds an `<@{bot_id}>` token for a real mention
    // chip, so plain text containing the bot's name does NOT trigger a reply.
    if msg.topic == "channel" {
        // Follow-up window: after the bot replies to a user in a channel, that
        // same user may keep talking (without re-mentioning) for this long.
        const CHANNEL_FOLLOWUP_WINDOW_SECS: u64 = 180;
        // Rolling un-answered context buffer limits.
        const CHANNEL_CONTEXT_CAP: usize = 40;
        const CHANNEL_CONTEXT_MAX_AGE_SECS: u64 = 30 * 60;

        let text = extract_message_text(&msg.content);
        if text.is_empty() {
            return;
        }

        // Sender label: prefer the senderName carried in the content JSON
        // (the web sender embeds it), else a short prefix of the sender id.
        let sender_label = serde_json::from_str::<serde_json::Value>(&msg.content)
            .ok()
            .and_then(|v| v["senderName"].as_str().map(|s| s.to_string()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if msg.from.is_empty() {
                    "Someone".to_string()
                } else {
                    truncate_str(&msg.from, 8).to_string()
                }
            });

        // INGEST: every channel message accrues into the rolling buffer,
        // whether or not the bot ends up responding. Trim by cap + age.
        let now = std::time::Instant::now();
        let max_age = std::time::Duration::from_secs(CHANNEL_CONTEXT_MAX_AGE_SECS);
        {
            let mut ctx = state.channel_context.lock().await;
            let deque = ctx.entry(msg.conversation_id.clone()).or_default();
            deque.push_back(state::ChannelMsg {
                sender: sender_label.clone(),
                text: text.clone(),
                at: now,
            });
            while deque
                .front()
                .map(|m| now.duration_since(m.at) > max_age)
                .unwrap_or(false)
            {
                deque.pop_front();
            }
            while deque.len() > CHANNEL_CONTEXT_CAP {
                deque.pop_front();
            }
        }

        // DECIDE: respond on an explicit @mention that resolves to THIS bot or
        // one of its exposed agents, or while an active follow-up window for
        // THIS sender is still open.
        //
        // The web composer embeds `<@{bot_id}>` for the primary bot and
        // `<@{loop_agent_id}>` for a custom exposed agent. Scan every token and
        // resolve the FIRST one that matches a known target. First match wins
        // if multiple agents are mentioned — no fan-out in v1.
        let bot_id = config::read_bot_id().unwrap_or_default();

        // Collect every `<@id>` mention that resolves to a known target — the
        // primary bot (empty id) or an exposed agent — deduped, in order of
        // appearance. This is the fan-out target set: each addressed agent runs
        // and replies for itself.
        let mut mentioned_targets: Vec<String> = Vec::new();
        for cap in MENTION_TOKEN_RE.captures_iter(&text) {
            let id = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let local_id = if !bot_id.is_empty() && id == bot_id {
                Some(String::new()) // primary bot
            } else {
                match state.store.get_agent_by_loop_agent_id(id) {
                    Ok(Some(a)) if a.loop_exposed != 0 => Some(a.id),
                    _ => None,
                }
            };
            // Diagnostic: what the composer put on the wire for each chip — the
            // bot_id (→ routes to primary "Nebo") or an agent's loop UUID. Debug
            // level so it's available when investigating routing but quiet in prod.
            tracing::debug!(
                mention_id = %id,
                is_bot_id = (!bot_id.is_empty() && id == bot_id),
                resolved = ?local_id.as_deref(),
                "channel mention token"
            );
            if let Some(aid) = local_id {
                if !mentioned_targets.contains(&aid) {
                    mentioned_targets.push(aid);
                }
            } else if !id.is_empty() {
                // A mention that doesn't resolve silently drops the agent from
                // the run (and can collapse coordination to the primary → "Nebo").
                // Surface it so a missing loop_agent_id / loop_exposed=0 is visible.
                tracing::warn!(
                    mention_id = %id,
                    conv_id = %msg.conversation_id,
                    "channel mention did not resolve to an exposed agent (loop_agent_id missing or loop_exposed=0)"
                );
            }
        }

        let mentioned = !mentioned_targets.is_empty();

        // Coordination mode: when the user EXPLICITLY asks several addressed
        // agents to produce one joint result, route to a single lead (the
        // first mentioned) that consults the others via `delegate` and writes
        // one combined reply — instead of fanning out N independent answers.
        // Conservative by design; independent fan-out is the default.
        let coordinate = mentioned_targets.len() > 1 && wants_coordination(&text);
        let coordinator_peers: Vec<String> = if coordinate {
            mentioned_targets[1..].to_vec()
        } else {
            Vec::new()
        };
        // Who actually runs and replies: just the lead in coordination mode,
        // otherwise everyone addressed.
        let responders: Vec<String> = if coordinate {
            mentioned_targets[..1].to_vec()
        } else {
            mentioned_targets.clone()
        };

        // The agents to dispatch to. On an explicit mention that's everyone
        // addressed; on a bare follow-up it's the agent(s) the engagement window
        // is bound to (so a user can keep talking to the same group without
        // re-mentioning). A single mention is just a one-element set.
        let targets: Vec<String> = if mentioned {
            let mut eng = state.channel_engagement.lock().await;
            eng.insert(
                msg.conversation_id.clone(),
                state::Engagement {
                    user: msg.from.clone(),
                    expires: now
                        + std::time::Duration::from_secs(CHANNEL_FOLLOWUP_WINDOW_SECS),
                    agent_ids: responders.clone(),
                },
            );
            responders.clone()
        } else {
            let mut eng = state.channel_engagement.lock().await;
            match eng.get(&msg.conversation_id) {
                Some(entry) if entry.user == msg.from && now < entry.expires => {
                    // Same engaged speaker, window still open → extend it and
                    // continue with the agent(s) the window is bound to.
                    let aids = entry.agent_ids.clone();
                    eng.insert(
                        msg.conversation_id.clone(),
                        state::Engagement {
                            user: msg.from.clone(),
                            expires: now
                                + std::time::Duration::from_secs(
                                    CHANNEL_FOLLOWUP_WINDOW_SECS,
                                ),
                            agent_ids: aids.clone(),
                        },
                    );
                    aids
                }
                Some(entry) => {
                    // A different speaker (or an expired window) closes it so a
                    // stale follow-up can't later trigger a reply.
                    if entry.user != msg.from {
                        eng.remove(&msg.conversation_id);
                    }
                    Vec::new()
                }
                None => Vec::new(),
            }
        };

        let should_respond = !targets.is_empty();

        tracing::info!(
            conv_id = %msg.conversation_id,
            from = %msg.from,
            mentioned = mentioned,
            should_respond = should_respond,
            "channel message"
        );

        if !should_respond {
            // Not addressed: message is already buffered for future context.
            // Surface to the event bus for triggers, but don't run the agent.
            state.event_bus.emit(tools::events::Event {
                source: "neboai.channel".to_string(),
                payload: serde_json::json!({
                    "from": msg.from,
                    "content": msg.content,
                    "conversation_id": msg.conversation_id,
                }),
                origin: "neboai".to_string(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            });
            return;
        }

        // Slash commands addressed to the bot in a channel (e.g. "<@bot> /stop").
        // Strip mention tokens so the command resolves, then handle it instead of
        // dispatching an agent run. Single canonical stop/new/clear path for channels
        // (previously these only worked in DMs/agent_spaces).
        let command_text = MENTION_TOKEN_RE.replace_all(&text, "").trim().to_string();
        if command_text.starts_with('/') {
            let session_key =
                agent::keyparser::build_session_key("neboai", "channel", &msg.conversation_id);
            if handle_comm_slash_command(
                &state,
                &command_text,
                &session_key,
                "channel",
                &msg.conversation_id,
            )
            .await
            .is_some()
            {
                return;
            }
        }

        // Respond → fan out to every addressed agent (resolved into `targets`).
        // DRAIN the un-answered buffer for this channel under the lock, then
        // release it. The drained entries are the conversation since the last
        // reply (including the current message, pushed above) — draining on
        // reply prevents re-sending them next turn.
        let buffered: Vec<state::ChannelMsg> = {
            let mut ctx = state.channel_context.lock().await;
            match ctx.get_mut(&msg.conversation_id) {
                Some(deque) => std::mem::take(deque).into_iter().collect(),
                None => Vec::new(),
            }
        };

        // Build a name lookup for every `<@id>` token resolvable to a known
        // bot/agent, so the transcript reads naturally (`@Name`). Covers tokens
        // across all buffered lines, not just the current message. Unknown
        // tokens are left as-is.
        let mut mention_names: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        {
            let registry = state.agent_registry.read().await;
            for line in std::iter::once(&text).chain(buffered.iter().map(|m| &m.text)) {
                for cap in MENTION_TOKEN_RE.captures_iter(line) {
                    let id = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                    if id.is_empty() || mention_names.contains_key(id) {
                        continue;
                    }
                    if !bot_id.is_empty() && id == bot_id {
                        let name = registry
                            .get("assistant")
                            .map(|r| r.name.clone())
                            .unwrap_or_else(|| "Nebo".to_string());
                        mention_names.insert(id.to_string(), name);
                    } else if let Ok(Some(a)) = state.store.get_agent_by_loop_agent_id(id) {
                        let name = registry
                            .get(&a.id)
                            .map(|r| r.name.clone())
                            .unwrap_or(a.name);
                        mention_names.insert(id.to_string(), name);
                    }
                }
            }
        }
        let replace_mentions = |line: &str| -> String {
            MENTION_TOKEN_RE
                .replace_all(line, |cap: &regex::Captures| {
                    let id = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                    match mention_names.get(id) {
                        Some(name) => format!("@{}", name),
                        None => cap.get(0).map(|m| m.as_str()).unwrap_or("").to_string(),
                    }
                })
                .into_owned()
        };

        // Build an attributed transcript as a single user turn.
        let prompt_text = if buffered.len() <= 1 {
            // Single line → no transcript header needed.
            let line = buffered
                .first()
                .map(|m| m.text.clone())
                .unwrap_or_else(|| text.clone());
            replace_mentions(&line)
        } else {
            let mut t = String::from("[Recent activity in this channel]\n");
            for m in &buffered {
                let line = replace_mentions(&m.text);
                t.push_str(&format!("{}: {}\n", m.sender, line));
            }
            t
        };

        // Shared prompt + attachments — computed once, cloned per agent below.
        let mut prompt = prompt_text;
        let images = process_comm_attachments(&state, &msg.attachments, &mut prompt).await;

        // Resolve a display name for every target (one registry read), capped so
        // a message mentioning a crowd can't fan out unbounded.
        const MAX_FANOUT: usize = 4;
        let dispatch: Vec<(String, String)> = {
            let registry = state.agent_registry.read().await;
            targets
                .iter()
                .map(|agent_id| {
                    let name = if agent_id.is_empty() {
                        registry.get("assistant").map(|r| r.name.clone())
                    } else {
                        // A loop-exposed agent may be resolved (its loop_agent_id)
                        // yet NOT loaded in the registry (not enabled locally) — fall
                        // back to its stored name, never the primary's "Nebo".
                        registry
                            .get(agent_id)
                            .map(|r| r.name.clone())
                            .or_else(|| state.store.get_agent(agent_id).ok().flatten().map(|a| a.name))
                    }
                    .unwrap_or_else(|| "Nebo".to_string());
                    (agent_id.clone(), name)
                })
                .collect()
        };
        let dispatch = if dispatch.len() > MAX_FANOUT {
            tracing::warn!(
                conv_id = %msg.conversation_id,
                mentioned = dispatch.len(),
                cap = MAX_FANOUT,
                "channel fan-out capped — only the first agents respond"
            );
            dispatch[..MAX_FANOUT].to_vec()
        } else {
            dispatch
        };

        // In coordination mode, resolve the peer display names the lead will be
        // told to consult via `delegate`.
        let coordinator_peer_names: Vec<String> = if coordinate {
            let registry = state.agent_registry.read().await;
            coordinator_peers
                .iter()
                .map(|agent_id| {
                    if agent_id.is_empty() {
                        registry.get("assistant").map(|r| r.name.clone())
                    } else {
                        registry
                            .get(agent_id)
                            .map(|r| r.name.clone())
                            .or_else(|| state.store.get_agent(agent_id).ok().flatten().map(|a| a.name))
                    }
                    .unwrap_or_else(|| "Nebo".to_string())
                })
                .collect()
        } else {
            Vec::new()
        };

        let preview = if prompt.len() > 80 {
            format!("{}...", truncate_str(&prompt, 80))
        } else {
            prompt.clone()
        };
        // Owned name list — used for the notification AND, when fanning out to
        // more than one agent, to tell each agent who else was addressed so it
        // replies only as itself.
        let all_names: Vec<String> = dispatch.iter().map(|(_, n)| n.clone()).collect();
        let is_group = all_names.len() > 1;
        notify_crate::send(&format!("Loop channel: {}", all_names.join(", ")), &preview);

        // Fan out: one independent run per addressed agent. Each gets its OWN
        // session within the channel (so histories don't collide) and replies
        // for itself via the existing per-reply senderName attribution. run_chat
        // enqueues async on the COMM lane, so these runs proceed concurrently.
        for (agent_id, agent_name) in dispatch {
            let session_key = if agent_id.is_empty() {
                agent::keyparser::build_session_key("neboai", "channel", &msg.conversation_id)
            } else {
                agent::keyparser::build_session_key(
                    "neboai",
                    "channel",
                    &format!("{}:{}", msg.conversation_id, agent_id),
                )
            };
            let _ = state
                .store
                .create_chat(&session_key, &format!("Loop channel ({})", agent_name));

            // Use the agent's config (custom agent) or the bot's main persona.
            let entity_config = if !agent_id.is_empty() {
                entity_config::resolve_for_chat(&state.store, "agent", &agent_id)
            } else {
                entity_config::resolve_for_chat(&state.store, "main", "main")
            };

            // Coordination framing: the lead is told to consult the named peers
            // via `delegate` (bounded by the sub-agent depth cap) and write one
            // combined answer — the peers do not reply here on their own.
            let mention_context = if coordinate {
                Some(format!(
                    "You are the lead for this request. The user asked you to work together with \
                     {peers} to produce ONE combined result. They are NOT replying here on their \
                     own — consult a peer when you need their expertise by calling \
                     agent(resource: \"registry\", action: \"delegate\", name: \"{first}\", \
                     prompt: \"<what you need from them>\") — then write a single integrated \
                     answer yourself.",
                    peers = coordinator_peer_names.join(", "),
                    first = coordinator_peer_names.first().map(|s| s.as_str()).unwrap_or("the peer"),
                ))
            } else if is_group {
                let others: Vec<&str> = all_names
                    .iter()
                    .map(|s| s.as_str())
                    .filter(|n| *n != agent_name.as_str())
                    .collect();
                Some(format!(
                    "You are \"{}\". This message was sent to several agents at once ({}), and a \
                     SEPARATE copy was delivered to each of them — so even if it says \"each of \
                     you\", \"all of you\", or lists names, answer ONLY about yourself, in the \
                     first person. The other agents ({}) are replying on their own; do NOT speak \
                     for them, introduce them, quote them, or describe their roles. The platform \
                     places your reply next to theirs automatically.",
                    agent_name,
                    all_names.join(", "),
                    others.join(", "),
                ))
            } else {
                None
            };

            let config = chat_dispatch::ChatConfig {
                session_key,
                prompt: prompt.clone(),
                system: String::new(),
                user_id: String::new(),
                channel: "neboai".to_string(),
                origin: tools::Origin::Comm,
                agent_id,
                cancel_token: tokio_util::sync::CancellationToken::new(),
                lane: types::constants::lanes::COMM.to_string(),
                comm_reply: Some(chat_dispatch::CommReplyConfig {
                    provider: "neboai".to_string(),
                    topic: "channel".to_string(),
                    conversation_id: msg.conversation_id.clone(),
                }),
                entity_config,
                images: images.clone(),
                entity_name: agent_name,
                origin_agent_id: None,
                mention_context,
                tool_scope: None,
                plan_mode: false,
                channel_ctx: None,
            };

            chat_dispatch::run_chat(&state, config).await;
        }

        state.event_bus.emit(tools::events::Event {
            source: "neboai.channel".to_string(),
            payload: serde_json::json!({
                "from": msg.from,
                "content": msg.content,
                "conversation_id": msg.conversation_id,
            }),
            origin: "neboai".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
        return;
    }

    // Emit other message types into event bus for role triggers
    state.event_bus.emit(tools::events::Event {
        source: format!("neboai.{}", msg.topic),
        payload: serde_json::json!({
            "from": msg.from,
            "content": msg.content,
            "topic": msg.topic,
        }),
        origin: "neboai".to_string(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    });

    // Default: broadcast to frontend clients
    state.hub.broadcast(
        "comm_message",
        serde_json::json!({
            "from": msg.from,
            "to": msg.to,
            "content": msg.content,
            "type": msg.msg_type,
            "topic": msg.topic,
        }),
    );
}

/// Resolve a role ID from an agent slug by scanning the active role registry.
/// Resolve the companion chat's session key (matches what the frontend uses).
/// Falls back to "web" if no companion chat exists yet.
fn resolve_companion_session_key(state: &AppState) -> String {
    match state.store.get_companion_chat_by_user("companion-default") {
        Ok(Some(chat)) => {
            let key = chat.session_name.unwrap_or(chat.id);
            tracing::debug!(session_key = %key, "resolved companion session key for NeboAI unification");
            key
        }
        _ => "web".to_string(),
    }
}

/// Resolve the session key for an inbound personal-loop message to a secondary
/// agent: the agent's most recently active local conversation, so loop DMs and
/// desktop threads stay ONE continuous conversation (the secondary-agent
/// counterpart of `resolve_companion_session_key`). Falls back to the agent's
/// legacy `:web` session when the agent has no conversations yet.
fn resolve_agent_session_key(state: &AppState, agent_id: &str) -> String {
    match state.store.get_latest_agent_chat(agent_id) {
        Ok(Some(chat)) => {
            let key = chat
                .session_name
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| agent::keyparser::build_agent_session_key(agent_id, "web"));
            tracing::debug!(session_key = %key, agent_id = %agent_id, "resolved agent session key for NeboAI unification");
            key
        }
        _ => agent::keyparser::build_agent_session_key(agent_id, "web"),
    }
}

async fn resolve_agent_id_from_slug(state: &AppState, slug: &str) -> String {
    if slug.is_empty() {
        return String::new();
    }
    let registry = state.agent_registry.read().await;
    for (id, role) in registry.iter() {
        if comm::handle::slugify(&role.name) == slug {
            return id.clone();
        }
    }
    String::new()
}

/// Resolve an inbound agent_space/dm delivery to a STABLE local agent id.
///
/// Returns `(local_agent_id, is_default_bot)`. `local_agent_id` is empty for
/// the default/primary bot. This never drops a message: any handle starting
/// with `bot_` (`bot_<id>` or `bot_<chosen>`) routes to the primary bot, and a
/// custom-agent slug that no longer resolves locally also falls back to the
/// primary bot rather than being silently dropped.
///
/// Resolution order (most stable first):
/// 1. PRIMARY handle (`bot_<id8>`, no further `_`) → primary bot.
/// 2. The loop's agent UUID for this conversation (or the delivery metadata)
///    → local agent via the stored `loop_agent_id` bridge. This is the stable,
///    NON-name-based path and is authoritative.
/// 3. Fallback (pre-stabilization only): secondary handle `bot_<id8>_<slug>` →
///    `<slug>` → local agent by name. Used only until `loop_agent_id` is stored.
/// 4. unresolved → primary bot (never drop).
async fn resolve_inbound_agent(
    state: &AppState,
    agent_slug: &str,
    conv_id: &str,
    metadata: &std::collections::HashMap<String, String>,
) -> (String, bool) {
    // 1. The bot's own (primary) handle.
    if comm::handle::is_primary_handle(agent_slug) {
        return (String::new(), true);
    }

    // 2. STABLE, non-name resolution: the conversation's loop agent UUID (from the
    // JOIN-populated ConvMaps, or the delivery metadata) → local agent via the
    // stored `loop_agent_id` bridge.
    let loop_agent_id = state
        .comm_manager
        .agent_id_for_conv(conv_id)
        .await
        .filter(|v| !v.is_empty())
        .or_else(|| metadata.get("agent_id").filter(|v| !v.is_empty()).cloned());
    if let Some(loop_agent_id) = loop_agent_id {
        if let Ok(Some(a)) = state.store.get_agent_by_loop_agent_id(&loop_agent_id) {
            return (a.id, false);
        }
    }

    // 3. Fallback (until loop_agent_id is stored): for a secondary handle
    // (`bot_<id8>_<slug>`) strip the bot prefix to the agent slug.
    let lookup = comm::handle::secondary_agent_slug(agent_slug).unwrap_or(agent_slug);
    let id = resolve_agent_id_from_slug(state, lookup).await;
    if !id.is_empty() {
        return (id, false);
    }

    // 4. Unresolved: route to the primary bot instead of dropping the message.
    tracing::warn!(
        agent_slug = %agent_slug,
        conv_id = %conv_id,
        "inbound: agent did not resolve locally, routing to primary bot"
    );
    (String::new(), true)
}

/// Handle built-in slash commands from comm (NeboAI) messages.
/// Returns Some(response_text) if the prompt was a slash command that was handled,
/// None if the prompt should be processed normally by the agent.
async fn handle_comm_slash_command(
    state: &AppState,
    text: &str,
    session_key: &str,
    topic: &str,
    conversation_id: &str,
) -> Option<()> {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    let (cmd, _args) = match trimmed.find(' ') {
        Some(i) => (&trimmed[..i], trimmed[i + 1..].trim()),
        None => (trimmed, ""),
    };
    let cmd = cmd.to_lowercase();

    let response = match cmd.as_str() {
        "/new" | "/reset" => {
            let cancelled = state.run_registry.cancel_by_session(session_key).await;
            if cancelled {
                tracing::info!(session_key = %session_key, "cancelled active run before /new");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            match state
                .runner
                .sessions()
                .resolve_session_id_by_key(session_key)
                .and_then(|sid| state.runner.sessions().reset(&sid))
            {
                Ok(_new_chat_id) => {
                    tracing::info!(
                        session_key = %session_key,
                        "comm slash: /new — rotated to fresh conversation"
                    );
                    "New conversation started. Previous context has been cleared.".to_string()
                }
                Err(e) => format!("Failed to start new conversation: {}", e),
            }
        }

        "/clear" => {
            let cancelled = state.run_registry.cancel_by_session(session_key).await;
            if cancelled {
                tracing::info!(session_key = %session_key, "cancelled active run before /clear");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            match state
                .runner
                .sessions()
                .resolve_session_id_by_key(session_key)
                .and_then(|sid| state.runner.sessions().clear_current_messages(&sid))
            {
                Ok(()) => "Conversation cleared.".to_string(),
                Err(e) => format!("Failed to clear: {}", e),
            }
        }

        "/stop" | "/cancel" | "/halt" => {
            let cancelled = state.run_registry.cancel_by_session(session_key).await;
            tracing::info!(
                session_key = %session_key,
                cancelled,
                "comm slash: /stop — cancel requested"
            );
            if cancelled {
                "Stopped.".to_string()
            } else {
                "Nothing is running right now.".to_string()
            }
        }

        "/status" => {
            let msg_count = state
                .runner
                .sessions()
                .resolve_session_id_by_key(session_key)
                .ok()
                .and_then(|sid| state.runner.sessions().get_messages(&sid).ok())
                .map(|m| m.len())
                .unwrap_or(0);

            format!(
                "Session: {}\nMessages in context: {}",
                session_key, msg_count,
            )
        }

        "/help" => {
            "/new — Start a new conversation (preserves history)\n\
             /clear — Clear current conversation messages\n\
             /stop — Stop the current run\n\
             /status — Show session info\n\
             /help — Show this help"
                .to_string()
        }

        _ => return None,
    };

    let reply = comm::CommMessage {
        id: uuid::Uuid::new_v4().to_string(),
        from: String::new(),
        to: String::new(),
        topic: topic.to_string(),
        conversation_id: conversation_id.to_string(),
        msg_type: comm::CommMessageType::Message,
        content: response,
        metadata: std::collections::HashMap::new(),
        timestamp: 0,
        human_injected: false,
        human_id: None,
        task_id: None,
        correlation_id: None,
        task_status: None,
        artifacts: vec![],
        error: None,
        attachments: vec![],
    };
    if let Err(e) = state.comm_manager.send(reply).await {
        tracing::warn!(error = %e, "failed to send slash command response via comm");
    }

    Some(())
}

/// Build the system-role context preamble for an embed conversation.
/// `ctx` is the host-page-asserted context (advisory); `embed` is the
/// gateway-stamped app identity (trusted). Absent fields are omitted.
fn build_embed_context(
    ctx: Option<&serde_json::Value>,
    embed: Option<&serde_json::Value>,
) -> Option<String> {
    if ctx.is_none() && embed.is_none() {
        return None;
    }

    let str_at = |v: Option<&serde_json::Value>, key: &str| -> Option<String> {
        v.and_then(|o| o.get(key))
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };

    let app = str_at(ctx, "app");
    let verified = str_at(embed, "app");

    let mut out = String::new();
    match (&app, &verified) {
        (Some(a), Some(v)) => out.push_str(&format!(
            "The user is chatting from inside {} (verified app: {}).",
            a, v
        )),
        (Some(a), None) => out.push_str(&format!("The user is chatting from inside {}.", a)),
        (None, Some(v)) => out.push_str(&format!(
            "The user is chatting from an embedded widget (verified app: {}).",
            v
        )),
        (None, None) => out.push_str("The user is chatting from an embedded widget."),
    }

    let mut location: Vec<String> = Vec::new();
    for key in ["project", "task"] {
        let obj = ctx.and_then(|c| c.get(key)).filter(|v| v.is_object());
        let name = str_at(obj, "name");
        let id = str_at(obj, "id");
        match (name, id) {
            (Some(n), Some(i)) => location.push(format!("{} \"{}\" (id {})", key, n, i)),
            (Some(n), None) => location.push(format!("{} \"{}\"", key, n)),
            (None, Some(i)) => location.push(format!("{} id {}", key, i)),
            (None, None) => {}
        }
    }
    if let Some(url) = str_at(ctx, "url") {
        location.push(format!("page {}", url));
    }
    if let Some(method) = str_at(ctx, "method") {
        location.push(format!("intent: {}", method));
    }
    if !location.is_empty() {
        out.push_str(&format!(" Current location: {}.", location.join(", ")));
    }

    if let Some(a) = app.or(verified) {
        out.push_str(&format!(
            " Use your {} tools (MCP) to look up or act on this context when relevant.",
            a
        ));
    }

    Some(out)
}

/// Extract text from a comm message content (JSON or plain text).
fn extract_message_text(content: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(text) = v["text"].as_str() {
            return text.to_string();
        }
        if let Some(text) = v["content"].as_str() {
            return text.to_string();
        }
    }
    content.to_string()
}

/// Convert image attachments to AI vision content and append text descriptions
/// for non-image attachments to the prompt.
async fn process_comm_attachments(
    state: &state::AppState,
    attachments: &[comm::wire::Attachment],
    prompt: &mut String,
) -> Vec<ai::ImageContent> {
    use base64::Engine;

    if attachments.is_empty() {
        return vec![];
    }

    let api = match codes::build_api_client(state) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, "cannot download attachments: no API client");
            return vec![];
        }
    };

    let mut images = Vec::new();

    for att in attachments {
        if att.mime_type.starts_with("image/") {
            match api.download_file(&att.file_id).await {
                Ok(bytes) => {
                    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    images.push(ai::ImageContent {
                        media_type: att.mime_type.clone(),
                        data,
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        file_id = %att.file_id,
                        error = %e,
                        "failed to download image attachment"
                    );
                }
            }
        } else {
            // Append a text description for non-image attachments
            let size_kb = att.size / 1024;
            let size_label = if size_kb >= 1024 {
                format!("{:.1} MB", size_kb as f64 / 1024.0)
            } else {
                format!("{} KB", size_kb)
            };
            prompt.push_str(&format!("\n[Attached: {} ({})]", att.filename, size_label));
        }
    }

    images
}

async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        version: VERSION.into(),
    })
}

fn cors_layer() -> CorsLayer {
    use axum::http::HeaderValue;
    use tower_http::cors::AllowOrigin;

    let static_origins: Vec<HeaderValue> = [
        "http://localhost:27895",
        "http://127.0.0.1:27895",
        "http://localhost:5173",
        "http://127.0.0.1:5173",
        "http://localhost:4173",
        "http://127.0.0.1:4173",
    ]
    .iter()
    .filter_map(|o| o.parse().ok())
    .collect();

    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin: &HeaderValue, _| {
            // Allow neboapp:// origins (Tauri custom protocol for app windows)
            if let Ok(s) = origin.to_str() {
                if s.starts_with("neboapp://") {
                    return true;
                }
            }
            static_origins.contains(origin)
        }))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
            Method::PATCH,
        ])
        .allow_headers(tower_http::cors::AllowHeaders::mirror_request())
        .allow_credentials(true)
}
