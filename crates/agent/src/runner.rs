use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use ai::{
    ChatRequest, Message, Provider, ProviderError, StreamEvent, StreamEventType,
};
use db::models::ChatMessage;
use db::Store;
use tools::{Origin, Registry, ToolContext, ToolResult};

use crate::concurrency::ConcurrencyController;
use crate::db_context;
use crate::dedupe::{self, DedupeCache};
use crate::keyparser;
use crate::memory;
use crate::prompt;
use crate::pruning::{self, ContextThresholds};
use crate::selector::{self, ModelSelector};
use crate::session::SessionManager;
use crate::steering;
use crate::tool_filter;

/// Default maximum agentic loop iterations per run.
const DEFAULT_MAX_ITERATIONS: usize = 100;
/// Extended ceiling when agent is making genuine progress (successful tool calls, no loops).
const EXTENDED_MAX_ITERATIONS: usize = 200;
/// Default context token limit for models that don't report one.
const DEFAULT_CONTEXT_TOKEN_LIMIT: usize = 80_000;
/// Max transient error retries before giving up.
const MAX_TRANSIENT_RETRIES: usize = 10;
/// Max retryable (provider/rate_limit/billing) retries before giving up.
const MAX_RETRYABLE_RETRIES: usize = 5;
/// Timeout for individual tool execution.
const TOOL_EXECUTION_TIMEOUT: Duration = Duration::from_secs(300);
/// Default max auto-continuations when agent stops mid-task (no work tasks).
const MAX_AUTO_CONTINUATIONS_DEFAULT: usize = 5;
/// Ceiling for auto-continuations even with many work tasks.
const MAX_AUTO_CONTINUATIONS_CEILING: usize = 50;
/// Max recovery attempts when output is truncated by token limit.
const MAX_OUTPUT_RECOVERY_ATTEMPTS: usize = 3;

/// Pick a non-gateway provider when available.  Falls back to first provider
/// (which may be Janus) only when no other option exists.  This prevents
/// background operations (memory extraction, compaction, summarisation) from
/// burning Janus credits when a CLI or direct-API provider is loaded.
fn prefer_non_gateway(providers: &[Arc<dyn Provider>]) -> Option<Arc<dyn Provider>> {
    providers.iter().find(|p| p.id() != "janus").cloned()
        .or_else(|| providers.first().cloned())
}

/// JSON shape for tool results stored in the DB. Includes optional image_url
/// so vision-capable providers can receive screenshots in tool result content.
#[derive(serde::Serialize)]
struct ToolResultRow {
    tool_call_id: String,
    content: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_url: Option<String>,
}

/// Input parameters for a run.
#[derive(Debug, Clone, Default)]
pub struct RunRequest {
    pub session_key: String,
    pub prompt: String,
    pub system: String,
    pub model_override: String,
    pub user_id: String,
    pub skip_memory_extract: bool,
    pub origin: Origin,
    pub channel: String,
    pub force_skill: String,
    /// Maximum agentic loop iterations (0 = default 100).
    pub max_iterations: usize,
    /// Cancellation token for cooperative shutdown of the agentic loop.
    pub cancel_token: CancellationToken,
    /// When set, this run executes as a specific agent (persona). The agent's persona
    /// replaces the default identity, and session history is isolated.
    pub agent_id: String,
    /// Per-entity permission overrides (tool category → allowed).
    pub permissions: Option<HashMap<String, bool>>,
    /// Per-entity resource grant overrides (resource → "allow"|"deny"|"inherit").
    pub resource_grants: Option<HashMap<String, String>>,
    /// Per-entity model preference (fuzzy-resolved before provider selection).
    pub model_preference: Option<String>,
    /// Per-entity personality snippet prepended to system prompt.
    pub personality_snippet: Option<String>,
    /// Images attached to the user's message (base64-encoded).
    pub images: Vec<ai::ImageContent>,
    /// Allowed filesystem paths — restricts file writes and shell commands to these directories.
    /// Empty = unrestricted.
    pub allowed_paths: Vec<String>,
    /// User presence tracker (shared Arc, for live updates during the run).
    pub presence_tracker: Option<Arc<crate::proactive::PresenceTracker>>,
    /// Proactive inbox (shared Arc, drained once per run).
    pub proactive_inbox: Option<Arc<crate::proactive::ProactiveInbox>>,
    /// Minimum iterations before allowing the agent to stop naturally.
    /// When set, the runner forces continuation even on text-only responses
    /// until this many iterations have been reached.
    pub min_iterations: usize,
    /// Optional progress counters shared with the global RunRegistry.
    /// When set, the runner updates these atomics during run_loop() so
    /// external observers can see live iteration/tool counts.
    pub progress: Option<RunProgress>,
}

/// Shared atomic counters for live run progress reporting.
/// Created by the server's RunRegistry and threaded into the runner.
#[derive(Clone, Debug)]
pub struct RunProgress {
    pub run_id: String,
    pub iteration_count: Arc<std::sync::atomic::AtomicU32>,
    pub tool_call_count: Arc<std::sync::atomic::AtomicU32>,
    pub current_tool: Arc<std::sync::Mutex<String>>,
}

/// Per-run mutable state (prevents data races across concurrent runs).
struct RunState {
    prompt_overhead: usize,
    last_input_tokens: usize,
    thresholds: Option<ContextThresholds>,
    /// Janus quota warning string, populated when session or weekly usage exceeds 80%.
    quota_warning: Option<String>,
    /// Whether a quota warning WS event has already been sent this run (fire once).
    quota_warning_sent: bool,
}

impl RunState {
    fn new() -> Self {
        Self {
            prompt_overhead: 0,
            last_input_tokens: 0,
            thresholds: None,
            quota_warning: None,
            quota_warning_sent: false,
        }
    }
}

/// The main agentic loop runner.
///
/// Providers are wrapped in `Arc` so they can be shared across concurrent runs
/// spawned via `tokio::spawn`.
pub struct Runner {
    sessions: SessionManager,
    providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    tools: Arc<Registry>,
    store: Arc<Store>,
    selector: Arc<ModelSelector>,
    _steering: steering::Pipeline,
    concurrency: Arc<ConcurrencyController>,
    hooks: Arc<napp::HookDispatcher>,
    mcp_context: Option<Arc<tokio::sync::Mutex<ToolContext>>>,
    agent_registry: tools::AgentRegistry,
    skill_loader: Option<Arc<tools::skills::Loader>>,
}

impl Runner {
    pub fn new(
        store: Arc<Store>,
        tools: Arc<Registry>,
        providers: Vec<Arc<dyn Provider>>,
        selector: ModelSelector,
        concurrency: Arc<ConcurrencyController>,
        hooks: Arc<napp::HookDispatcher>,
        mcp_context: Option<Arc<tokio::sync::Mutex<ToolContext>>>,
        agent_registry: tools::AgentRegistry,
        skill_loader: Option<Arc<tools::skills::Loader>>,
    ) -> Self {
        Self {
            sessions: SessionManager::new(store.clone()),
            providers: Arc::new(RwLock::new(providers)),
            tools,
            store,
            selector: Arc::new(selector),
            _steering: steering::Pipeline::new(),
            concurrency,
            hooks,
            mcp_context,
            agent_registry,
            skill_loader,
        }
    }

    /// Get the shared providers Arc (for workflow execution).
    pub fn providers(&self) -> Arc<RwLock<Vec<Arc<dyn Provider>>>> {
        self.providers.clone()
    }

    /// Replace the active providers list (called when auth_profiles change).
    pub async fn reload_providers(&self, providers: Vec<Arc<dyn Provider>>) {
        let loaded_ids: Vec<String> = providers.iter().map(|p| p.id().to_string()).collect();
        let mut lock = self.providers.write().await;
        let count = providers.len();
        *lock = providers;
        drop(lock);
        // Sync selector with newly loaded provider IDs
        self.selector.set_loaded_providers(loaded_ids);
        self.selector.rebuild_fuzzy(&std::collections::HashMap::new());
        info!(count, "reloaded AI providers");
    }

    /// Run the agentic loop: prompt -> stream -> tool calls -> loop.
    /// Returns a receiver of streaming events.
    pub async fn run(
        &self,
        req: RunRequest,
    ) -> Result<mpsc::Receiver<StreamEvent>, ProviderError> {
        info!(session_key = %req.session_key, channel = %req.channel, "Runner.run() called");
        {
            let lock = self.providers.read().await;
            if lock.is_empty() {
                warn!("No AI providers configured — rejecting run request");
                return Err(ProviderError::Request(
                    "No AI providers configured. Add API keys in Settings > Providers.".to_string(),
                ));
            }
            info!(provider_count = lock.len(), "providers available");
        }

        let session_key = if req.session_key.is_empty() {
            "default".to_string()
        } else {
            req.session_key.clone()
        };

        // Get or create session
        let session = self
            .sessions
            .get_or_create(&session_key, &req.user_id)
            .map_err(|e| {
                warn!(error = %e, "failed to get/create session");
                ProviderError::Request(format!("session error: {}", e))
            })?;

        let session_id = session.id.clone();
        info!(session_id = %session_id, "session ready");

        // Append user message — large inputs are offloaded to a temp file and
        // replaced with an LLM-generated summary so the full document never
        // enters the main chat context.
        if !req.prompt.is_empty() {
            let (effective_content, metadata) = if crate::large_input::is_large(&req.prompt) {
                info!(
                    session_id = %session_id,
                    prompt_len = req.prompt.len(),
                    "large input detected — saving to file and summarising"
                );

                let msg_id = uuid::Uuid::new_v4().to_string();

                // 1. Save full content to disk
                let file_path = crate::large_input::save_to_file(&req.prompt, &msg_id)
                    .map_err(|e| ProviderError::Request(format!("large input save: {e}")))?;
                let file_path_str = file_path.to_string_lossy().to_string();

                // 2. Detect content type for prompt tuning
                let content_type = crate::large_input::detect_content_type(&req.prompt);

                // 3. Summarise in an ISOLATED context (sidecar pattern).
                //    Acquire provider, drop lock, then call — the full text
                //    never touches the session or DB.
                let cheap_model = self.selector.get_cheapest_model();
                let summary = {
                    let prov = prefer_non_gateway(&self.providers.read().await);
                    match prov {
                        Some(p) => {
                            crate::large_input::summarize(
                                p.as_ref(),
                                &req.prompt,
                                content_type,
                                &cheap_model,
                            )
                            .await
                            .unwrap_or_else(|e| {
                                warn!(error = %e, "large input summarisation failed, using fallback");
                                crate::large_input::fallback_summary(&req.prompt)
                            })
                        }
                        None => crate::large_input::fallback_summary(&req.prompt),
                    }
                };

                // 4. Build replacement content + metadata
                let result = crate::large_input::build_replacement(
                    &req.prompt, &summary, &file_path_str, content_type,
                );

                // Merge with image metadata when both are present
                let mut meta_value: serde_json::Value =
                    serde_json::from_str(&result.metadata_json).unwrap_or_default();
                if !req.images.is_empty() {
                    meta_value["images"] = serde_json::json!(req.images);
                }

                info!(
                    session_id = %session_id,
                    summary_len = result.content.len(),
                    file = %file_path_str,
                    "large input replaced with summary"
                );

                (result.content, Some(meta_value.to_string()))
            } else {
                // Normal-sized prompt — pass through as-is
                let metadata = if !req.images.is_empty() {
                    Some(serde_json::json!({"images": req.images}).to_string())
                } else {
                    None
                };
                (req.prompt.clone(), metadata)
            };

            info!(session_id = %session_id, prompt_len = effective_content.len(), "appending user message");
            self.sessions.append_message(
                &session_id,
                "user",
                &effective_content,
                None,
                None,
                metadata.as_deref(),
            ).map_err(|e| {
                warn!(session_id = %session_id, error = %e, "failed to append user message");
                ProviderError::Request(format!("failed to store message: {}", e))
            })?;
        }

        // Create result channel
        let (tx, rx) = mpsc::channel(100);

        // Clone refs for the spawned task (SessionManager shares cache via Arc)
        let session_mgr = self.sessions.clone();
        let store = self.store.clone();
        let tools = self.tools.clone();
        let providers = self.providers.clone();
        let concurrency = self.concurrency.clone();
        let selector = self.selector.clone();
        let hooks = self.hooks.clone();
        let agent_registry = self.agent_registry.clone();
        let agent_id = req.agent_id.clone();
        let system_prompt = req.system.clone();
        let user_id = req.user_id.clone();
        let origin = req.origin;
        let skip_memory = req.skip_memory_extract;
        let user_prompt = req.prompt.clone();
        let force_skill = req.force_skill.clone();
        let skill_loader = self.skill_loader.clone();

        // Resolve fuzzy model override — prefer explicit model_override, fall back to entity preference
        let raw_model = if !req.model_override.is_empty() {
            req.model_override.clone()
        } else if let Some(ref pref) = req.model_preference {
            pref.clone()
        } else {
            String::new()
        };
        let model_override = if raw_model.is_empty() {
            String::new()
        } else {
            self.selector.resolve_fuzzy(&raw_model)
                .unwrap_or_else(|| raw_model.clone())
        };

        // Derive channel from session key via keyparser, fall back to explicit channel
        let channel = if !req.channel.is_empty() {
            req.channel.clone()
        } else {
            let key_info = keyparser::parse_session_key(&session_key);
            if key_info.channel.is_empty() { "web".to_string() } else { key_info.channel }
        };

        // Get model aliases for prompt injection
        let model_aliases = self.selector.get_aliases_text();

        let cancel_token = req.cancel_token.clone();
        let max_iterations = if req.max_iterations > 0 { req.max_iterations } else { DEFAULT_MAX_ITERATIONS };
        let min_iterations = req.min_iterations;
        let entity_permissions = req.permissions.clone();
        let entity_resource_grants = req.resource_grants.clone();
        let personality_snippet = req.personality_snippet.clone();
        let allowed_paths = req.allowed_paths.clone();
        let presence_tracker = req.presence_tracker.clone();
        let proactive_inbox = req.proactive_inbox.clone();
        let progress = req.progress.clone();

        // Set MCP context so CLI providers can access tools with the right session info
        if let Some(ref mcp_ctx) = self.mcp_context {
            let mut ctx = mcp_ctx.lock().await;
            ctx.session_key = session_key.clone();
            ctx.session_id = session_id.clone();
            ctx.origin = req.origin;
            ctx.user_id = req.user_id.clone();
        }

        tokio::spawn(async move {
            let steering = steering::Pipeline::new();

            let result = run_loop(
                &session_mgr,
                &tools,
                &store,
                &providers,
                &concurrency,
                &selector,
                &steering,
                &hooks,
                &tx,
                &session_id,
                &system_prompt,
                &model_override,
                &user_id,
                &channel,
                &model_aliases,
                origin,
                skip_memory,
                max_iterations,
                &cancel_token,
                &agent_registry,
                &agent_id,
                personality_snippet.as_deref(),
                entity_permissions.as_ref(),
                entity_resource_grants.as_ref(),
                &user_prompt,
                &force_skill,
                skill_loader.as_deref(),
                &allowed_paths,
                presence_tracker.as_ref(),
                proactive_inbox.as_ref(),
                min_iterations,
                progress.as_ref(),
            )
            .await;

            if let Err(e) = result {
                let _ = tx
                    .send(StreamEvent::error(format!("Agent error: {}", e)))
                    .await;
            }
            let _ = tx.send(StreamEvent::done()).await;
        });

        Ok(rx)
    }

