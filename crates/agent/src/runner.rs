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
/// Default context token limit for models that don't report one.
const DEFAULT_CONTEXT_TOKEN_LIMIT: usize = 80_000;
/// Max transient error retries before giving up.
const MAX_TRANSIENT_RETRIES: usize = 10;
/// Max retryable (provider/rate_limit/billing) retries before giving up.
const MAX_RETRYABLE_RETRIES: usize = 5;
/// Timeout for individual tool execution.
const TOOL_EXECUTION_TIMEOUT: Duration = Duration::from_secs(300);
/// Max auto-continuations when agent stops mid-task.
const MAX_AUTO_CONTINUATIONS: usize = 3;

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
}

/// Per-run mutable state (prevents data races across concurrent runs).
struct RunState {
    prompt_overhead: usize,
    last_input_tokens: usize,
    thresholds: Option<ContextThresholds>,
}

impl RunState {
    fn new() -> Self {
        Self {
            prompt_overhead: 0,
            last_input_tokens: 0,
            thresholds: None,
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
    active_role: tools::ActiveRoleState,
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
        active_role: tools::ActiveRoleState,
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
            active_role,
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

        // Append user message
        if !req.prompt.is_empty() {
            info!(session_id = %session_id, prompt_len = req.prompt.len(), "appending user message");
            if let Err(e) = self.sessions.append_message(
                &session_id,
                "user",
                &req.prompt,
                None,
                None,
            ) {
                warn!(session_id = %session_id, error = %e, "failed to append user message");
            }
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
        let active_role = self.active_role.clone();
        let system_prompt = req.system.clone();
        let user_id = req.user_id.clone();
        let origin = req.origin;
        let skip_memory = req.skip_memory_extract;

        // Resolve fuzzy model override (e.g. "sonnet" -> "anthropic/claude-sonnet-4")
        let model_override = if req.model_override.is_empty() {
            String::new()
        } else {
            self.selector.resolve_fuzzy(&req.model_override)
                .unwrap_or_else(|| req.model_override.clone())
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
                &active_role,
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
    active_role: &tools::ActiveRoleState,
) -> Result<(), String> {
    let mut state = RunState::new();
    let mut transient_retries = 0usize;
    let mut retryable_retries = 0usize;
    let mut called_tools: Vec<String> = Vec::new();
    let mut provider_idx: usize = 0;
    let mut auto_continuations = 0usize;

    // Load rich DB context (agent profile, user profile, personality directive, scored memories)
    let db_ctx = db_context::load_db_context(store, user_id);
    let agent_name = db_ctx
        .agent
        .as_ref()
        .map(|a| a.name.clone())
        .unwrap_or_else(|| "Nebo".to_string());
    let db_context_formatted = db_context::format_for_system_prompt(&db_ctx, &agent_name);

    // Get active task
    let active_task = sessions
        .get_active_task(session_id)
        .unwrap_or_default();

    // Build static system prompt — use modular prompt when no custom one is provided
    // STRAP docs and tool list are NOT included here — they're injected per-iteration
    // based on which tools pass the context filter (dynamic injection).
    let static_system = if system_prompt.is_empty() {
        let active_role_body = active_role.read().await.clone();
        let pctx = prompt::PromptContext {
            agent_name: agent_name.clone(),
            active_skill: None,
            skill_hints: Vec::new(),
            model_aliases: model_aliases.to_string(),
            channel: channel.to_string(),
            platform: std::env::consts::OS.to_string(),
            memory_context: String::new(),
            db_context: Some(db_context_formatted.clone()),
            active_role: active_role_body,
        };
        prompt::build_static(&pctx)
    } else {
        build_system_prompt(system_prompt, &db_context_formatted)
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

    for iteration in 1..=max_iterations {
        if cancel_token.is_cancelled() {
            info!(session_id, "run cancelled before iteration {}", iteration);
            return Ok(());
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
            if existing_summary.is_empty() {
                pruning::build_quick_fallback_summary(&evicted, &active_task)
            } else {
                existing_summary
            }
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
            user_prompt: String::new(),
            active_task: active_task.clone(),
            channel: channel.to_string(),
            agent_name: "Nebo".to_string(),
            iteration,
            work_tasks,
            quota_warning: None,
        };
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

        // Build dynamic system suffix
        let dctx = prompt::DynamicContext {
            provider_name: String::new(),
            model_name: if model_override.is_empty() { String::new() } else { model_override.to_string() },
            active_task: active_task.clone(),
            summary: summary.clone(),
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
        };

        // Acquire LLM permit before provider call (blocks if at capacity)
        let _llm_permit = concurrency.acquire_llm_permit().await;

        // Snapshot provider from lock, then release before I/O
        let provider = {
            let prov_lock = providers.read().await;
            if prov_lock.is_empty() {
                return Err("No AI providers available".to_string());
            }

            // Find provider by selected_provider_id, fall back to round-robin
            let idx = if !selected_provider_id.is_empty() {
                prov_lock
                    .iter()
                    .position(|p| p.id() == selected_provider_id)
                    .unwrap_or(provider_idx % prov_lock.len())
            } else {
                provider_idx % prov_lock.len()
            };

            info!(
                iteration,
                session_id,
                provider_idx = idx,
                provider_id = prov_lock[idx].id(),
                selected_model = %selected_model,
                provider_count = prov_lock.len(),
                message_count = chat_req.messages.len(),
                tool_count = chat_req.tools.len(),
                enable_thinking,
                "sending request to provider"
            );
            prov_lock[idx].clone()
        };
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

        while let Some(event) = rx.recv().await {
            match event.event_type {
                StreamEventType::Text => {
                    assistant_content.push_str(&event.text);
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
                    }
                }
                StreamEventType::Done | StreamEventType::ToolResult
                | StreamEventType::ApprovalRequest | StreamEventType::AskRequest => {
                    // Done: handled after loop. ToolResult/Approval/Ask: only sent by runner, not received from provider.
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

            let _ = sessions.append_message(
                session_id,
                "assistant",
                &assistant_content,
                tc_json.as_deref(),
                None,
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
                    info!(tool = %tc.name, id = %tc.id, "executing tool");
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
                    (idx, tc, result)
                });
            }

            // Collect results as they complete, send events immediately
            let mut results: Vec<Option<(ai::ToolCall, ToolResult)>> = vec![None; tool_calls.len()];
            while let Some((idx, tc, result)) = futures.next().await {
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
                    })
                    .await;
                results[idx] = Some((tc, result));
            }

            // Sidecar vision verification for browser tool results with screenshots
            {
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

                    while let Some((idx, verification)) = sidecar_futures.next().await {
                        if let Some(text) = verification {
                            if let Some((_, ref mut result)) = results[idx] {
                                result.content.push_str(&format!("\n\n[Visual: {}]", text));
                                result.image_url = None;
                            }
                        }
                    }
                }
            }

            // Save all tool results to session in deterministic order
            for entry in results.into_iter().flatten() {
                let (tc, result) = entry;
                let tr_json = serde_json::json!([{
                    "tool_call_id": tc.id,
                    "content": result.content,
                    "is_error": result.is_error
                }])
                .to_string();

                let _ = sessions.append_message(
                    session_id,
                    "tool",
                    "",
                    None,
                    Some(&tr_json),
                );
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
        if !active_task.is_empty()
            && auto_continuations < MAX_AUTO_CONTINUATIONS
            && looks_like_continuation_pause(&assistant_content)
        {
            auto_continuations += 1;
            info!(
                iteration, session_id,
                auto_continuations,
                "auto-continuing: agent paused mid-task"
            );

            // Inject a synthetic continuation message
            let _ = sessions.append_message(
                session_id,
                "user",
                "<system>Continue with your current objective. Do not ask for permission — use your tools to make progress.</system>",
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
            let user_id = user_id.to_string();
            let session_id_owned = session_id.to_string();

            debouncer.schedule(session_id, move || async move {
                let provider = {
                    let prov_lock = providers.read().await;
                    prov_lock.first().cloned()
                };
                if let Some(provider) = provider {
                    if let Some(facts) = memory::extract_facts(provider.as_ref(), &all_msgs).await {
                        memory::store_facts(&store, &facts, &user_id);
                        debug!(session_id = session_id_owned, "extracted and stored memory facts");
                    }
                }
            }).await;
        }
    }

    Ok(())
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

            Some(Message {
                role: msg.role.clone(),
                content: msg.content.clone(),
                tool_calls,
                tool_results,
                images: None,
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

    let classify_prompt = format!(
        r#"Classify this user message relative to the current working objective.

Current objective: {}
User message: {}

Respond with ONLY one JSON line, no markdown:
{{"action": "set", "objective": "concise 1-sentence objective"}}
OR {{"action": "update", "objective": "refined objective"}}
OR {{"action": "clear"}}
OR {{"action": "keep"}}

Rules:
- "set": User stated a new, distinct objective (e.g., "let's build X", "create Y", "fix Z")
- "update": User is refining or adding to the current objective (e.g., "also add tests", "and make it async")
- "clear": User is done or moving on without a new goal (e.g., "thanks", "looks good", "never mind")
- "keep": No change needed (questions, feedback, corrections about the CURRENT objective)
- Short messages (<15 words) that are CONVERSATIONAL with no action verb → "keep"
- Short messages (<15 words) that contain an ACTION or REQUEST → "set"
- If the message asks for something DIFFERENT from the current objective, use "set"
- If unsure, use "keep""#,
        obj_display, user_prompt
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
