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
/// Max auto-continuations when agent stops mid-task.
const MAX_AUTO_CONTINUATIONS: usize = 5;

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
                    let prov = self.providers.read().await.first().cloned();
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
        let entity_permissions = req.permissions.clone();
        let entity_resource_grants = req.resource_grants.clone();
        let personality_snippet = req.personality_snippet.clone();
        let allowed_paths = req.allowed_paths.clone();

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
) -> Result<(), String> {
    let mut state = RunState::new();
    let mut transient_retries = 0usize;
    let mut retryable_retries = 0usize;
    let mut called_tools: Vec<String> = Vec::new();
    let mut provider_idx: usize = 0;
    // Janus provider metadata for tool stickiness — echoed back in subsequent requests
    let mut sticky_metadata: Option<std::collections::HashMap<String, String>> = None;
    let mut auto_continuations = 0usize;
    let mut consecutive_error_iterations = 0usize;

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

    // Get active task
    let active_task = sessions
        .get_active_task(session_id)
        .unwrap_or_default();

    // Match skills against user prompt (force_skill overrides trigger matching)
    let (active_skill_template, skill_hints) = if let Some(loader) = skill_loader {
        if !force_skill.is_empty() {
            // Forced skill: look up by name directly
            match loader.get(force_skill).await {
                Some(skill) if skill.enabled => {
                    info!(skill = %skill.name, "force-activated skill");
                    (Some(skill.template.clone()), vec![format!("Active skill: {} — {}", skill.name, skill.description)])
                }
                _ => {
                    warn!(force_skill, "forced skill not found or disabled");
                    (None, Vec::new())
                }
            }
        } else if !user_prompt.is_empty() {
            // Trigger matching: find best matching skill
            let matches = loader.match_triggers(user_prompt, 3).await;
            if let Some(best) = matches.first() {
                info!(skill = %best.name, priority = best.priority, "skill matched via trigger");
                let hints: Vec<String> = matches.iter()
                    .map(|s| format!("Available skill: {} — {}", s.name, s.description))
                    .collect();
                (Some(best.template.clone()), hints)
            } else {
                (None, Vec::new())
            }
        } else {
            (None, Vec::new())
        }
    } else {
        (None, Vec::new())
    };

    // Build static system prompt — use modular prompt when no custom one is provided
    // STRAP docs and tool list are NOT included here — they're injected per-iteration
    // based on which tools pass the context filter (dynamic injection).
    let active_agent_body = active_agent_entry.as_ref().map(|r| r.agent_md.clone());
    let plugin_inventory = skill_loader
        .as_ref()
        .map(|l| l.plugin_inventory())
        .unwrap_or_default();

    let static_system = if system_prompt.is_empty() {
        let pctx = prompt::PromptContext {
            agent_name: agent_name.clone(),
            active_skill: active_skill_template,
            skill_hints,
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

    // Fire objective detection in background (non-blocking)
    {
        let providers = providers.clone();
        let store = store.clone();
        let session_id = session_id.to_string();
        let user_prompt = sessions.get_messages(&session_id)
            .ok()
            .and_then(|msgs| msgs.iter().rev().find(|m| m.role == "user").map(|m| m.content.clone()))
            .unwrap_or_default();
        tokio::spawn(async move {
            let session_mgr = SessionManager::new(store);
            detect_objective(&providers, &session_mgr, &session_id, &user_prompt).await;
        });
    }

    // Use the extended ceiling for the loop range; adaptive check below enforces
    // the default limit unless the agent is making genuine progress.
    let hard_ceiling = max_iterations.max(EXTENDED_MAX_ITERATIONS);

    for iteration in 1..=hard_ceiling {
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

        // Apply sliding window
        let (mut window_messages, evicted) =
            pruning::apply_sliding_window(&all_messages, run_start_time);

        // Build rolling summary if we evicted messages
        let summary = if !evicted.is_empty() {
            let existing_summary = sessions.get_summary(session_id).unwrap_or_default();
            let cheap_model = selector.get_cheapest_model();
            let prov = providers.read().await.first().cloned();
            let new_summary = if let Some(prov) = prov {
                pruning::build_llm_summary(
                    prov.as_ref(),
                    &evicted,
                    &existing_summary,
                    &active_task,
                    &cheap_model,
                )
                .await
                .unwrap_or_else(|e| {
                    debug!(error = %e, "LLM compaction failed, using fallback");
                    if existing_summary.is_empty() {
                        pruning::build_quick_fallback_summary(&evicted, &active_task)
                    } else {
                        format!(
                            "{}\n\n{}",
                            existing_summary,
                            pruning::build_quick_fallback_summary(&evicted, &active_task)
                        )
                    }
                })
            } else if existing_summary.is_empty() {
                pruning::build_quick_fallback_summary(&evicted, &active_task)
            } else {
                format!(
                    "{}\n\n{}",
                    existing_summary,
                    pruning::build_quick_fallback_summary(&evicted, &active_task)
                )
            };
            // Persist so it survives across iterations
            let _ = sessions.update_summary(session_id, &new_summary);
            new_summary
        } else {
            sessions.get_summary(session_id).unwrap_or_default()
        };

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

        // Compute context thresholds
        let thresholds = state.thresholds.get_or_insert_with(|| {
            ContextThresholds::from_context_window(
                DEFAULT_CONTEXT_TOKEN_LIMIT,
                state.prompt_overhead,
            )
        });

        // Micro-compact tool results if needed
        let (compacted_messages, _tokens_saved) =
            pruning::micro_compact(&window_messages, thresholds.warning);
        window_messages = compacted_messages;

        // Get tool definitions, filtered by context (returns active contexts for STRAP sub-doc injection)
        let all_tool_defs = tools.list().await;
        let (tool_defs, active_contexts) = tool_filter::filter_tools_with_context(&all_tool_defs, &window_messages, &called_tools);

        // Parse work tasks for steering
        let work_tasks_json = sessions.get_work_tasks(session_id).unwrap_or_default();
        let work_tasks: Vec<steering::WorkTask> = serde_json::from_str(&work_tasks_json)
            .unwrap_or_default();

        // Generate steering messages
        let steering_ctx = steering::Context {
            session_id: session_id.to_string(),
            messages: window_messages.clone(),
            user_prompt: user_prompt.to_string(),
            active_task: active_task.clone(),
            channel: channel.to_string(),
            agent_name: "Nebo".to_string(),
            iteration,
            work_tasks,
            quota_warning: state.quota_warning.clone(),
            consecutive_error_iterations,
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
            }).await;
            break;
        }

        let steering_messages = steering_pipeline.generate(&steering_ctx);

        // Hook: steering.generate — let apps inject additional steering messages
        let steering_messages = if hooks.has_subscribers("steering.generate") {
            let payload = serde_json::to_vec(&crate::hooks::SteeringGeneratePayload {
                session_id: session_id.to_string(),
                iteration,
            })
            .unwrap_or_default();
            let (result, _) = hooks.apply_filter("steering.generate", payload).await;
            match serde_json::from_slice::<crate::hooks::SteeringGenerateResponse>(&result) {
                Ok(resp) => {
                    let mut msgs = steering_messages;
                    for m in resp.messages {
                        let pos = if m.position == "after_user" {
                            steering::Position::AfterUser
                        } else {
                            steering::Position::End
                        };
                        msgs.push(steering::SteeringMessage {
                            content: m.content,
                            position: pos,
                        });
                    }
                    msgs
                }
                Err(_) => steering_messages,
            }
        } else {
            steering_messages
        };

        // Convert ChatMessage to ai::Message
        let mut ai_messages = convert_messages(&window_messages);

        // Inject steering
        if !steering_messages.is_empty() {
            let all_with_steering = steering::inject(window_messages.clone(), &steering_messages);
            ai_messages = convert_messages(&all_with_steering);
        }

        // Build per-iteration STRAP docs + tool list based on filtered tools
        let filtered_tool_names: Vec<String> = tool_defs.iter().map(|t| t.name.clone()).collect();
        let strap_section = prompt::build_strap_section(&filtered_tool_names, &active_contexts);
        let tools_list = prompt::build_tools_list(&filtered_tool_names);

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

        // Build dynamic system suffix — AFTER model selection so identity is accurate
        let dctx = prompt::DynamicContext {
            provider_name: selected_provider_id.to_string(),
            model_name: selected_model_name.to_string(),
            active_task: active_task.clone(),
            summary: summary.clone(),
            neboloop_connected: channel == "neboloop",
            channel: channel.to_string(),
        };
        let dynamic_suffix = prompt::build_dynamic_suffix(&dctx);

        let full_system = format!("{}\n\n{}\n\n{}{}", static_system, strap_section, tools_list, dynamic_suffix);

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
        };

        // Acquire LLM permit before provider call (blocks if at capacity)
        let _llm_permit = concurrency.acquire_llm_permit().await;

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

        let stream_result = provider.stream(&chat_req).await;

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
                    // Try next provider on transient error
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

                if e.is_retryable() {
                    retryable_retries += 1;
                    selector.mark_failed(&selected_model);
                    if retryable_retries > MAX_RETRYABLE_RETRIES {
                        return Err(format!("Service temporarily unavailable after {} retries: {}", MAX_RETRYABLE_RETRIES, e));
                    }
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

                return Err(format!("Provider error: {}", e));
            }
        };

        // Process stream (retry counters are reset after stream produces content)
        let mut assistant_content = String::new();
        let mut tool_calls: Vec<ai::ToolCall> = Vec::new();
        let mut stream_error: Option<String> = None;
        // Track the order of content blocks (text vs tool) for correct rehydration.
        // Each entry is either "text" (coalesced) or a tool index.
        let mut block_order: Vec<(&str, Option<usize>)> = Vec::new();

        loop {
            let event = tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!(session_id, "run cancelled during LLM stream");
                    return Ok(());
                }
                ev = rx.recv() => match ev {
                    Some(e) => e,
                    None => break,
                }
            };
            match event.event_type {
                StreamEventType::Text => {
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
                            (meta.session_limit_tokens, meta.session_remaining_tokens)
                        {
                            if limit > 0 {
                                let used_pct =
                                    ((limit.saturating_sub(remaining)) as f64 / limit as f64) * 100.0;
                                if used_pct >= 80.0 {
                                    warnings.push(format!(
                                        "Session usage at {:.0}% ({} of {} tokens remaining, resets at {})",
                                        used_pct,
                                        remaining,
                                        limit,
                                        meta.session_reset_at.as_deref().unwrap_or("unknown"),
                                    ));
                                }
                            }
                        }
                        if let (Some(limit), Some(remaining)) =
                            (meta.weekly_limit_tokens, meta.weekly_remaining_tokens)
                        {
                            if limit > 0 {
                                let used_pct =
                                    ((limit.saturating_sub(remaining)) as f64 / limit as f64) * 100.0;
                                if used_pct >= 80.0 {
                                    warnings.push(format!(
                                        "Weekly usage at {:.0}% ({} of {} tokens remaining, resets at {})",
                                        used_pct,
                                        remaining,
                                        limit,
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
                                }).await;
                            }
                        }
                    }
                }
                StreamEventType::Done => {
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

            let _ = sessions.append_message(
                session_id,
                "assistant",
                &assistant_content,
                tc_json.as_deref(),
                None,
                metadata.as_deref(),
            );

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
            };

            // Track tool names for filter
            for tc in &tool_calls {
                called_tools.push(tc.name.clone());
            }

            // Launch all tool calls concurrently via FuturesUnordered
            let mut futures = FuturesUnordered::new();
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
                let row = ToolResultRow {
                    tool_call_id: tc.id.clone(),
                    content: result.content,
                    is_error: result.is_error,
                    image_url: result.image_url,
                };
                let tr_json = serde_json::json!([row]).to_string();

                let _ = sessions.append_message(
                    session_id,
                    "tool",
                    "",
                    None,
                    Some(&tr_json),
                    None,
                );
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

        // No tool calls — check if we should auto-continue
        let has_task_context = !active_task.is_empty() || user_demanded_action(&all_messages);
        let looks_like_pause = looks_like_continuation_pause(&assistant_content)
            || looks_like_choice_question(&assistant_content);

        if has_task_context
            && auto_continuations < MAX_AUTO_CONTINUATIONS
            && looks_like_pause
        {
            auto_continuations += 1;
            info!(
                iteration, session_id,
                auto_continuations,
                "auto-continuing: agent paused mid-task"
            );

            // Use active_task if available, otherwise a generic fallback
            let task_desc = if active_task.is_empty() {
                "the task the user asked you to do".to_string()
            } else {
                active_task.clone()
            };

            // Inject a synthetic continuation message — scoped to the CURRENT task only
            let continuation_msg = format!(
                "<system>Continue with the task you were just working on: {}. Do not ask for permission or present options — pick the best approach and use your tools to execute it. If you have completed this task, say so and stop.</system>",
                task_desc
            );
            let _ = sessions.append_message(
                session_id,
                "user",
                &continuation_msg,
                Some(&serde_json::json!({"hidden": true}).to_string()),
                None,
                None,
            );
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
    // New messages reset the timer so extraction waits for conversation pauses.
    let has_providers = !providers.read().await.is_empty();
    if !skip_memory && has_providers {
        let all_msgs = sessions
            .get_messages(session_id)
            .unwrap_or_default();
        if all_msgs.len() >= 4 {
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
                    prov_lock.first().cloned()
                };
                if let Some(provider) = provider {
                    if let Some(facts) = memory::extract_facts(provider.as_ref(), &all_msgs).await {
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

/// Detect if the assistant's response looks like a "should I continue?" pause
/// rather than a genuine task completion. These patterns indicate the LLM is
/// asking for permission instead of autonomously continuing work.
fn looks_like_continuation_pause(content: &str) -> bool {
    let lower = content.to_lowercase();
    let patterns = [
        "should i continue",
        "shall i continue",
        "would you like me to continue",
        "want me to continue",
        "would you like me to proceed",
        "shall i proceed",
        "should i proceed",
        "want me to proceed",
        "would you like me to go ahead",
        "shall i go ahead",
        "ready to proceed",
        "let me know if you'd like me to",
        "let me know if you want me to",
        "let me know when you're ready",
        "do you want me to",
        "would you like me to",
        "i can continue",
        "i can proceed",
        "if you'd like, i can",
        "if you want, i can",
        "here's what i plan to do next",
        "here is what i plan to do",
        "my plan is to",
        "the next step would be",
        "the next steps would be",
        "next steps:",
        "i'll wait for your",
        "awaiting your",
    ];
    patterns.iter().any(|p| lower.contains(p))
}

/// Detect when the assistant presents options instead of acting — another form
/// of mid-task pausing that should trigger auto-continuation.
fn looks_like_choice_question(content: &str) -> bool {
    let lower = content.to_lowercase();
    let patterns = [
        "which do you prefer",
        "which would you prefer",
        "which option",
        "option 1",
        "option a)",
        "here are your options",
        "here are a few options",
        "would you prefer to",
        "there are a few ways",
        "there are several ways",
        "i could either",
        "we could either",
        "a few approaches",
        "which approach",
        "what would you like me to",
        "how would you like me to",
    ];
    patterns.iter().any(|p| lower.contains(p))
}

/// Check if recent user messages contain imperative language demanding action.
/// Serves as an implicit active-task signal for auto-continuation when objective
/// detection hasn't run yet.
fn user_demanded_action(messages: &[ChatMessage]) -> bool {
    let imperative_patterns = [
        "do it", "just do it", "get it done", "finish it", "keep going",
        "don't stop", "dont stop", "handle it", "do them all", "go ahead",
        "get them done", "do them", "finish them", "just go", "proceed",
        "continue", "keep at it", "do the rest", "all of them",
    ];
    // Check last 2 user messages (skip system/steering)
    let recent_user: Vec<&ChatMessage> = messages
        .iter()
        .rev()
        .filter(|m| {
            m.role == "user"
                && !m.content.starts_with("<system>")
                && !m.content.starts_with("<steering")
        })
        .take(2)
        .collect();

    for msg in &recent_user {
        let lower = msg.content.to_lowercase();
        // Only match short imperative messages (not long messages that happen to contain these phrases)
        if lower.len() < 120 {
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
        prov_lock.first().cloned()
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
}