    /// One-shot convenience: prompt -> response text (no tools).
    pub async fn chat(&self, prompt: &str) -> Result<String, ProviderError> {
        let prov_lock = self.providers.read().await;
        if prov_lock.is_empty() {
            return Err(ProviderError::Request("No providers configured".to_string()));
        }

        let req = ChatRequest {
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt.to_string(),
                ..Default::default()
            }],
            tools: vec![],
            max_tokens: 4096,
            temperature: 0.7,
            system: String::new(),
            static_system: String::new(),
            model: String::new(),
            enable_thinking: false,
            metadata: None,
            cache_breakpoints: vec![],
            cancel_token: None,
        };

        let mut rx = prov_lock[0].stream(&req).await?;
        drop(prov_lock); // Release lock before consuming stream
        let mut response = String::new();

        while let Some(event) = rx.recv().await {
            if event.event_type == StreamEventType::Text {
                response.push_str(&event.text);
            }
        }

        Ok(response)
    }

    pub fn sessions(&self) -> &SessionManager {
        &self.sessions
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// Get the number of active providers (blocking read for sync contexts).
    pub fn provider_count(&self) -> usize {
        // Use try_read to avoid blocking; fall back to 0 if locked
        match self.providers.try_read() {
            Ok(lock) => lock.len(),
            Err(_) => 0,
        }
    }
}

