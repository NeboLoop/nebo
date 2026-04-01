pub mod chat_dispatch;
pub mod codes;
pub mod deps;
pub mod entity_config;
pub mod handlers;
pub mod middleware;
pub mod routes;
pub mod workflow_manager;
mod heartbeat;
mod migration;
mod scheduler;
mod spa;
mod state;

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

use axum::http::Method;
use axum::response::Json;
use axum::Router;
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

/// Build AI providers from auth_profiles in the database.
/// Config is needed for NeboLoop's Janus URL (not stored in auth_profile).
pub fn build_providers(store: &db::Store, cfg: &Config, cli_statuses: Option<&config::AllCliStatuses>) -> Vec<Arc<dyn ai::Provider>> {
    let profiles = match store.list_auth_profiles() {
        Ok(p) => p,
        Err(e) => {
            warn!("failed to load auth profiles: {}", e);
            return Vec::new();
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
                    profile
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "http://localhost:11434".into()),
                    profile.model.clone().unwrap_or(default_model),
                )))
            }
            "neboloop" => {
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
                        .map(|models| models.iter().any(|m| {
                            let caps: Vec<String> = m.capabilities
                                .as_ref()
                                .and_then(|c| serde_json::from_str(c).ok())
                                .unwrap_or_default();
                            caps.iter().any(|c| c == "streaming" || c == "tools")
                        }))
                        .unwrap_or(false);
                    if !has_active_chat {
                        info!("janus provider has no active models in catalog, skipping");
                        None
                    } else {
                    // Janus URL comes from config (NeboLoop.JanusURL), NOT auth_profile base_url
                    let janus_url = &cfg.neboloop.janus_url;
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
                        "loaded Janus provider via NeboLoop"
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
                        "neboloop profile found but janus_provider not enabled, skipping AI provider"
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
            providers.push(p);
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

    if providers.is_empty() {
        warn!("no active AI providers configured — agent will be unavailable until providers are added");
    }

    providers
}

/// Run the Nebo HTTP server.
pub async fn run(cfg: Config, quiet: bool) -> Result<(), NeboError> {
    let port = cfg.port;
    let host = cfg.host.clone();
    let bind_addr = format!("{host}:{port}");

    // Check port availability
    TcpListener::bind(&bind_addr).map_err(|_| NeboError::PortInUse(port))?;

    if !quiet {
        println!("Starting server on http://localhost:{port}");
    }

    // Initialize database
    let store = Arc::new(db::Store::new(&cfg.database.sqlite_path)?);

    // Ensure FTS5 index for memories is healthy (auto-rebuild if corrupted)
    if let Err(e) = store.ensure_fts_healthy() {
        warn!(error = %e, "FTS health check failed — memory search may be degraded");
    }

    // Clean up orphaned workflow runs from previous shutdown
    match store.cleanup_orphaned_runs() {
        Ok(0) => {}
        Ok(n) => info!(count = n, "cancelled orphaned workflow runs from previous session"),
        Err(e) => warn!(error = %e, "failed to clean up orphaned workflow runs"),
    }

    // Ensure bot_id exists: file → DB (Go migration) → generate new
    if config::read_bot_id().is_none() {
        // Check DB for bot_id set by the Go version (plugin_settings table)
        let from_db = store
            .get_plugin_setting("neboloop", "bot_id")
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
        let _ = store.set_plugin_setting("neboloop", "bot_id", &bot_id);
    }

    // Auto-mark setup complete: DB initialized + bot_id exists = setup is done
    if !config::is_setup_complete().unwrap_or(false) {
        if config::read_bot_id().is_some() {
            if let Err(e) = config::mark_setup_complete() {
                warn!("failed to mark setup complete: {}", e);
            } else {
                info!("setup marked complete (DB ready + bot_id exists)");
            }
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
    let providers = build_providers(&store, &cfg, Some(&cli_statuses));

    // Build tool registry with default tools
    let mut policy = tools::Policy::new();
    policy.level = tools::PolicyLevel::Full;
    policy.ask_mode = tools::AskMode::Off;
    let data_dir = config::data_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let tool_registry = Arc::new(tools::Registry::new(policy));

    // Create empty orchestrator handle (filled after Runner is built)
    let orch_handle = tools::new_handle();

    // Initialize browser manager (with built-in ExtensionBridge for Chrome extension relay)
    let browser_config = browser::BrowserConfig::default();
    let browser_data_dir = data_dir
        .join("browser")
        .to_string_lossy()
        .to_string();
    let browser_manager = Arc::new(
        browser::Manager::new(browser_config, browser_data_dir)
    );
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
    }

    // Ensure artifact directory structure exists (nebo/ and user/ namespaces)
    if let Err(e) = config::ensure_artifact_dirs() {
        warn!("failed to create artifact directories: {}", e);
    }

    // Run one-time migration from old layout to nebo/user split
    migration::migrate_if_needed(&data_dir);

    // Extract sealed .napp archives to sibling directories (one-time)
    migration::migrate_napp_extraction(&data_dir);

    // Initialize plugin store for shared binary management
    let plugins_dir = data_dir.join("nebo").join("plugins");
    let _ = std::fs::create_dir_all(&plugins_dir);
    let user_plugins_dir = data_dir.join("user").join("plugins");
    let _ = std::fs::create_dir_all(&user_plugins_dir);
    let plugin_store = Arc::new(napp::plugin::PluginStore::new(plugins_dir, user_plugins_dir, None));

    // Initialize skill loader (bundled + extracted dirs from nebo/skills/ + loose files from user/skills/)
    let bundled_skills_dir = config::bundled_skills_dir().unwrap_or_else(|_| data_dir.join("bundled").join("skills"));
    let installed_skills_dir = data_dir.join("nebo").join("skills");
    let user_skills_dir = data_dir.join("user").join("skills");
    let _ = std::fs::create_dir_all(&bundled_skills_dir);
    let skill_loader = Arc::new(tools::skills::Loader::new(
        bundled_skills_dir,
        installed_skills_dir,
        user_skills_dir,
    ).with_plugin_store(plugin_store.clone()));
    skill_loader.load_all().await;
    skill_loader.watch();

    // Initialize advisor loader and runner (ADVISOR.md + DB advisors, LLM deliberation)
    let advisors_dir = data_dir.join("advisors");
    let advisor_loader = Arc::new(agent::advisors::Loader::new(advisors_dir, store.clone()));
    advisor_loader.load_all().await;
    advisor_loader.watch();

    // Build a second provider set for advisor deliberation (includes CLI providers)
    let advisor_providers = build_providers(&store, &cfg, Some(&cli_statuses));
    let advisor_runner: Option<Arc<dyn tools::AdvisorDeliberator>> = if advisor_providers.is_empty() {
        None
    } else {
        Some(Arc::new(agent::advisors::Runner::new(
            advisor_loader,
            Arc::new(advisor_providers),
        )))
    };

    // Create hybrid search adapter (FTS5 + vector similarity for memory search)
    let hybrid_searcher: Arc<dyn tools::HybridSearcher> = Arc::new(
        agent::search_adapter::HybridSearchAdapter::new(store.clone(), None),
    );

    // Initialize napp package registry
    let napp_config = napp::RegistryConfig {
        installed_tools_dir: data_dir.join("nebo").join("tools"),
        user_tools_dir: data_dir.join("user").join("tools"),
        neboloop_url: Some(cfg.neboloop.api_url.clone()),
    };
    let napp_registry = Arc::new(napp::Registry::new(napp_config));

    // Plan tier — updated by NeboLoop AUTH_OK handler, read by ExecuteTool
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
    let active_role_state: tools::AgentRegistry = std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));

    // Create broadcaster closure for tools to emit WS events
    let hub_for_tools = hub.clone();
    let broadcaster: tools::web_tool::Broadcaster = Arc::new(move |event_type, payload| {
        hub_for_tools.broadcast(event_type, payload);
    });

    tool_registry.set_plugin_store(plugin_store.clone());
    tool_registry
        .register_all_with_permissions(
            store.clone(),
            Some(browser_manager),
            orch_handle.clone(),
            Some(skill_loader.clone()),
            advisor_runner,
            Some(hybrid_searcher),
            None, // workflow_manager registered separately after Runner is created
            None,
            Some(plan_tier.clone()),
            sandbox_manager,
            None, // comm_plugin — set later when NeboLoop connects
            Some(active_role_state.clone()),
            Some(broadcaster),
        )
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

    // Register MCP STRAP tool — single tool for all connected MCP servers
    let mcp_tool = tools::mcp_tool::McpTool::new(bridge.clone(), store.clone());
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
                    let server_url = match &i.server_url {
                        Some(u) if !u.is_empty() => u.clone(),
                        _ => continue,
                    };
                    // Skip OAuth integrations that haven't completed auth yet
                    if i.auth_type == "oauth" && i.connection_status.is_none() {
                        continue;
                    }
                    // Retrieve stored OAuth token, refreshing if expired
                    let access_token = if i.auth_type == "oauth" {
                        match store_init.get_mcp_credential_full(&i.id, "oauth_token") {
                            Ok(Some(cred)) => {
                                if tools::mcp_tool::is_token_expired(cred.expires_at) && cred.refresh_token.is_some() {
                                    info!(name = %i.name, "MCP token expired on startup, attempting refresh");
                                    match tools::mcp_tool::refresh_mcp_token(&store_init, bridge_init.client(), &i.id).await {
                                        Ok(new_token) => Some(new_token),
                                        Err(e) => {
                                            warn!(name = %i.name, error = %e, "MCP token refresh on startup failed");
                                            bridge_init.client().decrypt_token(&cred.credential_value).ok()
                                        }
                                    }
                                } else {
                                    bridge_init.client().decrypt_token(&cred.credential_value).ok()
                                }
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };
                    let tool_prefix = i.name.to_lowercase()
                        .chars()
                        .map(|c| if c.is_alphanumeric() { c } else { '_' })
                        .collect::<String>()
                        .trim_matches('_')
                        .to_string();
                    match bridge_init.connect(&i.id, &tool_prefix, &server_url, access_token.as_deref()).await {
                        Ok(tools) => {
                            let _ = store_init.set_mcp_connection_status(&i.id, "connected", tools.len() as i64);
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

    // Set quarantine handler to broadcast via hub
    {
        let hub = hub.clone();
        napp_registry.set_quarantine_handler(move |event| {
            hub.broadcast("tool_quarantined", serde_json::json!({
                "toolId": event.tool_id,
                "reason": event.reason,
            }));
        }).await;
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
                    if tool.running { continue; }
                    if supervisor.should_restart(&tool.id).await {
                        supervisor.record_restart(&tool.id).await;
                        hub_ref.broadcast("tool_error", serde_json::json!({
                            "toolId": tool.id,
                            "error": "process died",
                        }));
                    }
                }
            }
        });
    }

    // Create comm plugin manager
    let comm_manager = Arc::new(comm::PluginManager::new());
    {
        let neboloop_plugin = Arc::new(comm::NeboLoopPlugin::new());
        let loopback_plugin = Arc::new(comm::LoopbackPlugin::new());
        comm_manager.register(neboloop_plugin).await;
        comm_manager.register(loopback_plugin).await;

        // Wire incoming messages to ClientHub broadcast + install event routing
        let comm_hub = hub.clone();
        let install_registry = napp_registry.clone();
        comm_manager
            .set_message_handler({
                let comm_hub = comm_hub.clone();
                let registry = install_registry.clone();
                Arc::new(move |msg: comm::CommMessage| {
                    // Route install events to napp registry
                    if msg.topic == "installs" {
                        if let Ok(event) = serde_json::from_str::<napp::InstallEvent>(&msg.content) {
                            let reg = registry.clone();
                            let hub = comm_hub.clone();
                            tokio::spawn(async move {
                                match reg.handle_install_event(event).await {
                                    Ok(()) => hub.broadcast("tool_event", serde_json::json!({"status": "ok"})),
                                    Err(e) => {
                                        tracing::warn!("install event handling failed: {}", e);
                                        hub.broadcast("tool_error", serde_json::json!({"error": e.to_string()}));
                                    }
                                }
                            });
                            return;
                        }
                    }
                    // Default: broadcast to clients
                    comm_hub.broadcast(
                        "comm_message",
                        serde_json::json!({
                            "from": msg.from,
                            "to": msg.to,
                            "content": msg.content,
                            "type": msg.msg_type,
                        }),
                    );
                })
            })
            .await;
    }

    // NeboLoop auto-connect and reconnect watcher are spawned after AppState construction
    // (see below) so they can use codes::activate_neboloop(&state).

    // Create lane manager and start pumps
    let lanes = Arc::new(agent::LaneManager::new());
    lanes.start_pumps();

    // Create adaptive concurrency controller and spawn resource monitor
    let concurrency = Arc::new(agent::ConcurrencyController::new());
    agent::concurrency::spawn_monitor(concurrency.clone());

    // Load models catalog from embedded models.yaml (needed for selector before runner)
    let models_cfg = config::ModelsConfig::load();
    let model_count: usize = models_cfg.providers.values().map(|v| v.len()).sum();
    info!(providers = models_cfg.providers.len(), models = model_count, "loaded models catalog");

    // Collect active provider IDs from auth profiles
    let active_provider_ids: Vec<String> = providers.iter().map(|p| p.id().to_string()).collect();

    // Build DB model overrides so the selector respects user toggles
    let model_overrides = build_model_overrides(&store);

    // Build real routing config from models catalog
    let routing_config = agent::selector::ModelRoutingConfig::from_models_config(&models_cfg, &active_provider_ids, &model_overrides);
    let selector = agent::ModelSelector::new(routing_config);

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
    let mcp_context = Arc::new(tokio::sync::Mutex::new(tools::ToolContext::default()));

    let runner = Arc::new(agent::Runner::new(
        store.clone(),
        tool_registry.clone(),
        providers,
        selector,
        concurrency.clone(),
        hooks.clone(),
        Some(mcp_context.clone()),
        active_role_state.clone(),
        Some(skill_loader.clone()),
    ));

    // Create event bus and dispatcher for workflow-to-workflow events
    let (event_bus, event_rx) = tools::EventBus::new();
    let event_dispatcher = Arc::new(workflow::events::EventDispatcher::new());

    // Register EmitTool so it appears in tools list and is available to all origins
    tool_registry.register(Box::new(tools::EmitTool::new(event_bus.clone()))).await;

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
    tool_registry.register(Box::new(tools::WorkTool::new(
        workflow_manager.clone() as Arc<dyn tools::WorkflowManager>,
    ))).await;

    // Create agent worker registry — manages autonomous trigger lifecycle for each agent
    let agent_workers = Arc::new(agent::AgentWorkerRegistry::new(
        store.clone(),
        workflow_manager.clone() as Arc<dyn tools::WorkflowManager>,
        event_dispatcher.clone(),
        plugin_store.clone(),
    ));

    // Start workers for all enabled agents (replaces manual trigger reconciliation)
    {
        if let Ok(agents) = store.list_agents(1000, 0) {
            let mut started = 0usize;
            for agent in &agents {
                if agent.is_enabled == 0 {
                    continue;
                }
                agent_workers.start_agent(&agent.id, &agent.name).await;
                started += 1;
            }
            if started > 0 {
                info!(count = started, "started agent workers for enabled agents");
            }
        }
    }

    // Populate agent_registry from DB so enabled agents appear in sidebar after restart
    {
        if let Ok(agents) = store.list_agents(1000, 0) {
            let mut registry = active_role_state.write().await;
            for agent in &agents {
                if agent.is_enabled == 0 {
                    continue;
                }
                let config = if !agent.frontmatter.is_empty() {
                    napp::agent::parse_agent_config(&agent.frontmatter).ok()
                } else {
                    None
                };
                registry.insert(agent.id.clone(), tools::ActiveAgent {
                    agent_id: agent.id.clone(),
                    name: agent.name.clone(),
                    agent_md: agent.agent_md.clone(),
                    config,
                    channel_id: None,
                    degraded: None,
                });
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

    let jwt_secret = JwtSecret(cfg.auth.access_secret.clone());

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
        approval_channels: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        ask_channels: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
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
        presence: Arc::new(agent::PresenceTracker::new()),
        proactive_inbox: Arc::new(agent::ProactiveInbox::new()),
    };

    // Replace comm message handler with full version that routes chat/DM to agent runner
    {
        let handler_state = state.clone();
        state.comm_manager.set_message_handler({
            Arc::new(move |msg: comm::CommMessage| {
                let st = handler_state.clone();
                tokio::spawn(async move {
                    handle_comm_message(st, msg).await;
                });
            })
        }).await;
    }

    // Auto-connect NeboLoop if enabled and credentials exist
    if cfg.is_neboloop_enabled() {
        let auto_state = state.clone();
        tokio::spawn(async move {
            match codes::activate_neboloop(&auto_state).await {
                Ok(()) => info!("neboloop: connected to gateway"),
                Err(e) => info!("neboloop: auto-connect skipped: {}", e),
            }
        });
    }

    // Reconnect watcher with exponential backoff
    if cfg.is_neboloop_enabled() {
        let reconnect_state = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let mut backoff_secs: u64 = 30;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                if reconnect_state.comm_manager.is_connected().await {
                    backoff_secs = 30;
                    continue;
                }
                match codes::activate_neboloop(&reconnect_state).await {
                    Ok(()) => {
                        info!("neboloop: reconnected to gateway");
                        // Persist rotated JWT so next reconnect uses the fresh token
                        if let Some(new_token) = reconnect_state.comm_manager.take_rotated_token().await {
                            if let Err(e) = reconnect_state.store.update_auth_profile_token_by_provider("neboloop", &new_token) {
                                warn!("neboloop: failed to persist rotated token: {}", e);
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
                        let progress_fn: updater::ProgressFn = Box::new(move |downloaded, total| {
                            let percent = if total > 0 { (downloaded * 100) / total } else { 0 };
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
                                        tracing::error!("update checksum verification failed: {}", e);
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

    // Spawn cron scheduler
    scheduler::spawn(
        state.store.clone(),
        state.runner.clone(),
        state.hub.clone(),
        state.snapshot_store.clone(),
        state.workflow_manager.clone(),
    );

    // Spawn heartbeat scheduler for per-entity heartbeats
    heartbeat::spawn(state.clone());

    // Build router
    // WebSocket routes are kept outside CompressionLayer — compression corrupts
    // the upgraded socket since it wraps the response body stream.
    let http_routes = Router::new()
        .route("/health", axum::routing::get(health_handler))
        .route("/server.json", axum::routing::get(spa::server_json))
        // MCP endpoint for CLI providers (Claude Code, Codex, Gemini)
        .route("/agent/mcp", axum::routing::post(handlers::mcp_server::agent_mcp_handler))
        // NeboLoop OAuth callback — top-level because the browser navigates here directly
        .route(
            "/auth/neboloop/callback",
            axum::routing::get(handlers::neboloop::oauth_callback),
        )
        .nest("/api/v1", routes::api_routes(jwt_secret)
            .layer(axum::middleware::from_fn(middleware::api_security_headers)))
        .fallback(spa::spa_handler)
        .layer(CompressionLayer::new());

    let app = Router::new()
        .route("/ws", axum::routing::get(handlers::ws::client_ws_handler))
        .route("/ws/extension", axum::routing::get(handlers::ws::extension_ws_handler))
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
        .with_state(state);

    if !quiet {
        info!("Server ready at http://localhost:{port}");
    }

    // Warn if non-loopback
    if host != "127.0.0.1" && host != "localhost" && host != "::1" {
        eprintln!("WARNING: Server binding to {bind_addr} — Nebo is designed for localhost-only access");
    }

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| NeboError::Server(format!("failed to bind: {e}")))?;

    axum::serve(listener, app)
        .await
        .map_err(|e| NeboError::Server(format!("server error: {e}")))?;

    Ok(())
}

/// Handle an incoming NeboLoop message with full access to runner/lanes/comm.
async fn handle_comm_message(state: AppState, msg: comm::CommMessage) {
    tracing::info!(
        topic = %msg.topic,
        from = %msg.from,
        conv_id = %msg.conversation_id,
        content_len = msg.content.len(),
        "handle_comm_message"
    );

    // Route account stream messages (plan changes, token refresh)
    if msg.topic == "account" {
        if let Ok(event) = serde_json::from_str::<serde_json::Value>(&msg.content) {
            if event.get("type").and_then(|t| t.as_str()) == Some("tokenRefresh") {
                if let Some(token) = event.get("token").and_then(|t| t.as_str()) {
                    let plan = event.get("plan").and_then(|p| p.as_str()).unwrap_or("free");
                    tracing::info!(plan = plan, "Account: plan updated via tokenRefresh");

                    // Persist fresh JWT to SQLite auth_profiles — next Janus request uses it
                    if let Ok(profiles) = state.store.list_all_active_auth_profiles_by_provider("neboloop") {
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
                    state.hub.broadcast("plan_changed", serde_json::json!({"plan": plan}));
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

    // Route agent space messages to the correct role
    if msg.topic == "agent_space" {
        let text = extract_message_text(&msg.content);
        if text.is_empty() {
            tracing::warn!(conv_id = %msg.conversation_id, "agent_space message with empty text, skipping");
            return;
        }

        let agent_slug = msg.metadata.get("agent_slug").cloned().unwrap_or_default();
        let agent_id = resolve_agent_id_from_slug(&state, &agent_slug).await;
        tracing::info!(
            agent_slug = %agent_slug,
            agent_id = %agent_id,
            text_len = text.len(),
            "agent_space: routing to role"
        );

        let session_key = agent::keyparser::build_session_key(
            "neboloop",
            "agent_space",
            &format!("{}:{}", agent_slug, msg.conversation_id),
        );

        // Pre-create chat with friendly title (agent name, not raw session key)
        let agent_name = {
            let registry = state.agent_registry.read().await;
            registry
                .get(&agent_id)
                .map(|r| r.name.clone())
                .unwrap_or_else(|| agent_slug.clone())
        };
        let _ = state.store.create_chat(&session_key, &format!("Agent: {}", agent_name));

        let preview = if text.len() > 80 { format!("{}...", truncate_str(&text, 80)) } else { text.clone() };
        notify_crate::send(&format!("Agent space: {}", agent_name), &preview);

        let entity_config = entity_config::resolve_for_chat(
            &state.store,
            "channel",
            "agent_space",
        );

        let config = chat_dispatch::ChatConfig {
            session_key,
            prompt: text,
            system: String::new(),
            user_id: String::new(),
            channel: "neboloop".to_string(),
            origin: tools::Origin::Comm,
            agent_id,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            lane: types::constants::lanes::COMM.to_string(),
            comm_reply: Some(chat_dispatch::CommReplyConfig {
                topic: "agent_space".to_string(),
                conversation_id: msg.conversation_id.clone(),
            }),
            entity_config,
            images: vec![],
        };

        chat_dispatch::run_chat(&state, config, None).await;

        state.event_bus.emit(tools::events::Event {
            source: format!("neboloop.agent_space.{}", agent_slug),
            payload: serde_json::json!({
                "from": msg.from,
                "content": msg.content,
                "conversation_id": msg.conversation_id,
                "agent_slug": agent_slug,
            }),
            origin: "neboloop".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
        return;
    }

    // Route chat and DM messages to the agent runner via unified chat pipeline
    if msg.topic == "chat" || msg.topic == "dm" {
        // Check if this conversation is actually an agent_space (gateway sends stream=dm for these)
        if let Some(agent_slug) = state.comm_manager.agent_slug_for_conv(&msg.conversation_id).await {
            let text = extract_message_text(&msg.content);
            if text.is_empty() {
                return;
            }
            let agent_id = resolve_agent_id_from_slug(&state, &agent_slug).await;
            tracing::info!(
                agent_slug = %agent_slug,
                agent_id = %agent_id,
                conv_id = %msg.conversation_id,
                "dm→agent_space reroute: conv belongs to agent space"
            );

            let session_key = agent::keyparser::build_session_key(
                "neboloop",
                "agent_space",
                &format!("{}:{}", agent_slug, msg.conversation_id),
            );

            let agent_name = {
                let registry = state.agent_registry.read().await;
                registry
                    .get(&agent_id)
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| agent_slug.clone())
            };
            let _ = state.store.create_chat(&session_key, &format!("Agent: {}", agent_name));

            let preview = if text.len() > 80 { format!("{}...", truncate_str(&text, 80)) } else { text.clone() };
            notify_crate::send(&format!("Agent space: {}", agent_name), &preview);

            let entity_config = entity_config::resolve_for_chat(
                &state.store,
                "channel",
                "agent_space",
            );

            let config = chat_dispatch::ChatConfig {
                session_key,
                prompt: text,
                system: String::new(),
                user_id: String::new(),
                channel: "neboloop".to_string(),
                origin: tools::Origin::Comm,
                agent_id,
                cancel_token: tokio_util::sync::CancellationToken::new(),
                lane: types::constants::lanes::COMM.to_string(),
                comm_reply: Some(chat_dispatch::CommReplyConfig {
                    topic: msg.topic.clone(),
                    conversation_id: msg.conversation_id.clone(),
                }),
                entity_config,
                images: vec![],
            };

            chat_dispatch::run_chat(&state, config, None).await;

            state.event_bus.emit(tools::events::Event {
                source: format!("neboloop.agent_space.{}", agent_slug),
                payload: serde_json::json!({
                    "from": msg.from,
                    "content": msg.content,
                    "conversation_id": msg.conversation_id,
                    "agent_slug": agent_slug,
                }),
                origin: "neboloop".to_string(),
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
        let preview = if text.len() > 80 { format!("{}...", truncate_str(&text, 80)) } else { text.clone() };
        notify_crate::send(&format!("Message from {}", msg.from), &preview);

        let session_key = agent::keyparser::build_session_key(
            "neboloop",
            &msg.topic,
            &msg.conversation_id,
        );

        // Resolve entity config for the channel
        let entity_config = entity_config::resolve_for_chat(
            &state.store,
            "channel",
            &msg.topic,
        );

        // Check for @mention routing — if agent_slug is present, resolve to agent_id
        let agent_id = {
            let agent_slug = msg.metadata.get("agent_slug").cloned().unwrap_or_default();
            resolve_agent_id_from_slug(&state, &agent_slug).await
        };

        // Pre-create chat with @mention context if applicable
        if !agent_id.is_empty() {
            let agent_slug = msg.metadata.get("agent_slug").cloned().unwrap_or_default();
            let _ = state.store.create_chat(&session_key, &format!("@{} (channel)", agent_slug));
        }

        let config = chat_dispatch::ChatConfig {
            session_key,
            prompt: text,
            system: String::new(),
            user_id: String::new(),
            channel: "neboloop".to_string(),
            origin: tools::Origin::Comm,
            agent_id,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            lane: types::constants::lanes::COMM.to_string(),
            comm_reply: Some(chat_dispatch::CommReplyConfig {
                topic: msg.topic.clone(),
                conversation_id: msg.conversation_id.clone(),
            }),
            entity_config,
            images: vec![],
        };

        chat_dispatch::run_chat(&state, config, None).await;

        // Also emit into event bus so role event triggers can fire
        state.event_bus.emit(tools::events::Event {
            source: format!("neboloop.{}", msg.topic),
            payload: serde_json::json!({
                "from": msg.from,
                "content": msg.content,
                "conversation_id": msg.conversation_id,
            }),
            origin: "neboloop".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
        return;
    }

    // Emit other message types into event bus for role triggers
    state.event_bus.emit(tools::events::Event {
        source: format!("neboloop.{}", msg.topic),
        payload: serde_json::json!({
            "from": msg.from,
            "content": msg.content,
            "topic": msg.topic,
        }),
        origin: "neboloop".to_string(),
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
async fn resolve_agent_id_from_slug(state: &AppState, slug: &str) -> String {
    if slug.is_empty() {
        return String::new();
    }
    let registry = state.agent_registry.read().await;
    for (id, role) in registry.iter() {
        if role.name.to_lowercase().replace(' ', "-") == slug {
            return id.clone();
        }
    }
    String::new()
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

async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        version: VERSION.into(),
    })
}

fn cors_layer() -> CorsLayer {
    use axum::http::HeaderValue;
    use tower_http::cors::AllowOrigin;

    let origins: Vec<HeaderValue> = [
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
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
            Method::PATCH,
        ])
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
            axum::http::header::ACCEPT,
            axum::http::header::ORIGIN,
        ])
        .allow_credentials(true)
}