/// The main agentic loop, running as an async task.
#[allow(clippy::too_many_arguments)]
async fn run_loop(
    sessions: &SessionManager,
    tools: &Arc<Registry>,
    store: &Arc<Store>,
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    concurrency: &Arc<ConcurrencyController>,
    selector: &ModelSelector,
    steering_pipeline: &steering::Pipeline,
    hooks: &napp::HookDispatcher,
    tx: &mpsc::Sender<StreamEvent>,
    session_id: &str,
    system_prompt: &str,
    model_override: &str,
    user_id: &str,
    channel: &str,
    model_aliases: &str,
    origin: Origin,
    skip_memory: bool,
    max_iterations: usize,
    cancel_token: &CancellationToken,
    agent_registry: &tools::AgentRegistry,
    agent_id: &str,
    personality_snippet: Option<&str>,
    entity_permissions: Option<&HashMap<String, bool>>,
    entity_resource_grants: Option<&HashMap<String, String>>,
    user_prompt: &str,
    force_skill: &str,
    skill_loader: Option<&tools::skills::Loader>,
    allowed_paths: &[String],
    presence_tracker: Option<&Arc<crate::proactive::PresenceTracker>>,
    proactive_inbox: Option<&Arc<crate::proactive::ProactiveInbox>>,
    min_iterations: usize,
    progress: Option<&RunProgress>,
) -> Result<(), String> {
    let mut state = RunState::new();
    let mut transient_retries = 0usize;
    let mut retryable_retries = 0usize;
    let mut called_tools: Vec<String> = Vec::new();
    // Rolling hashes of recent tool results for stale-result detection in steering
    let mut recent_tool_result_hashes: Vec<(u64, u64, u64)> = Vec::new();
    // Parallel vec of tool names (same indexing as recent_tool_result_hashes)
    let mut recent_tool_names: Vec<String> = Vec::new();
    let mut provider_idx: usize = 0;
    // Janus provider metadata for tool stickiness — echoed back in subsequent requests
    let mut sticky_metadata: Option<std::collections::HashMap<String, String>> = None;
    let mut auto_continuations = 0usize;
    // Cycle detection: track last auto-continued response to break loops
    let mut prev_auto_content: Option<String> = None;
    // Sticky flag: once user demands action ("don't stop", "do them all"), stays true for the run
    let mut user_demanded_action_sticky = user_demanded_action(&sessions.get_messages(session_id).unwrap_or_default());
    // Cache for tool documentation (help/schema results) — survives sliding window eviction
    // via injection into the dynamic suffix. Max 5 entries, LRU-evict oldest.
    let mut tool_doc_cache: Vec<(String, String)> = Vec::new();
    const MAX_TOOL_DOC_ENTRIES: usize = 5;
    const MAX_TOOL_DOC_CONTENT: usize = 4_000;
    let mut output_recovery_attempts = 0usize;
    let mut continuation_steering: Option<String> = None;
    let mut consecutive_error_iterations = 0usize;
    // Deferred tools that have been activated (keyword-matched or first-called)
    let mut activated_deferred: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Resolve agent from registry if agent_id is set
    let active_agent_entry = if !agent_id.is_empty() {
        let reg = agent_registry.read().await;
        reg.get(agent_id).cloned()
    } else {
        None
    };

    // Scope memory by agent: each agent gets its own memory namespace to prevent cross-contamination.
    // Main bot uses the raw user_id; agents use "user_id:agent:agent_id".
    let memory_user_id = if !agent_id.is_empty() {
        format!("{}:agent:{}", user_id, agent_id)
    } else {
        user_id.to_string()
    };

    // Load rich DB context (agent profile, user profile, personality directive, scored memories)
    let db_ctx = db_context::load_db_context(store, &memory_user_id);

    // Extract user-configured timezone for date/time in the dynamic suffix
    let user_timezone = db_ctx.user.as_ref()
        .and_then(|u| u.timezone.clone())
        .filter(|tz| !tz.is_empty());

    // If running as an agent (persona), use the agent name as agent_name
    let agent_name = if let Some(ref agent) = active_agent_entry {
        agent.name.clone()
    } else {
        db_ctx
            .agent
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Nebo".to_string())
    };
    let mut db_context_formatted = db_context::format_for_system_prompt(&db_ctx, &agent_name);

    // Pre-load prompt-relevant memories via FTS (surfaces memories the decay scoring may miss)
    if !user_prompt.is_empty() {
        let existing_ids: std::collections::HashSet<i64> =
            db_ctx.tacit_memories.iter().map(|sm| sm.memory.id).collect();
        let relevant = db_context::load_prompt_relevant_memories(
            store, &memory_user_id, user_prompt, &existing_ids,
        );
        if !relevant.is_empty() {
            db_context_formatted.push_str(&relevant);
        }
    }

    // Get active task (mutable: refreshed periodically to catch async detect_objective)
    let mut active_task = sessions
        .get_active_task(session_id)
        .unwrap_or_default();

    // Match skills against user prompt (force_skill overrides trigger matching)
    // Template variables (${NEBO_SKILL_DIR}, ${NEBO_DATA_DIR}, etc.) are expanded at activation.
    // The compact skill catalog (always-present listing) replaces old keyword-triggered hints.
    // Full skill bodies are only injected when: forced, trigger-matched, or agent-declared.
    let mut active_skill_template = if let Some(loader) = skill_loader {
        if !force_skill.is_empty() {
            match loader.get(force_skill).await {
                Some(skill) if skill.enabled => {
                    info!(skill = %skill.name, "force-activated skill");
                    Some(loader.expand_template(&skill, Some(store)))
                }
                _ => {
                    warn!(force_skill, "forced skill not found or disabled");
                    None
                }
            }
        } else if !user_prompt.is_empty() {
            let matches = loader.match_triggers(user_prompt, 3).await;
            if let Some(best) = matches.first() {
                info!(skill = %best.name, priority = best.priority, "skill matched via trigger");
                Some(loader.expand_template(best, Some(store)))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // Auto-load agent-declared skills so the LLM knows about plugin tools and
    // usage patterns from SKILL.md content. Without this, agents fall back to
    // native tools (e.g. organizer:mail) instead of using declared plugins.
    if let (Some(agent_entry), Some(loader)) = (&active_agent_entry, skill_loader) {
        if let Some(cfg) = &agent_entry.config {
            let mut agent_skill_parts: Vec<String> = Vec::new();
            for skill_ref in &cfg.skills {
                let skill_name = extract_skill_name(skill_ref);
                if let Some(skill) = loader.get(&skill_name).await {
                    if skill.enabled {
                        let expanded = loader.expand_template(&skill, Some(store));
                        if !expanded.is_empty() {
                            agent_skill_parts.push(expanded);
                        }
                    }
                }
            }
            if !agent_skill_parts.is_empty() {
                let combined = agent_skill_parts.join("\n\n---\n\n");
                active_skill_template = Some(match active_skill_template {
                    Some(existing) => format!("{}\n\n---\n\n{}", existing, combined),
                    None => combined,
                });
                info!(
                    agent = %agent_entry.name,
                    count = agent_skill_parts.len(),
                    "auto-loaded agent-declared skills"
                );
            }
        }
    }

    // Build static system prompt — use modular prompt when no custom one is provided
    // STRAP docs and tool list are NOT included here — they're injected per-iteration
    // based on which tools pass the context filter (dynamic injection).
    let active_agent_body = active_agent_entry.as_ref().map(|r| {
        // Strip YAML frontmatter from AGENT.md — inject only the prose body.
        // Frontmatter is machine metadata (name, triggers, etc.), not persona instructions.
        match napp::agent::split_frontmatter(&r.agent_md) {
            Ok((yaml_str, body)) => {
                if yaml_str.is_empty() {
                    body
                } else {
                    // Include a compact identity header from frontmatter properties
                    let mut result = String::new();
                    if let Ok(mapping) = serde_yaml::from_str::<serde_yaml::Mapping>(&yaml_str) {
                        let mut identity_parts = Vec::new();
                        for (k, v) in &mapping {
                            if let (serde_yaml::Value::String(key), val) = (k, v) {
                                match key.as_str() {
                                    "name" | "description" | "triggers" => {
                                        let val_str = match val {
                                            serde_yaml::Value::String(s) => s.clone(),
                                            serde_yaml::Value::Sequence(seq) => {
                                                seq.iter().filter_map(|i| match i {
                                                    serde_yaml::Value::String(s) => Some(s.as_str()),
                                                    _ => None,
                                                }).collect::<Vec<_>>().join(", ")
                                            }
                                            _ => continue,
                                        };
                                        identity_parts.push(format!("- **{}**: {}", key, val_str));
                                    }
                                    _ => {}
                                }
                            }
                        }
                        if !identity_parts.is_empty() {
                            result.push_str(&identity_parts.join("\n"));
                            result.push_str("\n\n");
                        }
                    }
                    result.push_str(&body);
                    result
                }
            }
            Err(_) => r.agent_md.clone(),
        }
    });
    let plugin_inventory = skill_loader
        .as_ref()
        .map(|l| l.plugin_inventory())
        .unwrap_or_default();

    // Build compact skill catalog (replaces old keyword-triggered skill_hints).
    // The full skill body is now in active_skill_template (agent-declared skills)
    // or loaded on-demand via the skill tool.
    let skill_catalog = if let Some(loader) = skill_loader {
        loader.compact_catalog().await
    } else {
        String::new()
    };

    let static_system = if system_prompt.is_empty() {
        let pctx = prompt::PromptContext {
            agent_name: agent_name.clone(),
            active_skill: active_skill_template,
            skill_catalog,
            model_aliases: model_aliases.to_string(),
            channel: channel.to_string(),
            platform: std::env::consts::OS.to_string(),
            memory_context: String::new(),
            db_context: Some(db_context_formatted.clone()),
            active_agent: active_agent_body,
            plugin_inventory,
        };
        prompt::build_static(&pctx)
    } else {
        build_system_prompt(system_prompt, &db_context_formatted)
    };

    // Prepend personality snippet if provided by entity config
    let static_system = if let Some(snippet) = personality_snippet {
        if snippet.is_empty() {
            static_system
        } else {
            format!("{}\n\n{}", snippet, static_system)
        }
    } else {
        static_system
    };

    // Record run start time for sliding window protection
    let run_start_time = chrono::Utc::now().timestamp();

    // Fire objective detection in background (non-blocking).
    // Acquires an LLM permit so it doesn't steal provider capacity from the main request.
    {
        let providers = providers.clone();
        let store = store.clone();
        let conc = concurrency.clone();
        let session_id = session_id.to_string();
        let user_prompt = sessions.get_messages(&session_id)
            .ok()
            .and_then(|msgs| msgs.iter().rev().find(|m| m.role == "user").map(|m| m.content.clone()))
            .unwrap_or_default();
        tokio::spawn(async move {
            let _permit = conc.acquire_llm_permit().await;
            let session_mgr = SessionManager::new(store);
            detect_objective(&providers, &session_mgr, &session_id, &user_prompt).await;
        });
    }

    // Use the extended ceiling for the loop range; adaptive check below enforces
    // the default limit unless the agent is making genuine progress.
    let hard_ceiling = max_iterations.max(EXTENDED_MAX_ITERATIONS);

    for iteration in 1..=hard_ceiling {
        // Update progress counter for external observers (RunRegistry dashboard)
        if let Some(p) = progress {
            p.iteration_count.store(iteration as u32, std::sync::atomic::Ordering::Relaxed);
        }

        if cancel_token.is_cancelled() {
            info!(session_id, "run cancelled before iteration {}", iteration);
            return Ok(());
        }

        // Adaptive iteration limit: extend past default only if making genuine progress
        if iteration > max_iterations && iteration <= hard_ceiling {
            if consecutive_error_iterations >= 2
                || steering::should_force_break(&steering::Context {
                    session_id: session_id.to_string(),
                    messages: sessions.get_messages(session_id).unwrap_or_default(),
                    user_prompt: user_prompt.to_string(),
                    active_task: active_task.clone(),
                    channel: channel.to_string(),
                    agent_name: "Nebo".to_string(),
                    iteration,
                    work_tasks: vec![],
                    quota_warning: None,
                    consecutive_error_iterations,
                    recent_tool_result_hashes: recent_tool_result_hashes.clone(),
                    recent_tool_names: recent_tool_names.clone(),
                    user_presence: String::new(),
                    user_just_returned: false,
                    proactive_items: vec![],
                    provider_id: String::new(),
                }).is_some()
            {
                info!(session_id, iteration, "adaptive limit: stopping, no progress");
                break;
            }
            if iteration == max_iterations + 1 {
                info!(session_id, "adaptive limit: extending past {} (making progress)", max_iterations);
            }
        }

        // agent.should_continue filter — let apps dynamically stop the agent
        if hooks.has_subscribers("agent.should_continue") {
            let payload = serde_json::to_vec(&crate::hooks::ShouldContinuePayload {
                session_id: session_id.to_string(),
                turn: iteration,
                total_tool_calls: called_tools.clone(),
                has_active_task: !active_task.is_empty(),
            })
            .unwrap_or_default();
            let (result, _) = hooks.apply_filter("agent.should_continue", payload).await;
            if let Ok(resp) = serde_json::from_slice::<crate::hooks::ShouldContinueResponse>(&result) {
                if !resp.should_continue {
                    info!(session_id, turn = iteration, reason = ?resp.reason, "hook requested stop");
                    break;
                }
            }
        }

        info!(iteration, session_id, "agentic loop iteration");

        // Load messages from session, then sanitize ordering.
        // Matches Go's sanitizeAgentMessages: strips orphaned tool results and
        // ensures tool results immediately follow their assistant message.
        let all_messages = sanitize_message_order(
            sessions
                .get_messages(session_id)
                .map_err(|e| format!("failed to load messages: {}", e))?,
        );

        // Refresh active_task from DB periodically to catch:
        // 1. Background detect_objective() completing after initial read
        // 2. Task updates from tool calls (bot:task:update)
        if iteration <= 5 || iteration % 10 == 0 {
            let refreshed = sessions.get_active_task(session_id).unwrap_or_default();
            if !refreshed.is_empty() && refreshed != active_task {
                info!(session_id, iteration, old = %active_task, new = %refreshed, "active_task refreshed from DB");
                active_task = refreshed;
            }
        }

        // Refresh sticky demand flag — once set, stays true for the entire run
        if !user_demanded_action_sticky {
            user_demanded_action_sticky = user_demanded_action(&all_messages);
        }

        if all_messages.is_empty() {
            let chat_id = sessions.resolve_session_key(session_id)
                .unwrap_or_else(|_| format!("(unresolved, fallback=chat-{})", session_id));
            warn!(
                session_id,
                chat_id = %chat_id,
                "No messages in session — session_key may not have been cached"
            );
            return Err(format!("No messages in session (session_id={}, chat_id={})", session_id, chat_id));
        }

        // Compute prompt overhead on first iteration
        if iteration == 1 {
            let system_tokens = static_system.len() / 4;
            let tool_defs = tools.list().await;
            let schema_tokens: usize = tool_defs
                .iter()
                .map(|t| (t.description.len() + t.input_schema.to_string().len()) / 4)
                .sum();
            state.prompt_overhead = system_tokens + schema_tokens + 4000;
        }

        // Compute context thresholds — use model's actual context window when
        // available so large-context providers (200K Claude, 128K GPT-4o) aren't
        // under-utilized.  Falls back to DEFAULT_CONTEXT_TOKEN_LIMIT (80K).
        let thresholds = state.thresholds.get_or_insert_with(|| {
            let model_ctx = if !model_override.is_empty() {
                selector.get_model_info(model_override)
                    .map(|m| m.context_window as usize)
                    .filter(|&w| w > 0)
            } else {
                let default_model = selector.select(&[]);
                if !default_model.is_empty() {
                    selector.get_model_info(&default_model)
                        .map(|m| m.context_window as usize)
                        .filter(|&w| w > 0)
                } else {
                    None
                }
            };
            let context_window = model_ctx.unwrap_or(DEFAULT_CONTEXT_TOKEN_LIMIT);
            ContextThresholds::from_context_window(context_window, state.prompt_overhead)
        });

        // Apply sliding window — token-only threshold (no message count limit).
        // auto_compact is ~80% of effective context window, so eviction only
        // fires when approaching the limit (Claude Code's proven approach).
        let (mut window_messages, evicted) =
            pruning::apply_sliding_window(&all_messages, run_start_time, thresholds.auto_compact);

        // Build rolling summary if we evicted messages.
        // Quick fallback is used immediately (no LLM call); the LLM-quality
        // summary is generated in the background and stored for next iteration.
        let summary = if !evicted.is_empty() {
            let existing_summary = sessions.get_summary(session_id).unwrap_or_default();

            // Immediate: quick fallback (pure string extraction, no LLM)
            let quick = pruning::build_quick_fallback_summary(&evicted, &active_task);
            let immediate_summary = if existing_summary.is_empty() {
                quick
            } else {
                format!("{}\n\n{}", existing_summary, quick)
            };
            let _ = sessions.update_summary(session_id, &immediate_summary);

            // Background: fire LLM summary, store when done (non-blocking)
            let cheap_model = selector.get_cheapest_model();
            let prov = prefer_non_gateway(&providers.read().await);
            if let Some(prov) = prov {
                let sess = sessions.clone();
                let sid = session_id.to_string();
                let task = active_task.clone();
                let existing = existing_summary.clone();
                let conc = concurrency.clone();
                tokio::spawn(async move {
                    let _permit = conc.acquire_llm_permit().await;
                    match pruning::build_llm_summary(
                        prov.as_ref(),
                        &evicted,
                        &existing,
                        &task,
                        &cheap_model,
                    )
                    .await
                    {
                        Ok(s) => {
                            let _ = sess.update_summary(&sid, &s);
                        }
                        Err(e) => {
                            debug!(error = %e, "background LLM compaction failed");
                        }
                    }
                });
            }

            immediate_summary
        } else {
            sessions.get_summary(session_id).unwrap_or_default()
        };

        // Time-based micro-compact: clear stale tool results when user
        // returns after inactivity (cache is cold, no point re-processing).
        let (tb_messages, tb_saved) = pruning::time_based_micro_compact(
            &window_messages,
            pruning::TIME_BASED_KEEP_RECENT,
            pruning::TIME_BASED_GAP_THRESHOLD_SECS,
        );
        if tb_saved > 0 {
            debug!(tokens_saved = tb_saved, "Time-based micro-compact fired");
            window_messages = tb_messages;
        }

        // Micro-compact tool results if needed
        let (compacted_messages, _tokens_saved) =
            pruning::micro_compact(&window_messages, thresholds.warning);
        window_messages = compacted_messages;

        // Detect which deferred tools should be activated this iteration (keyword match or prior call)
        let deferred_names = tools.get_deferred_names().await;
        let new_activations = tool_filter::detect_deferred_activations(
            &window_messages, &called_tools, &deferred_names, &activated_deferred,
        );
        if !new_activations.is_empty() {
            debug!(tools = ?new_activations, "activating deferred tools");
            activated_deferred.extend(new_activations);
        }

        // Get tool definitions: active (non-deferred + activated deferred) tools get full schemas
        let all_tool_defs = tools.list_active(&activated_deferred).await;
        let (tool_defs, active_contexts) = tool_filter::filter_tools_with_context(&all_tool_defs, &window_messages, &called_tools);

        // Parse work tasks for steering
        let work_tasks_json = sessions.get_work_tasks(session_id).unwrap_or_default();
        let work_tasks: Vec<steering::WorkTask> = serde_json::from_str(&work_tasks_json)
            .unwrap_or_default();

        // Resolve user presence for steering (live from shared tracker)
        let (user_presence, user_just_returned) = if let Some(tracker) = presence_tracker {
            let p = tracker.get("_global").await;
            let jr = tracker.just_returned("_global").await;
            (
                p.map(|p| p.as_str().to_string()).unwrap_or_default(),
                jr,
            )
        } else {
            (String::new(), false)
        };

        // Drain proactive inbox on first iteration only
        let proactive_items = if iteration == 1 {
            if let Some(inbox) = proactive_inbox {
                inbox.drain(session_id).await
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        // Build per-iteration STRAP docs + tool list based on filtered tools
        let filtered_tool_names: Vec<String> = tool_defs.iter().map(|t| t.name.clone()).collect();
        let strap_section = prompt::build_strap_section(&filtered_tool_names, &active_contexts);
        let tools_list = prompt::build_tools_list(&filtered_tool_names);

        // Build compact listing of deferred (not yet activated) tools
        let deferred_stubs = tools.list_deferred_stubs(&activated_deferred).await;
        let deferred_listing = prompt::build_deferred_listing(&deferred_stubs);

        // Select model: use override if set, otherwise ask the selector
        let selected_model = if !model_override.is_empty() {
            model_override.to_string()
        } else {
            selector.select(&window_messages)
        };

        // Determine thinking mode
        let enable_thinking = if !selected_model.is_empty() {
            let task = selector.classify_task(&window_messages);
            task == selector::TaskType::Reasoning && selector.supports_thinking(&selected_model)
        } else {
            false
        };

        // Parse selected model to find the right provider
        let (selected_provider_id, selected_model_name) = if selected_model.is_empty() {
            ("", "")
        } else {
            selector::parse_model_id(&selected_model)
        };

        // Generate steering directives (provider_id needed for skip rules)
        let steering_ctx = steering::Context {
            session_id: session_id.to_string(),
            messages: window_messages.clone(),
            user_prompt: user_prompt.to_string(),
            active_task: active_task.clone(),
            channel: channel.to_string(),
            agent_name: "Nebo".to_string(),
            iteration,
            work_tasks: work_tasks.clone(),
            quota_warning: state.quota_warning.clone(),
            consecutive_error_iterations,
            recent_tool_result_hashes: recent_tool_result_hashes.clone(),
            recent_tool_names: recent_tool_names.clone(),
            user_presence,
            user_just_returned,
            proactive_items,
            provider_id: selected_provider_id.to_string(),
        };

        // Circuit-breaker: check if the loop must be force-broken before making the next LLM call
        if let Some(reason) = steering::should_force_break(&steering_ctx) {
            warn!(session_id, iteration, reason = %reason, "circuit breaker triggered");
            let _ = tx.send(StreamEvent {
                event_type: StreamEventType::Text,
                text: format!(
                    "\n\nI got stuck in a loop and the circuit breaker stopped me. {}\n\n\
                     I apologize for the repeated attempts. Please let me know how you'd like to proceed.",
                    reason
                ),
                tool_call: None,
                error: None,
                usage: None,
                rate_limit: None,
                widgets: None,
                provider_metadata: None,
                stop_reason: None,
            }).await;
            break;
        }

        let (mut all_directives, proactive_context) = steering_pipeline.generate(&steering_ctx);

        // Hook: steering.generate — let apps inject additional directives
        if hooks.has_subscribers("steering.generate") {
            let payload = serde_json::to_vec(&crate::hooks::SteeringGeneratePayload {
                session_id: session_id.to_string(),
                iteration,
            })
            .unwrap_or_default();
            let (result, _) = hooks.apply_filter("steering.generate", payload).await;
            if let Ok(resp) = serde_json::from_slice::<crate::hooks::SteeringGenerateResponse>(&result) {
                for d in resp.directives {
                    all_directives.push(steering::SteeringDirective {
                        label: d.label,
                        content: d.content,
                        priority: d.priority,
                    });
                }
            }
        }

        // Append one-shot continuation steering (from auto-continue, never persisted)
        if let Some(cont) = continuation_steering.take() {
            all_directives.push(steering::SteeringDirective {
                label: "Continue".to_string(),
                content: cont,
                priority: 8,
            });
        }

        // Convert ChatMessage to ai::Message (no steering injection — steering goes in system prompt)
        let ai_messages = convert_messages(&window_messages);

        // Format steering for system prompt suffix
        let steering_text = steering::format_directives(&all_directives);
        let proactive_text = if proactive_context.is_empty() {
            String::new()
        } else {
            proactive_context.join("\n")
        };

        // Build dynamic system suffix — AFTER model selection so identity is accurate
        let dctx = prompt::DynamicContext {
            provider_name: selected_provider_id.to_string(),
            model_name: selected_model_name.to_string(),
            active_task: active_task.clone(),
            summary: summary.clone(),
            neboloop_connected: channel == "neboloop",
            channel: channel.to_string(),
            work_tasks: work_tasks.clone(),
            tool_doc_cache: tool_doc_cache.clone(),
            steering_directives: steering_text,
            proactive_context: proactive_text,
            user_timezone: user_timezone.clone(),
        };
        let dynamic_suffix = prompt::build_dynamic_suffix(&dctx);

        let full_system = if deferred_listing.is_empty() {
            format!("{}\n\n{}\n\n{}{}", static_system, strap_section, tools_list, dynamic_suffix)
        } else {
            format!("{}\n\n{}\n\n{}\n\n{}{}", static_system, strap_section, tools_list, deferred_listing, dynamic_suffix)
        };

        // Hook: message.pre_send — let apps modify system prompt before LLM call
        let full_system = if hooks.has_subscribers("message.pre_send") {
            let payload = serde_json::to_vec(&crate::hooks::PreSendPayload {
                system_prompt: full_system.clone(),
                message_count: ai_messages.len(),
            })
            .unwrap_or_default();
            let (result, _) = hooks.apply_filter("message.pre_send", payload).await;
            match serde_json::from_slice::<crate::hooks::PreSendResponse>(&result) {
                Ok(resp) => resp
                    .system_prompt
                    .filter(|s| !s.is_empty())
                    .unwrap_or(full_system),
                Err(_) => full_system,
            }
        } else {
            full_system
        };

        // Compute cache breakpoints for providers that support prompt caching.
        // Breakpoint 1: CACHE_BOUNDARY within static_system (stable identity/behaviour — rarely changes)
        // Breakpoint 2: end of static_system (semi-dynamic: skill hints, model aliases)
        // Everything after breakpoint 2 (STRAP, tools list, dynamic suffix) is fully dynamic.
        let cache_breakpoints = {
            let mut bps = Vec::new();
            if let Some(boundary) = prompt::cache_boundary_offset(&static_system) {
                bps.push(boundary);
            }
            let static_len = static_system.len();
            if static_len > 0
                && (bps.is_empty() || *bps.last().unwrap() < static_len)
            {
                bps.push(static_len);
            }
            bps
        };

        // Build ChatRequest
        let chat_req = ChatRequest {
            messages: ai_messages,
            tools: tool_defs,
            max_tokens: 4096,
            temperature: 0.7,
            system: full_system,
            static_system: static_system.clone(),
            model: if selected_model_name.is_empty() {
                String::new()
            } else {
                selected_model_name.to_string()
            },
            enable_thinking,
            metadata: sticky_metadata.clone(),
            cache_breakpoints,
            cancel_token: Some(cancel_token.clone()),
        };

        // Acquire LLM permit before provider call (blocks if at capacity)
        let _llm_permit = tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(session_id, "run cancelled waiting for LLM permit");
                return Ok(());
            }
            permit = concurrency.acquire_llm_permit() => permit,
        };

        // Snapshot provider from lock, then release before I/O
        let provider = {
            let prov_lock = providers.read().await;
            if prov_lock.is_empty() {
                return Err("No AI providers available".to_string());
            }

            // Find provider: use model-based lookup on first attempt,
            // but after retries (provider_idx > 0) use round-robin so we
            // actually fall through to the next provider (e.g. CLI agent).
            let idx = if provider_idx > 0 {
                provider_idx % prov_lock.len()
            } else if !selected_provider_id.is_empty() {
                prov_lock
                    .iter()
                    .position(|p| p.id() == selected_provider_id)
                    .unwrap_or(0)
            } else {
                0
            };

            info!(
                iteration,
                session_id,
                provider_idx = idx,
                provider_id = prov_lock[idx].id(),
                selected_provider_id,
                selected_model = %selected_model,
                provider_count = prov_lock.len(),
                message_count = chat_req.messages.len(),
                tool_count = chat_req.tools.len(),
                enable_thinking,
                "sending request to provider"
            );
            prov_lock[idx].clone()
        };

        // If we fell through to a different provider (e.g. CLI after Janus rate limit),
        // clear the model so the fallback provider uses its own default.
        let mut chat_req = chat_req;
        if provider.id() != selected_provider_id {
            chat_req.model = String::new();
        }

        let stream_result = tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(session_id, "run cancelled during provider.stream() call");
                return Ok(());
            }
            result = provider.stream(&chat_req) => result,
        };

        let mut rx = match stream_result {
            Ok(rx) => {
                info!(iteration, session_id, "provider stream started");
                rx
            }
            Err(e) => {
                // Deduplicate repeated errors to avoid log spam
                let err_str = format!("{}", e);
                let fingerprint = dedupe::fingerprint_error(&err_str);
                static ERROR_DEDUP: std::sync::OnceLock<DedupeCache> = std::sync::OnceLock::new();
                let dedup = ERROR_DEDUP.get_or_init(DedupeCache::default);
                if dedup.check(&fingerprint) {
                    debug!(iteration, session_id, "deduplicated provider error");
                } else {
                    warn!(iteration, session_id, error = %e, "provider error");
                }

                if ai::is_context_overflow(&e) {
                    warn!("context overflow, reducing window");
                    continue;
                }

                if ai::is_transient_error(&e) {
                    transient_retries += 1;
                    selector.mark_failed(&selected_model);
                    if transient_retries > MAX_TRANSIENT_RETRIES {
                        return Err(format!("Too many transient errors: {}", e));
                    }
                    // Try next provider on transient error — but never
                    // silently fall from CLI to Janus (burns Nebo credits).
                    let prov_lock = providers.read().await;
                    let prov_count = prov_lock.len();
                    if prov_count > 1 {
                        let next_idx = (provider_idx + 1) % prov_count;
                        if prov_lock[next_idx].id() == "janus" {
                            drop(prov_lock);
                            return Err(format!("Provider error (no fallback to Janus): {}", e));
                        }
                        drop(prov_lock);
                        provider_idx += 1;
                    } else {
                        drop(prov_lock);
                    }
                    tokio::select! {
                        _ = cancel_token.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    }
                    continue;
                }

                if e.is_retryable() {
                    retryable_retries += 1;
                    selector.mark_failed(&selected_model);
                    if retryable_retries > MAX_RETRYABLE_RETRIES {
                        return Err(format!("Service temporarily unavailable after {} retries: {}", MAX_RETRYABLE_RETRIES, e));
                    }
                    let prov_lock = providers.read().await;
                    let prov_count = prov_lock.len();
                    if prov_count > 1 {
                        let next_idx = (provider_idx + 1) % prov_count;
                        if prov_lock[next_idx].id() == "janus" {
                            drop(prov_lock);
                            return Err(format!("Provider error (no fallback to Janus): {}", e));
                        }
                        drop(prov_lock);
                        provider_idx += 1;
                    } else {
                        drop(prov_lock);
                    }
                    tokio::select! {
                        _ = cancel_token.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    }
                    continue;
                }

                return Err(format!("Provider error: {}", e));
            }
        };

        // Process stream (retry counters are reset after stream produces content)
        let mut assistant_content = String::new();
        let mut tool_calls: Vec<ai::ToolCall> = Vec::new();
        let mut stream_error: Option<String> = None;
        let mut stop_reason: Option<String> = None;
        // Track the order of content blocks (text vs tool) for correct rehydration.
        // Each entry is either "text" (coalesced) or a tool index.
        let mut block_order: Vec<(&str, Option<usize>)> = Vec::new();
        // CLI providers run multi-turn tool loops — save each turn incrementally.
        let cli_incremental = provider.handles_tools();

        loop {
            let event = tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!(session_id, "run cancelled during LLM stream");
                    // Best-effort: save whatever content we accumulated before cancellation
                    if !assistant_content.is_empty() || !tool_calls.is_empty() {
                        let tc_json = if !tool_calls.is_empty() {
                            serde_json::to_string(&tool_calls).ok()
                        } else {
                            None
                        };
                        if let Err(e) = sessions.append_message(
                            session_id, "assistant", &assistant_content,
                            tc_json.as_deref(), None, None,
                        ) {
                            warn!(session_id = %session_id, error = %e, "failed to save partial assistant message on cancel");
                        } else {
                            info!(session_id, content_len = assistant_content.len(), tool_count = tool_calls.len(), "saved partial assistant message before cancel");
                        }
                    }
                    return Ok(());
                }
                ev = rx.recv() => match ev {
                    Some(e) => e,
                    None => break,
                }
            };
            match event.event_type {
                StreamEventType::Text => {
                    // CLI incremental save: text after tool calls = new turn.
                    // Flush the previous turn's content + tool calls to DB.
                    if cli_incremental && !tool_calls.is_empty() {
                        let tc_json = serde_json::to_string(&tool_calls).ok();
                        if let Err(e) = sessions.append_message(
                            session_id, "assistant", &assistant_content,
                            tc_json.as_deref(), None, None,
                        ) {
                            warn!(session_id = %session_id, error = %e, "failed to save CLI turn to DB");
                        } else {
                            debug!(session_id, content_len = assistant_content.len(), tool_count = tool_calls.len(), "saved CLI turn incrementally");
                        }
                        assistant_content.clear();
                        tool_calls.clear();
                        block_order.clear();
                    }
                    assistant_content.push_str(&event.text);
                    // Coalesce consecutive text events into one block
                    if block_order.last().map_or(true, |b| b.0 != "text") {
                        block_order.push(("text", None));
                    }
                    let _ = tx.send(event).await;
                }
                StreamEventType::Thinking => {
                    info!(session_id, "received thinking block");
                    let _ = tx.send(event).await;
                }
                StreamEventType::ToolCall => {
                    if let Some(ref tc) = event.tool_call {
                        info!(session_id, tool = %tc.name, tool_id = %tc.id, "tool call received");
                        tool_calls.push(tc.clone());
                        block_order.push(("tool", Some(tool_calls.len() - 1)));
                    }
                    let _ = tx.send(event).await;
                }
                StreamEventType::Error => {
                    warn!(session_id, error = ?event.error, "stream error event");
                    stream_error = event.error.clone();
                    // Don't forward to user yet — classify after stream ends
                }
                StreamEventType::Usage => {
                    if let Some(ref usage) = event.usage {
                        state.last_input_tokens = usage.input_tokens as usize;
                    }
                    let _ = tx.send(event).await;
                }
                StreamEventType::RateLimit => {
                    if let Some(ref meta) = event.rate_limit {
                        concurrency.report_success(Some(meta));

                        // Check Janus session/weekly usage and generate quota warning at >80%
                        let mut warnings = Vec::new();
                        if let (Some(limit), Some(remaining)) =
                            (meta.session_limit_credits, meta.session_remaining_credits)
                        {
                            if limit > 0 {
                                let used_pct =
                                    ((limit.saturating_sub(remaining)) as f64 / limit as f64) * 100.0;
                                if used_pct >= 80.0 {
                                    warnings.push(format!(
                                        "Session usage at {:.0}% (resets at {})",
                                        used_pct,
                                        meta.session_reset_at.as_deref().unwrap_or("unknown"),
                                    ));
                                }
                            }
                        }
                        if let (Some(limit), Some(remaining)) =
                            (meta.weekly_limit_credits, meta.weekly_remaining_credits)
                        {
                            if limit > 0 {
                                let used_pct =
                                    ((limit.saturating_sub(remaining)) as f64 / limit as f64) * 100.0;
                                if used_pct >= 80.0 {
                                    warnings.push(format!(
                                        "Weekly usage at {:.0}% (resets at {})",
                                        used_pct,
                                        meta.weekly_reset_at.as_deref().unwrap_or("unknown"),
                                    ));
                                }
                            }
                        }
                        if !warnings.is_empty() {
                            let warning_text = warnings.join(". ");
                            state.quota_warning = Some(warning_text.clone());

                            // Forward the rate limit event with warning text once per run
                            // so chat_dispatch can broadcast a quota_warning WS event.
                            if !state.quota_warning_sent {
                                state.quota_warning_sent = true;
                                let _ = tx.send(StreamEvent {
                                    event_type: StreamEventType::RateLimit,
                                    text: warning_text,
                                    tool_call: None,
                                    error: None,
                                    usage: None,
                                    rate_limit: event.rate_limit.clone(),
                                    widgets: None,
                                    provider_metadata: None,
                                    stop_reason: None,
                                }).await;
                            }
                        }
                    }
                }
                StreamEventType::Done => {
                    // Capture stop reason for max output recovery
                    if event.stop_reason.is_some() {
                        stop_reason = event.stop_reason.clone();
                    }
                    // Capture provider metadata for Janus tool stickiness
                    if let Some(meta) = event.provider_metadata {
                        sticky_metadata = Some(meta);
                    }
                }
                StreamEventType::ToolResult
                | StreamEventType::ApprovalRequest | StreamEventType::AskRequest => {
                    // ToolResult/Approval/Ask: only sent by runner, not received from provider.
                }
                StreamEventType::SubagentStart
                | StreamEventType::SubagentProgress
                | StreamEventType::SubagentComplete => {
                    // Forwarded from sub-agent orchestrator via stream_tx; relay to parent.
                    let _ = tx.send(event).await;
                }
            }
        }

        // Drop LLM permit now that stream is complete
        drop(_llm_permit);

        // Reset retry counters only when stream actually produced content
        if stream_error.is_none() && (!assistant_content.is_empty() || !tool_calls.is_empty()) {
            transient_retries = 0;
            retryable_retries = 0;
        }

        // Report success or rate limit to concurrency controller
        if stream_error.is_none() {
            concurrency.report_success(None);
        }

        // Handle stream errors — classify and retry (matches Go runner logic)
        if let Some(ref err_msg) = stream_error {
            warn!("stream error: {}", err_msg);
            let err = ProviderError::Stream(err_msg.clone());
            let reason = ai::classify_error_reason(&err);

            // Layer 1: Transient errors (connection reset, timeout, EOF)
            if ai::is_transient_error(&err) {
                transient_retries += 1;
                if transient_retries <= MAX_TRANSIENT_RETRIES {
                    let prov_count = providers.read().await.len();
                    if prov_count > 1 {
                        provider_idx += 1;
                    }
                    tokio::select! {
                        _ = cancel_token.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    }
                    continue;
                }
            }

            // Report rate limit to concurrency controller
            if reason == "rate_limit" {
                concurrency.report_rate_limit(None);
            }

            // Layer 2: Retryable errors (rate_limit, billing, provider errors)
            let is_retryable = err.is_retryable()
                || reason == "rate_limit"
                || reason == "billing"
                || reason == "provider"
                || reason == "timeout";
            if is_retryable {
                retryable_retries += 1;
                if retryable_retries > MAX_RETRYABLE_RETRIES {
                    let _ = tx.send(StreamEvent::error(
                        format!("Service temporarily unavailable after {} retries: {}", MAX_RETRYABLE_RETRIES, err_msg)
                    )).await;
                    break;
                }
                warn!(reason, retryable_retries, "retryable stream error, trying next provider");
                let prov_count = providers.read().await.len();
                if prov_count > 1 {
                    provider_idx += 1;
                }
                tokio::select! {
                    _ = cancel_token.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
                continue;
            }

            // Layer 3: Non-retryable — send error to user
            let _ = tx.send(StreamEvent::error(err_msg.clone())).await;
        }

        info!(
            session_id,
            iteration,
            content_len = assistant_content.len(),
            tool_call_count = tool_calls.len(),
            has_error = stream_error.is_some(),
            "stream complete"
        );

        // Hook: message.post_receive — let apps modify response text before saving
        let assistant_content = if hooks.has_subscribers("message.post_receive") {
            let payload = serde_json::to_vec(&crate::hooks::PostReceivePayload {
                response_text: assistant_content.clone(),
                tool_calls_count: tool_calls.len(),
            })
            .unwrap_or_default();
            let (result, _) = hooks.apply_filter("message.post_receive", payload).await;
            match serde_json::from_slice::<crate::hooks::PostReceiveResponse>(&result) {
                Ok(resp) => resp.response_text.unwrap_or(assistant_content),
                Err(_) => assistant_content,
            }
        } else {
            assistant_content
        };

        // Save assistant message.
        // If there was a stream error, strip tool_calls — they won't be executed
        // so saving them would create orphans in the session history.
        let save_tool_calls = stream_error.is_none();
        if !assistant_content.is_empty() || (save_tool_calls && !tool_calls.is_empty()) {
            let tc_json = if !save_tool_calls || tool_calls.is_empty() {
                None
            } else {
                serde_json::to_string(&tool_calls).ok()
            };

            // Persist the content block order so rehydration preserves it.
            let metadata = if block_order.len() > 1 || block_order.first().map_or(false, |b| b.0 == "tool") {
                let blocks: Vec<serde_json::Value> = block_order
                    .iter()
                    .map(|(kind, idx)| match (*kind, idx) {
                        ("tool", Some(i)) => serde_json::json!({"type": "tool", "toolCallIndex": i}),
                        _ => serde_json::json!({"type": "text"}),
                    })
                    .collect();
                serde_json::to_string(&serde_json::json!({"contentBlocks": blocks})).ok()
            } else {
                None // single text block = default order, no need to persist
            };

            if let Err(e) = sessions.append_message(
                session_id,
                "assistant",
                &assistant_content,
                tc_json.as_deref(),
                None,
                metadata.as_deref(),
            ) {
                warn!(session_id = %session_id, error = %e, "failed to save assistant message to DB");
            }

            // Hook: session.message_append — notify apps that a message was saved
            if hooks.has_subscribers("session.message_append") {
                let payload = serde_json::to_vec(&crate::hooks::MessageAppendPayload {
                    session_id: session_id.to_string(),
                    role: "assistant".to_string(),
                    content: assistant_content.clone(),
                })
                .unwrap_or_default();
                hooks.do_action("session.message_append", payload).await;
            }
        }

        // CLI providers handle their own tool execution via MCP — skip runner tool loop
        if provider.handles_tools() && !tool_calls.is_empty() {
            info!(session_id, tool_count = tool_calls.len(), "CLI provider handled tools via MCP");
            break;
        }

        // Execute tool calls in parallel
        if !tool_calls.is_empty() {
            let resolved_key = sessions.resolve_session_key(session_id)
                .unwrap_or_else(|_| session_id.to_string());
            let ctx = ToolContext {
                origin,
                session_key: resolved_key,
                session_id: session_id.to_string(),
                user_id: user_id.to_string(),
                entity_permissions: entity_permissions.cloned(),
                resource_grants: entity_resource_grants.cloned(),
                allowed_paths: allowed_paths.to_vec(),
                cancel_token: cancel_token.clone(),
                stream_tx: Some(tx.clone()),
                run_id: progress.map(|p| p.run_id.clone()),
            };

            // Track tool names for filter + activate any deferred tools on first call
            for tc in &tool_calls {
                called_tools.push(tc.name.clone());
                if deferred_names.contains(&tc.name) && !activated_deferred.contains(&tc.name) {
                    debug!(tool = %tc.name, "activating deferred tool on first call");
                    activated_deferred.insert(tc.name.clone());
                }
            }

            // Launch all tool calls concurrently via FuturesUnordered
            let mut futures = FuturesUnordered::new();
            // Update progress: count tools and set current tool name
            if let Some(p) = progress {
                p.tool_call_count.fetch_add(tool_calls.len() as u32, std::sync::atomic::Ordering::Relaxed);
                if let Ok(mut ct) = p.current_tool.lock() {
                    ct.clear();
                    if tool_calls.len() == 1 {
                        ct.push_str(&tool_calls[0].name);
                    } else {
                        ct.push_str(&format!("{} tools", tool_calls.len()));
                    }
                }
            }
            for (idx, tc) in tool_calls.iter().enumerate() {
                let tools = tools.clone();
                let ctx = ctx.clone();
                let tc = tc.clone();
                let concurrency = concurrency.clone();
                futures.push(async move {
                    // Acquire tool permit inside the future
                    let _permit = concurrency.acquire_tool_permit().await;
                    let input_str = tc.input.to_string();
                    let input_log = truncate_str(&input_str, 500);
                    info!(tool = %tc.name, id = %tc.id, input = %input_log, "executing tool");
                    let result = tokio::time::timeout(
                        TOOL_EXECUTION_TIMEOUT,
                        tools.execute(&ctx, &tc.name, tc.input.clone()),
                    )
                    .await;
                    let result = match result {
                        Ok(r) => r,
                        Err(_) => ToolResult::error(format!(
                            "Tool '{}' timed out after {}s",
                            tc.name,
                            TOOL_EXECUTION_TIMEOUT.as_secs()
                        )),
                    };
                    let result_log = truncate_str(&result.content, 300);
                    info!(tool = %tc.name, id = %tc.id, is_error = result.is_error, result = %result_log, "tool result");
                    (idx, tc, result)
                });
            }

            // Collect results as they complete, send events immediately
            let mut results: Vec<Option<(ai::ToolCall, ToolResult)>> = vec![None; tool_calls.len()];
            loop {
                let item = tokio::select! {
                    _ = cancel_token.cancelled() => {
                        info!(session_id, "run cancelled during tool execution");
                        return Ok(());
                    }
                    next = futures.next() => match next {
                        Some(v) => v,
                        None => break,
                    }
                };
                let (idx, tc, result) = item;
                // Send tool result event immediately as each completes
                let _ = tx
                    .send(StreamEvent {
                        event_type: StreamEventType::ToolResult,
                        text: result.content.clone(),
                        tool_call: Some(ai::ToolCall {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            input: serde_json::Value::Null,
                        }),
                        error: if result.is_error {
                            Some(result.content.clone())
                        } else {
                            None
                        },
                        usage: None,
                        rate_limit: None,
                        widgets: None,
                        provider_metadata: None,
                        stop_reason: None,
                    })
                    .await;
                results[idx] = Some((tc, result));
            }

            // Sidecar vision verification — only for providers that can't include
            // images directly in tool results. Vision-capable providers (Anthropic,
            // Gemini) get the raw image passed through instead.
            {
                let main_supports_images = {
                    let prov_lock = providers.read().await;
                    prov_lock.first().map_or(false, |p| p.supports_tool_result_images())
                };

                if !main_supports_images {
                    let sidecar_provider = {
                        let prov_lock = providers.read().await;
                        prov_lock.first().cloned()
                    };
                    if let Some(provider) = sidecar_provider {
                        let mut sidecar_futures = FuturesUnordered::new();

                        for (idx, entry) in results.iter().enumerate() {
                            if let Some((tc, result)) = entry {
                                if let Some(ref image_url) = result.image_url {
                                    let image_url = image_url.clone();
                                    let action_ctx = format!("{} — {}", tc.name, result.content);
                                    let prov = provider.clone();
                                    sidecar_futures.push(async move {
                                        let verification = crate::sidecar::verify_screenshot(
                                            prov.as_ref(), &image_url, &action_ctx,
                                        )
                                        .await;
                                        (idx, verification)
                                    });
                                }
                            }
                        }

                        while let Some((idx, verification)) = tokio::select! {
                            _ = cancel_token.cancelled() => {
                                info!(session_id, "run cancelled during sidecar verification");
                                return Ok(());
                            }
                            next = sidecar_futures.next() => next
                        } {
                            if let Some(text) = verification {
                                if let Some((_, ref mut result)) = results[idx] {
                                    result.content.push_str(&format!("\n\n[Visual: {}]", text));
                                    result.image_url = None;
                                }
                            }
                        }
                    }
                }
            }

            // Save all tool results to session in deterministic order
            // and track whether ALL results in this iteration were errors
            const UNIVERSAL_TOOL_RESULT_CAP: usize = 100_000;
            let mut all_errors_this_iteration = true;
            let mut had_results = false;
            for entry in results.into_iter().flatten() {
                let (tc, mut result) = entry;
                had_results = true;
                if !result.is_error {
                    all_errors_this_iteration = false;
                }
                // Universal safety net: cap any tool result that exceeds 100K chars.
                // Per-tool caps (50K) handle common cases; this catches MCP, plugin, etc.
                if result.content.len() > UNIVERSAL_TOOL_RESULT_CAP {
                    let total_len = result.content.len();
                    let preview = truncate_str(&result.content, 4_000);
                    result.content = format!(
                        "{}\n\n[Result truncated: {} chars total, showing first 4000. Re-run the tool with narrower parameters.]",
                        preview, total_len
                    );
                }
                // Cache tool documentation results so they survive sliding window eviction.
                // Detect help/schema actions on skill and plugin tools.
                if !result.is_error && result.content.len() > 100 {
                    if let Some(cache_key) = detect_tool_doc_call(&tc.name, &tc.input) {
                        let content = if result.content.len() > MAX_TOOL_DOC_CONTENT {
                            truncate_str(&result.content, MAX_TOOL_DOC_CONTENT).to_string()
                        } else {
                            result.content.clone()
                        };
                        // Remove existing entry with same key (LRU refresh)
                        tool_doc_cache.retain(|(k, _)| k != &cache_key);
                        // Evict oldest if at capacity
                        if tool_doc_cache.len() >= MAX_TOOL_DOC_ENTRIES {
                            tool_doc_cache.remove(0);
                        }
                        tool_doc_cache.push((cache_key.clone(), content));
                        debug!(key = %cache_key, "cached tool documentation");
                    }
                }

                let row = ToolResultRow {
                    tool_call_id: tc.id.clone(),
                    content: result.content,
                    is_error: result.is_error,
                    image_url: result.image_url,
                };
                let tr_json = serde_json::json!([row]).to_string();

                if let Err(e) = sessions.append_message(
                    session_id,
                    "tool",
                    "",
                    None,
                    Some(&tr_json),
                    None,
                ) {
                    warn!(session_id = %session_id, error = %e, "failed to save tool message to DB");
                }
            }

            // Compute tool call hashes for loop detection (OpenClaw-style).
            // Tuple: (name_hash, args_hash, result_hash) — detects same-tool-same-args
            // and stale results independently.
            for tc in &tool_calls {
                let name_hash = simple_hash(tc.name.as_bytes());
                let args_str = tc.input.to_string();
                let args_hash = simple_hash(args_str.as_bytes());
                // Hash first 2000 bytes of the most recent result for this tool
                let content_hash = sessions.get_messages(session_id)
                    .ok()
                    .and_then(|msgs| msgs.iter().rev().find(|m| m.role == "tool").cloned())
                    .and_then(|m| m.tool_results)
                    .map(|tr| simple_hash(tr.as_bytes().get(..2000).unwrap_or(tr.as_bytes())))
                    .unwrap_or(0);
                recent_tool_result_hashes.push((name_hash, args_hash, content_hash));
                recent_tool_names.push(tc.name.clone());
                // Keep last 10 for ping-pong detection
                if recent_tool_result_hashes.len() > 10 {
                    recent_tool_result_hashes.remove(0);
                    recent_tool_names.remove(0);
                }
            }

            // Update consecutive error iteration counter
            if had_results && all_errors_this_iteration {
                consecutive_error_iterations += 1;
                warn!(
                    session_id, iteration,
                    consecutive_error_iterations,
                    "all tool calls failed this iteration"
                );
            } else {
                consecutive_error_iterations = 0;
            }

            // Clear current tool in progress tracker
            if let Some(p) = progress {
                if let Ok(mut ct) = p.current_tool.lock() {
                    ct.clear();
                }
            }

            // agent.turn action — notify apps after tool execution
            if hooks.has_subscribers("agent.turn") {
                let turn_tool_names: Vec<String> = tool_calls.iter().map(|tc| tc.name.clone()).collect();
                let payload = serde_json::to_vec(&crate::hooks::TurnPayload {
                    session_id: session_id.to_string(),
                    turn: iteration,
                    tool_calls: turn_tool_names,
                    total_tool_calls: called_tools.clone(),
                    has_active_task: !active_task.is_empty(),
                })
                .unwrap_or_default();
                hooks.do_action("agent.turn", payload).await;
            }

            // Continue loop — LLM needs to respond to tool results
            continue;
        }

        // Max output tokens recovery: if response was truncated, force continuation
        if stop_reason.as_deref() == Some("length")
            || stop_reason.as_deref() == Some("max_tokens")
        {
            if output_recovery_attempts < MAX_OUTPUT_RECOVERY_ATTEMPTS {
                output_recovery_attempts += 1;
                info!(iteration, session_id, attempt = output_recovery_attempts, "max output tokens recovery");
                continuation_steering = Some(
                    "<system>Your previous response was cut off by the output token limit. \
                     Resume directly from where you stopped. No recap, no apology. \
                     If you had pending tool calls, make them now.</system>".to_string()
                );
                continue;
            }
        }
        // Reset recovery counter on successful non-truncated completion
        if stop_reason.as_deref() != Some("length") && stop_reason.as_deref() != Some("max_tokens") {
            output_recovery_attempts = 0;
        }

        // Token budget continuation: if min_iterations is set and not yet reached,
        // force-continue even if the LLM wants to stop.
        if min_iterations > 0 && iteration < min_iterations && tool_calls.is_empty() {
            if cancel_token.is_cancelled() {
                info!(session_id, "skipping budget continuation: run was cancelled");
                break;
            }
            if !assistant_content.is_empty() {
                info!(iteration, session_id, min = min_iterations, "budget continuation: forcing next iteration");
                continuation_steering = Some(
                    "<system>You stopped early but your task is not complete. \
                     Keep working — use your tools to make more progress. \
                     Do not summarize or ask to continue. Take the next action.</system>".to_string()
                );
                continue;
            }
        }

        // No tool calls — check if we should auto-continue.
        //
        // Philosophy (aligned with Claude Code / OpenClaw / Hermes): the primary
        // continuation signal is the presence of tool_use blocks, NOT pattern
        // matching on response text.  We only auto-continue when there are
        // concrete incomplete work tasks AND the response isn't a cycle.
        let has_incomplete = work_tasks.iter().any(|t| t.status != "completed");
        let has_task_context = !active_task.is_empty()
            || user_demanded_action_sticky
            || (has_incomplete && !work_tasks.is_empty());
        let auto_limit = max_auto_continuations(&work_tasks);

        // Work-task-aware continuation — only when there are explicitly
        // incomplete work tasks (concrete signal, not text heuristics).
        if has_task_context
            && has_incomplete
            && auto_continuations < auto_limit
            && !assistant_content.is_empty()
        {
            // Cycle detection: if the response is identical to the previous
            // auto-continued response, the agent is stuck — stop.
            if let Some(ref prev) = prev_auto_content {
                if prev == &assistant_content {
                    info!(
                        iteration, session_id,
                        auto_continuations,
                        "cycle detected: identical response, stopping auto-continuation"
                    );
                    break;
                }
            }

            if cancel_token.is_cancelled() {
                info!(session_id, "skipping work-task auto-continue: run was cancelled");
                break;
            }
            prev_auto_content = Some(assistant_content.clone());
            auto_continuations += 1;
            let incomplete_count = work_tasks.iter().filter(|t| t.status != "completed").count();
            info!(
                iteration, session_id,
                auto_continuations,
                auto_limit,
                incomplete_count,
                "auto-continuing: incomplete work tasks remain"
            );
            continuation_steering = Some(format!(
                "<system>You stopped but there are still {} incomplete tasks. \
                 Continue working on the next incomplete task. Do not summarize \
                 or ask permission — take the next action now.</system>",
                incomplete_count
            ));
            continue;
        }

        // agent.turn action — notify apps at natural break
        if hooks.has_subscribers("agent.turn") {
            let payload = serde_json::to_vec(&crate::hooks::TurnPayload {
                session_id: session_id.to_string(),
                turn: iteration,
                tool_calls: vec![],
                total_tool_calls: called_tools.clone(),
                has_active_task: !active_task.is_empty(),
            })
            .unwrap_or_default();
            hooks.do_action("agent.turn", payload).await;
        }

        // Conversation turn complete
        info!(iteration, session_id, "agentic loop complete");
        break;
    }

    // Debounced memory extraction: only runs after 5s idle per session.
    // Extract from last exchange only (last user msg + assistant response + tool
    // calls) to avoid re-extracting facts from old messages and creating duplicates.
    let has_providers = !providers.read().await.is_empty();
    if !skip_memory && has_providers {
        let all_msgs = sessions
            .get_messages(session_id)
            .unwrap_or_default();
        // Find the last user message and take everything from there onward.
        let last_exchange: Vec<_> = {
            let last_user_idx = all_msgs.iter().rposition(|m| m.role == "user");
            match last_user_idx {
                Some(idx) => all_msgs[idx..].to_vec(),
                None => vec![],
            }
        };
        if last_exchange.len() >= 2 {
            use crate::memory_debounce::MemoryDebouncer;
            use std::sync::OnceLock;
            static DEBOUNCER: OnceLock<MemoryDebouncer> = OnceLock::new();
            let debouncer = DEBOUNCER.get_or_init(MemoryDebouncer::default);

            let providers = providers.clone();
            let store = store.clone();
            let mem_uid = memory_user_id.clone();
            let session_id_owned = session_id.to_string();

            debouncer.schedule(session_id, move || async move {
                let provider = {
                    let prov_lock = providers.read().await;
                    prefer_non_gateway(&prov_lock)
                };
                if let Some(provider) = provider {
                    if let Some(facts) = memory::extract_facts(provider.as_ref(), &last_exchange).await {
                        memory::store_facts(&store, &facts, &mem_uid);
                        debug!(session_id = session_id_owned, "extracted and stored memory facts");
                    }
                }
            }).await;
        }
    }

    Ok(())
}

/// Truncate a string to at most `max_bytes` bytes without splitting a multi-byte
/// UTF-8 character. Returns a `&str` that is always valid UTF-8.
pub(crate) fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Walk backwards from max_bytes to find a char boundary
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Detect if a tool call is requesting documentation (help/schema).
/// Returns a cache key like "skill:gws-sheets" or "plugin:sheets:help" if so.
fn detect_tool_doc_call(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let resource = input.get("resource").and_then(|v| v.as_str()).unwrap_or("");

    match tool_name {
        "skill" => {
            if action == "help" || action == "list" || action == "docs" {
                let skill_name = input.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                Some(format!("skill:{}", skill_name))
            } else {
                None
            }
        }
        "plugin" => {
            if action == "help" || action == "schema" || action == "services" {
                let name = if !resource.is_empty() {
                    resource
                } else {
                    input.get("name").and_then(|v| v.as_str()).unwrap_or("unknown")
                };
                Some(format!("plugin:{}:{}", name, action))
            } else {
                None
            }
        }
        // MCP tool documentation
        "mcp" => {
            if action == "help" || action == "list" || action == "schema" {
                let server = input.get("server").and_then(|v| v.as_str()).unwrap_or("unknown");
                Some(format!("mcp:{}:{}", server, action))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Compute max auto-continuations based on incomplete work tasks.
/// Scales with remaining work so batch tasks get more runway.
fn max_auto_continuations(work_tasks: &[steering::WorkTask]) -> usize {
    let incomplete = work_tasks.iter().filter(|t| t.status != "completed").count();
    if incomplete > 0 {
        (incomplete * 2).clamp(10, MAX_AUTO_CONTINUATIONS_CEILING)
    } else {
        MAX_AUTO_CONTINUATIONS_DEFAULT
    }
}

/// Check if recent user messages contain imperative language demanding action.
/// Serves as an implicit active-task signal for auto-continuation when objective
/// detection hasn't run yet.
fn user_demanded_action(messages: &[ChatMessage]) -> bool {
    let imperative_patterns = [
        "do it", "just do it", "get it done", "finish it", "keep going",
        "don't stop", "dont stop", "do not stop", "handle it", "do them all", "go ahead",
        "get them done", "do them", "finish them", "just go", "proceed",
        "continue", "keep at it", "do the rest", "all of them",
        "finish all", "complete all", "process all", "handle all",
        "work through all", "why did you stop", "why are you stopping",
    ];
    // Check last 5 user messages (was 2) to catch demands that scroll off
    let recent_user: Vec<&ChatMessage> = messages
        .iter()
        .rev()
        .filter(|m| {
            m.role == "user"
                && !m.content.starts_with("<system>")
                && !m.content.starts_with("<steering")
        })
        .take(5)
        .collect();

    for msg in &recent_user {
        let lower = msg.content.to_lowercase();
        // Match short-to-medium imperative messages (raised from 120 to 200)
        if lower.len() < 200 {
            for p in &imperative_patterns {
                if lower.contains(p) {
                    return true;
                }
            }
        }
    }
    false
}

/// Convert database ChatMessages to ai::Messages for the provider.
fn convert_messages(messages: &[ChatMessage]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|msg| {
            // Skip empty messages
            if msg.content.is_empty()
                && msg.tool_calls.as_ref().map_or(true, |tc| tc.is_empty())
                && msg.tool_results.as_ref().map_or(true, |tr| tr.is_empty())
            {
                return None;
            }

            let tool_calls = msg
                .tool_calls
                .as_ref()
                .and_then(|tc| {
                    if tc.is_empty() || tc == "[]" || tc == "null" {
                        None
                    } else {
                        serde_json::from_str::<serde_json::Value>(tc).ok()
                    }
                });

            let tool_results = msg
                .tool_results
                .as_ref()
                .and_then(|tr| {
                    if tr.is_empty() || tr == "[]" || tr == "null" {
                        None
                    } else {
                        serde_json::from_str::<serde_json::Value>(tr).ok()
                    }
                });

            let images = msg
                .metadata
                .as_ref()
                .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
                .and_then(|v| v.get("images").cloned())
                .and_then(|v| serde_json::from_value::<Vec<ai::ImageContent>>(v).ok());

            Some(Message {
                role: msg.role.clone(),
                content: msg.content.clone(),
                tool_calls,
                tool_results,
                images,
            })
        })
        .collect()
}

/// Sanitize message ordering: ensure tool results immediately follow their
/// corresponding assistant message. Self-heals corrupted session data
/// (back-to-back assistants, out-of-order tool results) that strict providers
/// like GPT-5-mini reject. Also strips orphaned tool results that reference
/// tool_call_ids not found in any preceding assistant message (matches Go's
/// sanitizeAgentMessages).
fn sanitize_message_order(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    if messages.is_empty() {
        return messages;
    }

    // Phase 1: Collect all tool_call_ids issued by assistant messages
    let mut issued_call_ids = HashSet::new();
    for msg in &messages {
        if msg.role == "assistant" {
            if let Some(ref tc_json) = msg.tool_calls {
                if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc_json) {
                    for call in &calls {
                        if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                            issued_call_ids.insert(id.to_string());
                        }
                    }
                }
            }
        }
    }

    // Phase 2: Map tool_call_id → tool result message for reordering.
    // Each tool message in DB has a single-element tool_results array.
    // Track which message indices are tool messages to skip in output.
    let mut tool_result_map: HashMap<String, ChatMessage> = HashMap::new();
    let mut tool_msg_indices = HashSet::new();
    let mut orphaned = 0u32;

    for (i, msg) in messages.iter().enumerate() {
        if msg.role != "tool" {
            continue;
        }
        if let Some(ref tr_json) = msg.tool_results {
            if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(tr_json) {
                let mut valid_results = Vec::new();
                for r in &results {
                    let tcid = r
                        .get("tool_call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if tcid.is_empty() || !issued_call_ids.contains(tcid) {
                        orphaned += 1;
                        continue;
                    }
                    valid_results.push((tcid.to_string(), r.clone()));
                }

                if !valid_results.is_empty() {
                    tool_msg_indices.insert(i);
                    for (tcid, result_val) in valid_results {
                        let single_tr = serde_json::json!([result_val]).to_string();
                        tool_result_map.insert(
                            tcid,
                            ChatMessage {
                                id: msg.id.clone(),
                                chat_id: msg.chat_id.clone(),
                                role: "tool".to_string(),
                                content: msg.content.clone(),
                                metadata: msg.metadata.clone(),
                                created_at: msg.created_at,
                                day_marker: msg.day_marker.clone(),
                                tool_calls: None,
                                tool_results: Some(single_tr),
                                token_estimate: msg.token_estimate,
                            },
                        );
                    }
                } else if orphaned > 0 {
                    // All results in this message were orphaned — skip entire message
                    tool_msg_indices.insert(i);
                }
            }
        }
    }

    // Phase 3: Rebuild with tool results injected after their assistant
    let mut result = Vec::with_capacity(messages.len());
    let mut reordered = 0u32;

    for (i, msg) in messages.into_iter().enumerate() {
        if tool_msg_indices.contains(&i) {
            continue;
        }
        let has_tool_calls = msg.role == "assistant" && msg.tool_calls.is_some();
        let tc_json = msg.tool_calls.clone();
        result.push(msg);

        if has_tool_calls {
            if let Some(ref tc) = tc_json {
                if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc) {
                    for call in &calls {
                        if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                            if let Some(tool_msg) = tool_result_map.remove(id) {
                                reordered += 1;
                                result.push(tool_msg);
                            }
                        }
                    }
                }
            }
        }
    }

    if reordered > 0 {
        debug!(reordered, "reordered tool results for correct message ordering");
    }
    if orphaned > 0 {
        debug!(orphaned, "stripped orphaned tool results");
    }
    // Drop any remaining unmatched results — they're double orphans
    let unmatched = tool_result_map.len();
    if unmatched > 0 {
        debug!(unmatched, "dropped unmatched tool results");
    }

    result
}

/// Detect user's working objective from latest message.
/// Runs as a background task (fire-and-forget) before the main loop.
async fn detect_objective(
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    sessions: &SessionManager,
    session_id: &str,
    user_prompt: &str,
) {
    if user_prompt.is_empty() {
        return;
    }

    let provider = {
        let prov_lock = providers.read().await;
        prefer_non_gateway(&prov_lock)
    };
    let provider = match provider {
        Some(p) => p,
        None => return,
    };

    let current_objective = sessions.get_active_task(session_id).unwrap_or_default();
    let obj_display = if current_objective.is_empty() {
        "none".to_string()
    } else {
        current_objective.clone()
    };

    // Gather recent conversation context (last 6 messages) for better classification
    let recent_context = sessions.get_messages(session_id)
        .ok()
        .map(|msgs| {
            let recent: Vec<String> = msgs.iter()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .filter(|m| m.role == "user" || m.role == "assistant")
                .map(|m| {
                    let content = if m.content.len() > 200 {
                        format!("{}...", truncate_str(&m.content, 200))
                    } else {
                        m.content.clone()
                    };
                    format!("[{}]: {}", m.role, content)
                })
                .collect();
            recent.join("\n")
        })
        .unwrap_or_default();

    let classify_prompt = format!(
        r#"You are classifying whether a user has started a NEW task or is continuing their current one.

Current objective: {obj}
Recent conversation:
{context}
Latest user message: {msg}

Respond with ONLY one JSON line, no markdown fences:
{{"action": "set", "objective": "concise 1-sentence objective"}}
OR {{"action": "update", "objective": "refined objective incorporating the addition"}}
OR {{"action": "clear"}}
OR {{"action": "keep"}}

## Decision rules (in priority order):

1. **TOPIC CHANGE → "set"**: If the user's message introduces a DIFFERENT subject, domain, or goal than the current objective, this is a new task. People don't announce "I'm starting a new task" — they just start talking about something else.
   - Current: "fix the login bug" → User: "can you help me write tests for the API" → **set** (different area)
   - Current: "build the dashboard" → User: "let's work on the deployment pipeline" → **set** (different system)
   - Current: "optimize the search query" → User: "I need to update the README" → **set** (different task entirely)

2. **CONTINUATION / REFINEMENT → "update"**: The user is adding scope or adjusting the SAME objective.
   - Current: "build user auth" → User: "also add password reset" → **update** (same domain, added scope)
   - Current: "fix the API" → User: "and add rate limiting while you're at it" → **update** (same area, added requirement)

3. **COMPLETION SIGNALS → "clear"**: The user signals the task is done with no new goal.
   - "thanks", "looks good", "perfect", "that's it", "never mind", "done"

4. **SAME TASK INTERACTION → "keep"**: Questions, feedback, or corrections that are clearly ABOUT the current objective.
   - Current: "fix the login bug" → User: "what's causing the null pointer?" → **keep** (investigating same bug)
   - Current: "build the dashboard" → User: "use a bar chart instead" → **keep** (feedback on same work)

5. **When objective is "none"**: Any message with an action or request → **"set"**. Pure greetings or questions with no task → **"keep"**.

## Key principle: When in doubt between "set" and "keep", prefer "set". A stale objective that doesn't match what the user actually wants is MORE harmful than resetting. The user can always continue the old task, but they can't unstick an agent that's persisting on a finished objective."#,
        obj = obj_display,
        context = if recent_context.is_empty() { "(no prior messages)".to_string() } else { recent_context },
        msg = user_prompt
    );

    let req = ChatRequest {
        messages: vec![Message {
            role: "user".to_string(),
            content: classify_prompt,
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 256,
        temperature: 0.0,
        system: String::new(),
        static_system: String::new(),
        model: String::new(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
    };

    let stream_result = provider.stream(&req).await;

    let mut rx = match stream_result {
        Ok(rx) => rx,
        Err(e) => {
            debug!(error = %e, "objective detection failed");
            return;
        }
    };

    let mut resp = String::new();
    while let Some(event) = rx.recv().await {
        if event.event_type == StreamEventType::Text {
            resp.push_str(&event.text);
        }
        if event.event_type == StreamEventType::Error {
            return;
        }
    }

    // Strip markdown fences and parse
    let resp = resp.trim();
    let resp = resp.trim_start_matches("```json").trim_start_matches("```");
    let resp = resp.trim_end_matches("```").trim();

    #[derive(serde::Deserialize)]
    struct ObjectiveResult {
        action: String,
        #[serde(default)]
        objective: String,
    }

    let result: ObjectiveResult = match serde_json::from_str(resp) {
        Ok(r) => r,
        Err(e) => {
            debug!(error = %e, response = resp, "objective parse failed");
            return;
        }
    };

    match result.action.as_str() {
        "set" if !result.objective.is_empty() => {
            info!(objective = %result.objective, "objective set");
            let _ = sessions.set_active_task(session_id, &result.objective);
        }
        "update" if !result.objective.is_empty() => {
            info!(objective = %result.objective, "objective updated");
            let _ = sessions.set_active_task(session_id, &result.objective);
        }
        "clear" => {
            info!("objective cleared");
            let _ = sessions.clear_active_task(session_id);
        }
        "keep" | _ => {
            // No change
        }
    }
}

/// Build the static system prompt.
fn build_system_prompt(custom_system: &str, memory_context: &str) -> String {
    let mut prompt = if custom_system.is_empty() {
        "You are Nebo, a personal AI assistant. You are helpful, accurate, and proactive. \
         You have access to tools for file operations, shell commands, web browsing, and memory.\n\
         \n\
         Guidelines:\n\
         - Use tools to accomplish tasks rather than just describing how to do them\n\
         - Be concise but thorough in your responses\n\
         - When asked to do something, do it — don't just explain how\n\
         - Store important information about the user using memory tools\n\
         - If a task requires multiple steps, work through them systematically\n"
            .to_string()
    } else {
        custom_system.to_string()
    };

    if !memory_context.is_empty() {
        prompt.push_str("\n\n# Memory context\n");
        prompt.push_str(memory_context);
    }

    prompt
}

/// Extract the plain skill name from a qualified ref.
/// "@nebo/skills/gws-gmail@^1.0.0" → "gws-gmail"
/// "SKIL-ABCD-1234" → "SKIL-ABCD-1234" (passed through)
/// "gws-gmail" → "gws-gmail" (plain names pass through)
/// Simple FNV-1a hash for stale-result detection. Not cryptographic.
fn simple_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn extract_skill_name(skill_ref: &str) -> String {
    if skill_ref.starts_with('@') {
        let without_at = &skill_ref[1..];
        let name_part = without_at.split('@').next().unwrap_or(without_at);
        name_part.rsplit('/').next().unwrap_or(name_part).to_string()
    } else {
        skill_ref.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_messages() {
        let messages = vec![
            ChatMessage {
                id: "1".into(),
                chat_id: "c".into(),
                role: "user".into(),
                content: "hello".into(),
                metadata: None,
                created_at: 0,
                day_marker: None,
                tool_calls: None,
                tool_results: None,
                token_estimate: None,
            },
            ChatMessage {
                id: "2".into(),
                chat_id: "c".into(),
                role: "assistant".into(),
                content: "hi there".into(),
                metadata: None,
                created_at: 0,
                day_marker: None,
                tool_calls: None,
                tool_results: None,
                token_estimate: None,
            },
        ];

        let result = convert_messages(&messages);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
    }

    #[test]
    fn test_build_system_prompt() {
        let prompt = build_system_prompt("", "- favorite color: blue");
        assert!(prompt.contains("Nebo"));
        assert!(prompt.contains("favorite color: blue"));
    }

    #[test]
    fn test_build_system_prompt_custom() {
        let prompt = build_system_prompt("You are a coding assistant.", "");
        assert!(prompt.contains("coding assistant"));
        assert!(!prompt.contains("Memory context"));
    }

    fn make_msg(id: &str, role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            chat_id: "c".into(),
            role: role.into(),
            content: content.into(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
        }
    }

    #[test]
    fn test_sanitize_preserves_correct_order() {
        // Already correct: assistant → tool → assistant → tool
        let msg1 = make_msg("1", "user", "hello");
        let mut msg2 = make_msg("2", "assistant", "let me help");
        msg2.tool_calls = Some(r#"[{"id":"call_1","name":"web","input":{}}]"#.into());
        let mut msg3 = make_msg("3", "tool", "");
        msg3.tool_results =
            Some(r#"[{"tool_call_id":"call_1","content":"result","is_error":false}]"#.into());
        let msg4 = make_msg("4", "assistant", "done");

        let result = sanitize_message_order(vec![msg1, msg2, msg3, msg4]);
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
        assert_eq!(result[2].role, "tool");
        assert_eq!(result[3].role, "assistant");
    }

    #[test]
    fn test_sanitize_reorders_back_to_back_assistants() {
        // Broken: assistant, assistant, tool(for #2), tool(for #1)
        let msg1 = make_msg("1", "user", "hello");
        let mut msg2 = make_msg("2", "assistant", "calling web");
        msg2.tool_calls = Some(r#"[{"id":"call_A","name":"web","input":{}}]"#.into());
        let mut msg3 = make_msg("3", "assistant", "calling system");
        msg3.tool_calls = Some(r#"[{"id":"call_B","name":"system","input":{}}]"#.into());
        let mut msg4 = make_msg("4", "tool", "");
        msg4.tool_results =
            Some(r#"[{"tool_call_id":"call_B","content":"sys result","is_error":false}]"#.into());
        let mut msg5 = make_msg("5", "tool", "");
        msg5.tool_results =
            Some(r#"[{"tool_call_id":"call_A","content":"web result","is_error":false}]"#.into());

        let result = sanitize_message_order(vec![msg1, msg2, msg3, msg4, msg5]);

        // Expected: user, assistant(A), tool(A), assistant(B), tool(B)
        assert_eq!(result.len(), 5);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant"); // call_A
        assert_eq!(result[2].role, "tool"); // result for call_A
        assert!(result[2]
            .tool_results
            .as_ref()
            .unwrap()
            .contains("call_A"));
        assert_eq!(result[3].role, "assistant"); // call_B
        assert_eq!(result[4].role, "tool"); // result for call_B
        assert!(result[4]
            .tool_results
            .as_ref()
            .unwrap()
            .contains("call_B"));
    }

    #[test]
    fn test_sanitize_strips_orphaned_tool_results() {
        // Tool result references a call_id that no assistant ever issued
        let msg1 = make_msg("1", "user", "hello");
        let mut msg2 = make_msg("2", "tool", "");
        msg2.tool_results = Some(
            r#"[{"tool_call_id":"call_ORPHAN","content":"orphaned","is_error":false}]"#.into(),
        );
        let msg3 = make_msg("3", "assistant", "hi");

        let result = sanitize_message_order(vec![msg1, msg2, msg3]);
        // Orphaned tool message should be stripped
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
    }

    #[test]
    fn test_extract_skill_name() {
        // Qualified ref with org and version
        assert_eq!(extract_skill_name("@nebo/skills/gws-gmail@^1.0.0"), "gws-gmail");
        // Qualified ref without version
        assert_eq!(extract_skill_name("@nebo/skills/gws-gmail"), "gws-gmail");
        // Plain name passthrough
        assert_eq!(extract_skill_name("gws-gmail"), "gws-gmail");
        // Code passthrough
        assert_eq!(extract_skill_name("SKIL-ABCD-1234"), "SKIL-ABCD-1234");
    }
}
