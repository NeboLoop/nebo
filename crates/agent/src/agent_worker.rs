//! AgentWorker — autonomous agent execution.
//!
//! One `AgentWorker` per active agent. Spawns and owns all trigger tasks
//! (schedule, heartbeat, event, watch, channel). The `AgentWorkerRegistry`
//! manages the lifecycle of all workers.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use db::Store;
use napp::plugin::PluginStore;
use tools::events::EventBus;
use tools::workflows::WorkflowManager;
use workflow::events::{EventDispatcher, EventSubscription};

use crate::dedupe::hash_text;

/// Cross-crate callback for broadcasting WebSocket events from agent workers.
pub type NotifyFn = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

/// Trait for dispatching a channel message into the agent's chat pipeline.
///
/// Defined here (agent crate), implemented in the server crate.
/// This is the bridge between channel_loop and run_chat().
///
/// File uploads are NOT handled by this trait. Each channel plugin owns its
/// own upload mechanism via a `upload` CLI subcommand that uses the plugin's
/// existing API client and auth. See `docs/publishers-guide/channel-plugins.md`
/// for the convention. The channel context is propagated to tools via
/// `ToolContext.channel`, which the plugin tool turns into env vars.
pub trait ChannelDispatcher: Send + Sync {
    /// Send a message to an agent and return the complete response text.
    ///
    /// - `agent_id`: target agent
    /// - `session_key`: conversation session (e.g., "agent:brief:slack:C123")
    /// - `channel_ctx`: which channel/thread this message came from — passed
    ///   through to tools so plugin uploads target the right destination
    /// - `prompt`: the user's message text
    fn dispatch<'a>(
        &'a self,
        agent_id: &'a str,
        session_key: &'a str,
        channel_ctx: tools::ChannelContext,
        prompt: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
}

/// A single autonomous agent worker. Owns all trigger tasks for one agent.
pub struct AgentWorker {
    pub agent_id: String,
    pub name: String,
    cancel: CancellationToken,
    event_dispatcher: Arc<EventDispatcher>,
    workflow_manager: Arc<dyn WorkflowManager>,
    _plugin_store: Arc<PluginStore>,
    _event_bus: EventBus,
}

impl AgentWorker {
    /// Start the worker: read bindings from DB, resolve agent config, spawn trigger tasks.
    ///
    /// When `config` is `Some`, uses the pre-parsed agent config directly (avoids
    /// redundant DB reads and `parse_agent_config` calls during startup).
    /// When `None`, falls back to loading and parsing from DB.
    pub fn start(
        agent_id: String,
        name: String,
        store: Arc<Store>,
        workflow_manager: Arc<dyn WorkflowManager>,
        event_dispatcher: Arc<EventDispatcher>,
        plugin_store: Arc<PluginStore>,
        event_bus: EventBus,
        notify_fn: Option<NotifyFn>,
        config: Option<napp::agent::AgentConfig>,
        watch_semaphore: Arc<tokio::sync::Semaphore>,
        channel_dispatch: Option<Arc<dyn ChannelDispatcher>>,
        shared_bridges: Arc<SharedBridgeRegistry>,
    ) -> Self {
        let cancel = CancellationToken::new();

        let bindings = match store.list_agent_workflows(&agent_id) {
            Ok(b) => b,
            Err(e) => {
                warn!(agent = %agent_id, error = %e, "failed to load agent workflow bindings");
                return Self {
                    agent_id,
                    name,
                    cancel,
                    event_dispatcher,
                    workflow_manager,
                    _plugin_store: plugin_store,
                    _event_bus: event_bus,
                };
            }
        };

        // Use pre-parsed config if provided, otherwise load from DB
        let agent_config = config.or_else(|| {
            match store.get_agent(&agent_id) {
                Ok(Some(r)) => match napp::agent::parse_agent_config(&r.frontmatter) {
                    Ok(cfg) => Some(cfg),
                    Err(e) => {
                        warn!(agent = %agent_id, error = %e, "failed to parse agent config frontmatter");
                        None
                    }
                },
                Ok(None) => {
                    warn!(agent = %agent_id, "agent not found in DB");
                    None
                }
                Err(e) => {
                    warn!(agent = %agent_id, error = %e, "failed to load agent from DB");
                    None
                }
            }
        });

        // Schedule triggers: delegate to existing cron system
        workflow::triggers::register_agent_triggers(&agent_id, &bindings, &store);

        for binding in &bindings {
            // Look up the WorkflowBinding from agent config to get activities
            let wf_binding = agent_config
                .as_ref()
                .and_then(|c| c.workflows.get(&binding.binding_name));

            match binding.trigger_type.as_str() {
                "heartbeat" => {
                    let (duration, window) = parse_heartbeat(&binding.trigger_config);
                    if duration.is_zero() {
                        warn!(
                            agent = %agent_id,
                            binding = %binding.binding_name,
                            config = %binding.trigger_config,
                            "invalid heartbeat config, skipping"
                        );
                        continue;
                    }
                    // Build inline definition JSON from agent config
                    let def_json = match wf_binding {
                        Some(wb) if wb.has_activities() => {
                            wb.to_workflow_json(&binding.binding_name)
                        }
                        _ => {
                            warn!(agent = %agent_id, binding = %binding.binding_name, "no inline activities found, skipping heartbeat");
                            continue;
                        }
                    };
                    let inputs: serde_json::Value = binding
                        .inputs
                        .as_ref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_else(|| serde_json::json!({}));
                    // Build emit_source from binding: "{agent-slug}.{emit-name}"
                    let emit_source = wf_binding.and_then(|wb| wb.emit.as_ref()).map(|emit_name| {
                        let slug = name.to_lowercase().replace(' ', "-");
                        format!("{}.{}", slug, emit_name)
                    });
                    let mgr = workflow_manager.clone();
                    let agent = agent_id.clone();
                    let bname = binding.binding_name.clone();
                    let token = cancel.clone();
                    let hb_store = store.clone();

                    tokio::spawn(async move {
                        let mut interval = tokio::time::interval(duration);
                        interval.tick().await; // skip first immediate tick

                        let wf_id = format!("agent:{}", agent);

                        loop {
                            tokio::select! {
                                _ = interval.tick() => {
                                    // Check time window if configured
                                    if let Some((start, end)) = &window {
                                        let now = chrono::Local::now().time();
                                        if now < *start || now > *end {
                                            continue;
                                        }
                                    }

                                    // Skip if a previous run for this binding is still active
                                    match hb_store.has_running_run(&wf_id, &bname) {
                                        Ok(true) => {
                                            debug!(
                                                agent = %agent,
                                                binding = %bname,
                                                "heartbeat skipped: previous run still active"
                                            );
                                            continue;
                                        }
                                        Ok(false) => {}
                                        Err(e) => {
                                            warn!(
                                                agent = %agent,
                                                binding = %bname,
                                                error = %e,
                                                "failed to check running runs, proceeding anyway"
                                            );
                                        }
                                    }

                                    match mgr.run_inline(def_json.clone(), inputs.clone(), "heartbeat", Some(bname.clone()), &agent, emit_source.clone()).await {
                                        Ok(run_id) => {
                                            info!(
                                                agent = %agent,
                                                binding = %bname,
                                                run_id = %run_id,
                                                "heartbeat triggered inline workflow"
                                            );
                                            notify_crate::send("Nebo", &format!("Heartbeat: {}", bname));
                                        }
                                        Err(e) => {
                                            warn!(
                                                agent = %agent,
                                                binding = %bname,
                                                error = %e,
                                                "heartbeat inline workflow run failed"
                                            );
                                            notify_crate::send("Nebo", &format!("{} failed: {}", bname, e));
                                        }
                                    }
                                }
                                _ = token.cancelled() => break,
                            }
                        }
                    });

                    info!(
                        agent = %agent_id,
                        binding = %binding.binding_name,
                        interval = ?duration,
                        window = ?window,
                        "started heartbeat trigger"
                    );
                }
                "event" => {
                    // Build inline definition JSON from agent config
                    let def_json = wf_binding
                        .filter(|wb| wb.has_activities())
                        .map(|wb| wb.to_workflow_json(&binding.binding_name));

                    let inputs: serde_json::Value = binding
                        .inputs
                        .as_ref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_else(|| serde_json::json!({}));

                    // Build emit_source from binding: "{agent-slug}.{emit-name}"
                    let event_emit_source =
                        wf_binding.and_then(|wb| wb.emit.as_ref()).map(|emit_name| {
                            let slug = name.to_lowercase().replace(' ', "-");
                            format!("{}.{}", slug, emit_name)
                        });

                    for source in binding.trigger_config.split(',') {
                        let pattern = source.trim().to_string();
                        if pattern.is_empty() {
                            continue;
                        }
                        let dispatcher = event_dispatcher.clone();
                        let sub = EventSubscription {
                            pattern,
                            default_inputs: inputs.clone(),
                            agent_source: agent_id.clone(),
                            binding_name: binding.binding_name.clone(),
                            definition_json: def_json.clone(),
                            emit_source: event_emit_source.clone(),
                        };
                        tokio::spawn(async move {
                            dispatcher.subscribe(sub).await;
                        });
                    }
                }
                "watch" => {
                    // Parse watch config from trigger_config JSON
                    let mut watch_cfg: WatchTriggerConfig =
                        match serde_json::from_str(&binding.trigger_config) {
                            Ok(c) => c,
                            Err(e) => {
                                warn!(
                                    agent = %agent_id,
                                    binding = %binding.binding_name,
                                    error = %e,
                                    "invalid watch trigger config, skipping"
                                );
                                continue;
                            }
                        };

                    // If event is specified, resolve command from plugin manifest
                    let auto_emit: Option<(String, bool)> = if let Some(ref event_name) =
                        watch_cfg.event
                    {
                        match plugin_store.resolve_event(&watch_cfg.plugin, event_name) {
                            Some(event_def) => {
                                if watch_cfg.command.is_empty() {
                                    watch_cfg.command = event_def.command.clone();
                                }
                                let multiplexed = watch_cfg.multiplexed || event_def.multiplexed;
                                let source = format!("{}.{}", watch_cfg.plugin, event_name);
                                Some((source, multiplexed))
                            }
                            None => {
                                warn!(
                                    agent = %agent_id,
                                    binding = %binding.binding_name,
                                    plugin = %watch_cfg.plugin,
                                    event = %event_name,
                                    "plugin event not found in manifest, falling back to explicit command"
                                );
                                if watch_cfg.command.is_empty() {
                                    warn!(agent = %agent_id, binding = %binding.binding_name, "no command and no event definition, skipping");
                                    continue;
                                }
                                None
                            }
                        }
                    } else {
                        None
                    };

                    // Resolve plugin binary
                    let binary_path = match plugin_store.resolve(&watch_cfg.plugin, "*") {
                        Some(p) => p,
                        None => {
                            warn!(
                                agent = %agent_id,
                                binding = %binding.binding_name,
                                plugin = %watch_cfg.plugin,
                                "watch plugin not found, skipping"
                            );
                            continue;
                        }
                    };

                    // Build inline definition JSON from agent config
                    let def_json = wf_binding
                        .filter(|wb| wb.has_activities())
                        .map(|wb| wb.to_workflow_json(&binding.binding_name));

                    // Skip only if no activities AND no auto-emit event
                    if def_json.is_none() && auto_emit.is_none() {
                        warn!(agent = %agent_id, binding = %binding.binding_name, "no inline activities and no event auto-emit, skipping watch");
                        continue;
                    }

                    let inputs: serde_json::Value = binding
                        .inputs
                        .as_ref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_else(|| serde_json::json!({}));

                    // Substitute agent input values into the command template
                    let input_values: serde_json::Value = match store.get_agent(&agent_id) {
                        Ok(Some(r)) => serde_json::from_str(&r.input_values).unwrap_or_else(|_| serde_json::json!({})),
                        _ => serde_json::json!({}),
                    };
                    let command = substitute_inputs(&watch_cfg.command, &input_values);

                    let emit_source = wf_binding.and_then(|wb| wb.emit.as_ref()).map(|emit_name| {
                        let slug = name.to_lowercase().replace(' ', "-");
                        format!("{}.{}", slug, emit_name)
                    });

                    let token = cancel.clone();
                    let mgr = workflow_manager.clone();
                    let agent = agent_id.clone();
                    let bname = binding.binding_name.clone();
                    let ps = plugin_store.clone();
                    let watch_plugin = watch_cfg.plugin.clone();
                    let bus = event_bus.clone();
                    let watch_store = store.clone();
                    let watch_notify = notify_fn.clone();
                    let watch_sem = watch_semaphore.clone();

                    tokio::spawn(watch_loop(
                        binary_path,
                        command,
                        watch_cfg,
                        ps,
                        def_json,
                        inputs,
                        agent,
                        bname,
                        emit_source,
                        mgr,
                        token,
                        auto_emit,
                        bus,
                        watch_store,
                        watch_notify,
                        watch_sem,
                    ));

                    info!(
                        agent = %agent_id,
                        binding = %binding.binding_name,
                        plugin = %watch_plugin,
                        "started watch trigger"
                    );
                }
                "folder" => {
                    let folder_cfg: FolderTriggerConfig =
                        match serde_json::from_str(&binding.trigger_config) {
                            Ok(c) => c,
                            Err(e) => {
                                warn!(
                                    agent = %agent_id,
                                    binding = %binding.binding_name,
                                    error = %e,
                                    "invalid folder trigger config, skipping"
                                );
                                continue;
                            }
                        };

                    let def_json = wf_binding
                        .filter(|wb| wb.has_activities())
                        .map(|wb| wb.to_workflow_json(&binding.binding_name));

                    if def_json.is_none() {
                        warn!(
                            agent = %agent_id,
                            binding = %binding.binding_name,
                            "no inline activities for folder trigger, skipping"
                        );
                        continue;
                    }

                    let inputs: serde_json::Value = binding
                        .inputs
                        .as_ref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_else(|| serde_json::json!({}));

                    // Substitute agent input values into the path template
                    let input_values: serde_json::Value = match store.get_agent(&agent_id) {
                        Ok(Some(r)) => serde_json::from_str(&r.input_values)
                            .unwrap_or_else(|_| serde_json::json!({})),
                        _ => serde_json::json!({}),
                    };
                    let watch_path =
                        substitute_inputs(&folder_cfg.path, &input_values);

                    let emit_source =
                        wf_binding
                            .and_then(|wb| wb.emit.as_ref())
                            .map(|emit_name| {
                                let slug = name.to_lowercase().replace(' ', "-");
                                format!("{}.{}", slug, emit_name)
                            });

                    let token = cancel.clone();
                    let mgr = workflow_manager.clone();
                    let agent = agent_id.clone();
                    let bname = binding.binding_name.clone();
                    let bus = event_bus.clone();
                    let folder_store = store.clone();

                    tokio::spawn(folder_watch_loop(
                        watch_path,
                        folder_cfg,
                        def_json.unwrap(),
                        inputs,
                        agent,
                        bname,
                        emit_source,
                        mgr,
                        token,
                        bus,
                        folder_store,
                    ));

                    info!(
                        agent = %agent_id,
                        binding = %binding.binding_name,
                        "started folder trigger"
                    );
                }
                "schedule" => {
                    // Already handled by register_agent_triggers above
                }
                "manual" => {
                    // No-op: user triggers via chat
                }
                other => {
                    warn!(
                        agent = %agent_id,
                        binding = %binding.binding_name,
                        trigger_type = %other,
                        "unknown trigger type"
                    );
                }
            }
        }

        // Spawn channel loops for DB-configured channel bindings.
        // Channel bindings are user-configured (Settings → Agent → Channels),
        // not declared in agent.json — so any agent can use any channel plugin.
        if let Some(dispatcher) = &channel_dispatch {
            let bindings = store
                .list_channel_bindings_for_agent(&agent_id)
                .unwrap_or_default();
            for binding in bindings {
                if !binding.is_enabled {
                    continue;
                }
                // Look up the plugin's channel capability from manifest
                let channel_def = match plugin_store.get_channel_def(&binding.plugin_slug) {
                    Some(ch) => ch,
                    None => {
                        warn!(
                            agent = %agent_id,
                            plugin = %binding.plugin_slug,
                            "plugin has no channel capability, skipping"
                        );
                        continue;
                    }
                };
                let binary_path = match plugin_store.resolve(&binding.plugin_slug, "*") {
                    Some(p) => p,
                    None => {
                        warn!(
                            agent = %agent_id,
                            plugin = %binding.plugin_slug,
                            "channel plugin binary not found, skipping"
                        );
                        continue;
                    }
                };

                let is_shared = channel_def.shared;

                if is_shared {
                    // Shared bridge: register this agent, start bridge if first
                    let sb = shared_bridges.clone();
                    let ps = plugin_store.clone();
                    let dispatch = dispatcher.clone();
                    let ch_notify = notify_fn.clone();
                    let ch_sem = watch_semaphore.clone();
                    let slug = binding.plugin_slug.clone();
                    let agent = agent_id.clone();
                    let agent_display = name.clone();
                    let bp = binary_path.clone();
                    let cd = channel_def;
                    let token = cancel.clone();

                    tokio::spawn(async move {
                        let already_running = sb.register_agent(&slug, &agent, &agent_display).await;
                        if already_running {
                            info!(
                                agent = %agent,
                                plugin = %slug,
                                "registered with existing shared bridge"
                            );
                        } else {
                            // First agent — start the shared bridge
                            info!(
                                agent = %agent,
                                plugin = %slug,
                                "starting shared bridge"
                            );
                            shared_channel_loop(
                                bp, slug, cd, ps, sb,
                                dispatch, token, ch_notify, ch_sem,
                            ).await;
                        }
                    });
                } else {
                    // Per-agent bridge (original behavior)
                    let token = cancel.clone();
                    let agent = agent_id.clone();
                    let agent_display = name.clone();
                    let ch_name = binding.plugin_slug.clone();
                    let ps = plugin_store.clone();
                    let dispatch = dispatcher.clone();
                    let ch_notify = notify_fn.clone();
                    let ch_store = store.clone();
                    let ch_sem = watch_semaphore.clone();
                    let agent_config = binding.config.clone();

                    tokio::spawn(channel_loop(
                        binary_path,
                        ch_name.clone(),
                        channel_def,
                        ps,
                        agent,
                        agent_display,
                        dispatch,
                        token,
                        ch_store,
                        ch_notify,
                        ch_sem,
                        agent_config,
                    ));
                }

                info!(
                    agent = %agent_id,
                    plugin = %binding.plugin_slug,
                    shared = %is_shared,
                    "started channel listener"
                );
            }
        }

        info!(agent = %agent_id, name = %name, bindings = bindings.len(), "agent worker started");

        Self {
            agent_id,
            name,
            cancel,
            event_dispatcher,
            workflow_manager,
            _plugin_store: plugin_store,
            _event_bus: event_bus,
        }
    }

    /// Stop the worker: cancel all spawned tasks, running workflows, cron jobs and event subscriptions.
    pub fn stop(&self, store: &Store) {
        self.cancel.cancel();
        workflow::triggers::unregister_agent_triggers(&self.agent_id, store);
        // Cancel any running workflows spawned by this agent
        let mgr = self.workflow_manager.clone();
        let agent_id_wf = self.agent_id.clone();
        tokio::spawn(async move {
            mgr.cancel_runs_for_agent(&agent_id_wf).await;
        });
        // Clean up event subscriptions (async, fire-and-forget)
        let dispatcher = self.event_dispatcher.clone();
        let agent_id = self.agent_id.clone();
        tokio::spawn(async move {
            dispatcher.unsubscribe_agent(&agent_id).await;
        });
        info!(agent = %self.agent_id, "agent worker stopped");
    }
}

/// Tracks a running shared bridge process and the agents registered with it.
struct SharedBridge {
    /// Agents registered: agent_id → agent display name
    agents: HashMap<String, String>,
    /// Cancel token for the bridge process (None if not yet started)
    cancel: Option<CancellationToken>,
}

/// Registry of shared bridge processes (one per plugin slug).
/// For plugins with `channel.shared = true`, a single bridge process serves all agents.
pub struct SharedBridgeRegistry {
    bridges: tokio::sync::RwLock<HashMap<String, SharedBridge>>,
}

impl SharedBridgeRegistry {
    pub fn new() -> Self {
        Self {
            bridges: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Register an agent with a shared bridge.
    /// Returns true if the bridge process is already running.
    /// Returns false if this is the first agent — caller must start the bridge.
    async fn register_agent(
        &self,
        plugin_slug: &str,
        agent_id: &str,
        agent_name: &str,
    ) -> bool {
        let mut bridges = self.bridges.write().await;
        if let Some(bridge) = bridges.get_mut(plugin_slug) {
            bridge.agents.insert(agent_id.to_string(), agent_name.to_string());
            bridge.cancel.is_some() // true if process is running
        } else {
            // First agent — create entry, caller will start the process
            let mut agents = HashMap::new();
            agents.insert(agent_id.to_string(), agent_name.to_string());
            bridges.insert(plugin_slug.to_string(), SharedBridge { agents, cancel: None });
            false
        }
    }

    /// Unregister an agent from a shared bridge. Stops the bridge if no agents remain.
    pub async fn unregister_agent(&self, plugin_slug: &str, agent_id: &str) {
        let mut bridges = self.bridges.write().await;
        let should_stop = if let Some(bridge) = bridges.get_mut(plugin_slug) {
            bridge.agents.remove(agent_id);
            bridge.agents.is_empty()
        } else {
            false
        };
        if should_stop {
            if let Some(bridge) = bridges.remove(plugin_slug) {
                if let Some(cancel) = bridge.cancel {
                    cancel.cancel();
                }
                info!(plugin = %plugin_slug, "shared bridge stopped — no agents remaining");
            }
        }
    }

    /// Mark the bridge as started with its cancel token.
    async fn mark_started(&self, plugin_slug: &str, cancel: CancellationToken) {
        let mut bridges = self.bridges.write().await;
        if let Some(bridge) = bridges.get_mut(plugin_slug) {
            bridge.cancel = Some(cancel);
        }
    }

    /// Mark the bridge as stopped (process exited, may restart).
    async fn mark_stopped(&self, plugin_slug: &str) {
        let mut bridges = self.bridges.write().await;
        if let Some(bridge) = bridges.get_mut(plugin_slug) {
            bridge.cancel = None;
        }
    }

    /// Get a snapshot of agents for routing.
    async fn get_agents(&self, plugin_slug: &str) -> HashMap<String, String> {
        self.bridges
            .read()
            .await
            .get(plugin_slug)
            .map(|b| b.agents.clone())
            .unwrap_or_default()
    }

    /// Stop all shared bridges (server shutdown).
    pub async fn stop_all(&self) {
        let mut bridges = self.bridges.write().await;
        for (slug, bridge) in bridges.drain() {
            if let Some(cancel) = bridge.cancel {
                cancel.cancel();
            }
            info!(plugin = %slug, "shared bridge stopped (shutdown)");
        }
    }
}

/// Registry of all active agent workers.
pub struct AgentWorkerRegistry {
    workers: RwLock<HashMap<String, AgentWorker>>,
    store: Arc<Store>,
    workflow_manager: Arc<dyn WorkflowManager>,
    event_dispatcher: Arc<EventDispatcher>,
    plugin_store: Arc<PluginStore>,
    event_bus: EventBus,
    notify_fn: Option<NotifyFn>,
    /// Serializes watch process spawning to prevent concurrent keychain prompts
    /// on macOS. Permits = 1 means only one watch process starts at a time.
    watch_semaphore: Arc<tokio::sync::Semaphore>,
    /// Dispatcher for channel messages → run_chat(). Set after AppState is created
    /// via OnceLock (late binding — same pattern as run_querier_handle).
    channel_dispatch: Arc<std::sync::OnceLock<Arc<dyn ChannelDispatcher>>>,
    /// Shared bridge processes — one per plugin for `shared: true` channels.
    pub shared_bridges: Arc<SharedBridgeRegistry>,
}

impl AgentWorkerRegistry {
    pub fn new(
        store: Arc<Store>,
        workflow_manager: Arc<dyn WorkflowManager>,
        event_dispatcher: Arc<EventDispatcher>,
        plugin_store: Arc<PluginStore>,
        event_bus: EventBus,
        notify_fn: Option<NotifyFn>,
    ) -> Self {
        Self {
            workers: RwLock::new(HashMap::new()),
            store,
            workflow_manager,
            event_dispatcher,
            plugin_store,
            event_bus,
            notify_fn,
            watch_semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
            channel_dispatch: Arc::new(std::sync::OnceLock::new()),
            shared_bridges: Arc::new(SharedBridgeRegistry::new()),
        }
    }

    /// Set the channel dispatcher (called once after AppState is ready).
    pub fn set_channel_dispatch(&self, dispatch: Arc<dyn ChannelDispatcher>) {
        let _ = self.channel_dispatch.set(dispatch);
    }

    /// Start an agent worker. If already running, stops the old one first.
    ///
    /// Pass `config` to avoid redundant `parse_agent_config` calls. When `None`,
    /// the worker loads and parses from DB (used by handlers that don't cache configs).
    pub async fn start_agent(
        &self,
        agent_id: &str,
        name: &str,
        config: Option<napp::agent::AgentConfig>,
    ) {
        // Stop existing worker if any
        {
            let mut workers = self.workers.write().await;
            if let Some(old) = workers.remove(agent_id) {
                old.stop(&self.store);
            }
        }

        let worker = AgentWorker::start(
            agent_id.to_string(),
            name.to_string(),
            self.store.clone(),
            self.workflow_manager.clone(),
            self.event_dispatcher.clone(),
            self.plugin_store.clone(),
            self.event_bus.clone(),
            self.notify_fn.clone(),
            config,
            self.watch_semaphore.clone(),
            self.channel_dispatch.get().cloned(),
            self.shared_bridges.clone(),
        );

        self.workers
            .write()
            .await
            .insert(agent_id.to_string(), worker);
    }

    /// Stop an agent worker.
    pub async fn stop_agent(&self, agent_id: &str) {
        let mut workers = self.workers.write().await;
        if let Some(worker) = workers.remove(agent_id) {
            worker.stop(&self.store);
        }
    }

    /// Stop all workers (shutdown).
    pub async fn stop_all(&self) {
        self.shared_bridges.stop_all().await;
        let mut workers = self.workers.write().await;
        for (_, worker) in workers.drain() {
            worker.stop(&self.store);
        }
    }
}

/// Parse a heartbeat trigger config string.
///
/// Format: `"30m"` or `"30m|08:00-18:00"`
///
/// Returns `(Duration, Option<(NaiveTime, NaiveTime)>)`.
fn parse_heartbeat(
    config: &str,
) -> (
    std::time::Duration,
    Option<(chrono::NaiveTime, chrono::NaiveTime)>,
) {
    let parts: Vec<&str> = config.split('|').collect();
    let duration = parse_duration(parts[0].trim());
    let window = if parts.len() > 1 {
        parse_time_window(parts[1].trim())
    } else {
        None
    };
    (duration, window)
}

/// Parse a duration string like "30m", "1h", "5s", "2h30m".
fn parse_duration(s: &str) -> std::time::Duration {
    let s = s.trim();
    let mut total_secs: u64 = 0;
    let mut num_buf = String::new();

    for c in s.chars() {
        if c.is_ascii_digit() {
            num_buf.push(c);
        } else {
            let n: u64 = num_buf.parse().unwrap_or(0);
            num_buf.clear();
            match c {
                'h' => total_secs += n * 3600,
                'm' => total_secs += n * 60,
                's' => total_secs += n,
                _ => {}
            }
        }
    }

    // If there's a trailing number with no unit, treat as minutes
    if !num_buf.is_empty() {
        let n: u64 = num_buf.parse().unwrap_or(0);
        total_secs += n * 60;
    }

    std::time::Duration::from_secs(total_secs)
}

/// Parse a time window like "08:00-18:00".
fn parse_time_window(s: &str) -> Option<(chrono::NaiveTime, chrono::NaiveTime)> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 2 {
        return None;
    }
    let start = chrono::NaiveTime::parse_from_str(parts[0].trim(), "%H:%M").ok()?;
    let end = chrono::NaiveTime::parse_from_str(parts[1].trim(), "%H:%M").ok()?;
    Some((start, end))
}

// ---------------------------------------------------------------------------
// Watch trigger
// ---------------------------------------------------------------------------

/// Deserialized watch trigger config from DB's trigger_config JSON.
#[derive(Debug, Clone, serde::Deserialize)]
struct WatchTriggerConfig {
    plugin: String,
    #[serde(default)]
    command: String,
    /// Plugin event name — when set, enables auto-emission and command resolution.
    #[serde(default)]
    event: Option<String>,
    /// If true, NDJSON lines may contain an `"event"` field for multiplexing.
    #[serde(default)]
    multiplexed: bool,
    #[serde(default = "default_restart_delay")]
    restart_delay_secs: u64,
}

fn default_restart_delay() -> u64 {
    5
}

/// Substitute `{{key}}` placeholders in a template with values from agent inputs.
fn substitute_inputs(template: &str, inputs: &serde_json::Value) -> String {
    let mut result = template.to_string();
    if let Some(obj) = inputs.as_object() {
        for (key, val) in obj {
            if let Some(s) = val.as_str() {
                result = result.replace(&format!("{{{{{}}}}}", key), s);
            }
        }
    }
    result
}

/// Long-running loop that spawns a plugin watcher process, reads NDJSON lines
/// from stdout, and fires `run_inline()` for each line received.
///
/// When `auto_emit` is set, each NDJSON line is also emitted into the EventBus
/// with the appropriate event source name. The workflow still runs if `def_json`
/// is provided.
///
/// On process exit (not cancelled): waits with exponential backoff, then restarts.
/// On cancel token: kills child process and breaks.
async fn watch_loop(
    binary_path: std::path::PathBuf,
    command: String,
    cfg: WatchTriggerConfig,
    plugin_store: Arc<PluginStore>,
    def_json: Option<String>,
    base_inputs: serde_json::Value,
    agent_id: String,
    binding_name: String,
    emit_source: Option<String>,
    workflow_manager: Arc<dyn WorkflowManager>,
    cancel: CancellationToken,
    auto_emit: Option<(String, bool)>,
    event_bus: EventBus,
    store: Arc<Store>,
    notify_fn: Option<NotifyFn>,
    watch_semaphore: Arc<tokio::sync::Semaphore>,
) {
    let mut backoff_secs = cfg.restart_delay_secs;
    let max_backoff_secs = 300; // 5 minutes

    // Clean stale dedup entries on (re)start
    let _ = store.cleanup_event_dedup(10 * 60);

    loop {
        if cancel.is_cancelled() {
            break;
        }

        // Parse command string into args
        let args = match shlex::split(&command) {
            Some(a) => a,
            None => {
                error!(
                    agent = %agent_id,
                    binding = %binding_name,
                    command = %command,
                    "failed to parse watch command, stopping"
                );
                break;
            }
        };

        let runtime = napp::PluginRuntime::new(
            &cfg.plugin,
            binary_path.clone(),
            plugin_store.clone(),
        )
        .with_home()
        .with_permissions();

        let mut cmd = tokio::process::Command::new(&binary_path);
        cmd.args(&args);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);
        cmd.env_clear();
        for (k, v) in runtime.build_env() {
            cmd.env(k, v);
        }

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        info!(
            agent = %agent_id,
            binding = %binding_name,
            binary = %binary_path.display(),
            command = %command,
            "spawning watch process"
        );

        // Serialize watch process spawning to prevent concurrent keychain
        // prompts on macOS. The permit is held only during spawn + initial
        // setup, then released so other watches can start.
        let _spawn_permit = watch_semaphore.acquire().await;

        // Reap any pre-existing instance of this binary — orphans from a
        // prior crashed Nebo will hold its events/sockets otherwise.
        napp::child_guard::reap_existing_for(&binary_path);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                error!(
                    agent = %agent_id,
                    binding = %binding_name,
                    error = %e,
                    "failed to spawn watch process, retrying in {}s",
                    backoff_secs
                );
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
                    _ = cancel.cancelled() => break,
                }
                backoff_secs = (backoff_secs * 2).min(max_backoff_secs);
                continue;
            }
        };

        // Release spawn permit — process is running, keychain access is past
        drop(_spawn_permit);

        // Register child PID so the signal handler can SIGTERM it on shutdown
        // (kill_on_drop alone doesn't fire on SIGTERM/SIGKILL of nebo).
        let child_pid = child.id().unwrap_or(0);
        napp::child_guard::register_child(child_pid);

        // Track spawn time so we only reset backoff if process ran long enough
        let spawn_time = std::time::Instant::now();

        let stdout = child.stdout.take().expect("stdout piped");
        let mut lines = BufReader::new(stdout).lines();

        // Spawn a task to log and collect stderr for auth error detection
        let stderr = child.stderr.take();
        let stderr_agent = agent_id.clone();
        let stderr_binding = binding_name.clone();
        let stderr_collected = Arc::new(tokio::sync::Mutex::new(String::new()));
        let stderr_buf = stderr_collected.clone();
        let stderr_handle = tokio::spawn(async move {
            if let Some(stderr) = stderr {
                let mut stderr_lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = stderr_lines.next_line().await {
                    // Sidecar stderr is its diagnostic/progress stream, not an error
                    // channel — log at debug. Auth errors are still detected below via
                    // the collected buffer + is_auth_error().
                    debug!(
                        agent = %stderr_agent,
                        binding = %stderr_binding,
                        "watch stderr: {}",
                        line
                    );
                    let mut buf = stderr_buf.lock().await;
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
        });

        // Read NDJSON lines until process exits or cancel
        loop {
            tokio::select! {
                line_result = lines.next_line() => {
                    match line_result {
                        Ok(Some(line)) => {
                            let line = line.trim().to_string();
                            if line.is_empty() {
                                continue;
                            }

                            // Parse JSON payload
                            let payload: serde_json::Value = match serde_json::from_str(&line) {
                                Ok(v) => v,
                                Err(e) => {
                                    warn!(
                                        agent = %agent_id,
                                        binding = %binding_name,
                                        error = %e,
                                        line = %line,
                                        "invalid JSON from watch process, skipping line"
                                    );
                                    continue;
                                }
                            };

                            // Auto-emit into EventBus if event-based watch
                            // Skip error payloads — plugins may output errors (e.g. DNS
                            // failures, auth errors) as JSON on stdout. These should NOT
                            // be emitted as events that trigger workflows.
                            if payload.get("error").is_some() {
                                warn!(
                                    agent = %agent_id,
                                    binding = %binding_name,
                                    "skipping error payload from watch process (not an event)"
                                );
                                continue;
                            }

                            if let Some((ref base_source, multiplexed)) = auto_emit {
                                let (event_source, event_payload) = if multiplexed {
                                    if let Some(event_name) = payload.get("event").and_then(|v| v.as_str()) {
                                        let source = format!("{}.{}", cfg.plugin, event_name);
                                        let mut cleaned = payload.clone();
                                        if let Some(obj) = cleaned.as_object_mut() {
                                            obj.remove("event");
                                        }
                                        (source, cleaned)
                                    } else {
                                        (base_source.clone(), payload.clone())
                                    }
                                } else {
                                    (base_source.clone(), payload.clone())
                                };

                                // Deduplicate: hash the (source + payload) and skip if seen recently.
                                // Uses DB-backed dedup so fingerprints survive restarts.
                                let fingerprint = hash_text(&format!("{}:{}", event_source, event_payload));
                                let is_dup = store.check_event_dedup(&fingerprint, 10 * 60).unwrap_or(false);
                                if is_dup {
                                    debug!(
                                        agent = %agent_id,
                                        binding = %binding_name,
                                        event_source = %event_source,
                                        "skipping duplicate event (seen within dedup window)"
                                    );
                                } else {
                                    let _ = store.record_event_dedup(&fingerprint, &event_source);

                                    let timestamp = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs();

                                    event_bus.emit(tools::events::Event {
                                        source: event_source.clone(),
                                        payload: event_payload,
                                        origin: format!("plugin:{}:{}", cfg.plugin, binding_name),
                                        timestamp,
                                    });

                                    debug!(
                                        agent = %agent_id,
                                        binding = %binding_name,
                                        event_source = %event_source,
                                        "auto-emitted plugin event"
                                    );
                                }
                            }

                            // Run inline workflow if activities are defined
                            if let Some(ref def) = def_json {
                                let mut run_inputs = base_inputs.clone();
                                if let Some(obj) = run_inputs.as_object_mut() {
                                    obj.insert("_watch_payload".to_string(), payload);
                                    obj.insert("_watch_source".to_string(), serde_json::Value::String(cfg.plugin.clone()));
                                }

                                let watch_detail = Some(format!("{}:{}", binding_name, cfg.plugin));
                                match workflow_manager.run_inline(
                                    def.clone(),
                                    run_inputs,
                                    "watch",
                                    watch_detail,
                                    &agent_id,
                                    emit_source.clone(),
                                ).await {
                                    Ok(run_id) => {
                                        info!(
                                            agent = %agent_id,
                                            binding = %binding_name,
                                            run_id = %run_id,
                                            "watch triggered inline workflow"
                                        );
                                    }
                                    Err(e) => {
                                        warn!(
                                            agent = %agent_id,
                                            binding = %binding_name,
                                            error = %e,
                                            "watch inline workflow run failed"
                                        );
                                        notify_crate::send("Nebo", &format!("{} failed: {}", binding_name, e));
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            // stdout closed — process exiting
                            info!(agent = %agent_id, binding = %binding_name, "watch process stdout closed");
                            break;
                        }
                        Err(e) => {
                            warn!(agent = %agent_id, binding = %binding_name, error = %e, "watch stdout read error");
                            break;
                        }
                    }
                }
                _ = cancel.cancelled() => {
                    info!(agent = %agent_id, binding = %binding_name, "watch cancelled, killing process");
                    let _ = child.kill().await;
                    stderr_handle.abort();
                    return;
                }
            }
        }

        // Wait for the child to finish
        let _ = child.wait().await;
        napp::child_guard::unregister_child(child_pid);
        // Give stderr task a moment to flush, then stop it
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        stderr_handle.abort();

        if cancel.is_cancelled() {
            break;
        }

        // Check if the process failed due to a plugin auth error
        {
            let stderr_text = stderr_collected.lock().await;
            if tools::plugin_tool::is_auth_error(&stderr_text) {
                warn!(
                    agent = %agent_id,
                    binding = %binding_name,
                    plugin = %cfg.plugin,
                    "watch failed: plugin not authenticated, pausing until auth completes"
                );

                let notif_id = format!("auth-required:{}:{}", agent_id, cfg.plugin);
                if let Err(e) = store.create_notification_if_not_exists(
                    &notif_id,
                    "",
                    "warning",
                    &format!("{} needs authentication", cfg.plugin),
                    Some(&format!(
                        "Connect your {} account to enable automated workflows. Go to Settings → Plugins.",
                        cfg.plugin
                    )),
                    Some("/settings/plugins"),
                    None,
                ) {
                    warn!(error = %e, "failed to create auth notification");
                }

                // Broadcast so notification bell updates in real-time
                if let Some(ref notify) = notify_fn {
                    notify(
                        "notification",
                        serde_json::json!({
                            "id": notif_id,
                            "type": "warning",
                            "title": format!("{} needs authentication", cfg.plugin),
                            "body": format!("Connect your {} account to enable automated workflows.", cfg.plugin),
                            "link": "/settings/plugins",
                        }),
                    );
                }

                break; // Stop retrying — worker restarted via plugin_auth_complete
            }
        }

        // Only reset backoff if process ran for >30s (not a fast crash)
        if spawn_time.elapsed() > std::time::Duration::from_secs(30) {
            backoff_secs = cfg.restart_delay_secs;
        }

        // Restart with backoff
        info!(
            agent = %agent_id,
            binding = %binding_name,
            backoff_secs = backoff_secs,
            "watch process exited, restarting"
        );
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
            _ = cancel.cancelled() => break,
        }
        backoff_secs = (backoff_secs * 2).min(max_backoff_secs);
    }
}

// ---------------------------------------------------------------------------
// Built-in folder watcher trigger
// ---------------------------------------------------------------------------

/// Deserialized folder trigger config from DB's trigger_config JSON.
#[derive(Debug, Clone, serde::Deserialize)]
struct FolderTriggerConfig {
    path: String,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default = "default_true")]
    recursive: bool,
    #[serde(default = "default_debounce_secs")]
    debounce_secs: u64,
}

fn default_true() -> bool {
    true
}

fn default_debounce_secs() -> u64 {
    2
}

/// Built-in folder watcher that uses OS filesystem notifications (notify crate)
/// to detect new/changed files. Only fires a workflow when files actually change,
/// unlike the heartbeat approach which polls on a fixed interval.
///
/// Events are debounced: rapid changes within `debounce_secs` are aggregated into
/// a single workflow trigger containing all affected file paths.
async fn folder_watch_loop(
    watch_path: String,
    cfg: FolderTriggerConfig,
    def_json: String,
    base_inputs: serde_json::Value,
    agent_id: String,
    binding_name: String,
    emit_source: Option<String>,
    workflow_manager: Arc<dyn WorkflowManager>,
    cancel: CancellationToken,
    event_bus: EventBus,
    store: Arc<Store>,
) {
    use notify::{Event, EventKind, RecursiveMode, Watcher};
    use tokio::sync::mpsc;

    let path = std::path::PathBuf::from(&watch_path);
    if !path.exists() {
        warn!(
            agent = %agent_id,
            binding = %binding_name,
            path = %watch_path,
            "folder watch path does not exist, waiting for creation"
        );
        loop {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                    if path.exists() { break; }
                }
                _ = cancel.cancelled() => return,
            }
        }
    }

    let (tx, mut rx) = mpsc::channel::<notify::Result<Event>>(64);

    let mut watcher = match notify::RecommendedWatcher::new(
        move |res| {
            let _ = tx.blocking_send(res);
        },
        notify::Config::default().with_poll_interval(std::time::Duration::from_secs(2)),
    ) {
        Ok(w) => w,
        Err(e) => {
            warn!(
                agent = %agent_id,
                binding = %binding_name,
                error = %e,
                "failed to create filesystem watcher"
            );
            return;
        }
    };

    let mode = if cfg.recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    if let Err(e) = watcher.watch(&path, mode) {
        warn!(
            agent = %agent_id,
            binding = %binding_name,
            error = %e,
            path = %watch_path,
            "failed to start watching path"
        );
        return;
    }

    info!(
        agent = %agent_id,
        binding = %binding_name,
        path = %watch_path,
        recursive = cfg.recursive,
        "folder watcher started"
    );

    let debounce = std::time::Duration::from_secs(cfg.debounce_secs);
    let mut pending_files: Vec<std::path::PathBuf> = Vec::new();
    let mut debounce_deadline: Option<tokio::time::Instant> = None;

    loop {
        let sleep_fut = match debounce_deadline {
            Some(deadline) => tokio::time::sleep_until(deadline),
            None => tokio::time::sleep(std::time::Duration::from_secs(3600)),
        };

        tokio::select! {
            result = rx.recv() => {
                match result {
                    Some(Ok(event)) => {
                        let dominated = matches!(
                            event.kind,
                            EventKind::Create(_) | EventKind::Modify(_)
                        );
                        if !dominated {
                            continue;
                        }

                        // Filter by extension if configured
                        let matched: Vec<_> = event.paths.into_iter().filter(|p| {
                            if cfg.extensions.is_empty() {
                                return true;
                            }
                            p.extension()
                                .and_then(|e| e.to_str())
                                .map(|ext| cfg.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)))
                                .unwrap_or(false)
                        }).collect();

                        if matched.is_empty() {
                            continue;
                        }

                        for p in matched {
                            if !pending_files.contains(&p) {
                                pending_files.push(p);
                            }
                        }

                        debounce_deadline = Some(tokio::time::Instant::now() + debounce);
                    }
                    Some(Err(e)) => {
                        warn!(
                            agent = %agent_id,
                            binding = %binding_name,
                            error = %e,
                            "filesystem watcher error"
                        );
                    }
                    None => {
                        warn!(
                            agent = %agent_id,
                            binding = %binding_name,
                            "filesystem watcher channel closed"
                        );
                        break;
                    }
                }
            }
            _ = sleep_fut, if debounce_deadline.is_some() => {
                debounce_deadline = None;
                let files: Vec<String> = pending_files
                    .drain(..)
                    .filter_map(|p| p.to_str().map(String::from))
                    .collect();

                if files.is_empty() {
                    continue;
                }

                // Dedup via DB-backed fingerprint
                let fingerprint = hash_text(&format!(
                    "folder:{}:{}",
                    binding_name,
                    files.join(",")
                ));
                if store
                    .check_event_dedup(&fingerprint, 10 * 60)
                    .unwrap_or(false)
                {
                    debug!(
                        agent = %agent_id,
                        binding = %binding_name,
                        "skipping duplicate folder event"
                    );
                    continue;
                }
                let _ = store.record_event_dedup(
                    &fingerprint,
                    &format!("folder:{}", binding_name),
                );

                let payload = serde_json::json!({
                    "files": files,
                    "watch_path": watch_path,
                    "event_type": "file_change",
                });

                // Emit event if configured
                if let Some(ref source) = emit_source {
                    let timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    event_bus.emit(tools::events::Event {
                        source: source.clone(),
                        payload: payload.clone(),
                        origin: format!("folder:{}", binding_name),
                        timestamp,
                    });
                }

                // Run inline workflow
                let mut run_inputs = base_inputs.clone();
                if let Some(obj) = run_inputs.as_object_mut() {
                    obj.insert(
                        "_watch_payload".to_string(),
                        payload,
                    );
                    obj.insert(
                        "_watch_source".to_string(),
                        serde_json::Value::String("folder".into()),
                    );
                }

                let detail = Some(format!("{}:folder", binding_name));
                match workflow_manager
                    .run_inline(
                        def_json.clone(),
                        run_inputs,
                        "folder",
                        detail,
                        &agent_id,
                        emit_source.clone(),
                    )
                    .await
                {
                    Ok(run_id) => {
                        info!(
                            agent = %agent_id,
                            binding = %binding_name,
                            run_id = %run_id,
                            "folder change triggered workflow"
                        );
                    }
                    Err(e) => {
                        warn!(
                            agent = %agent_id,
                            binding = %binding_name,
                            error = %e,
                            "folder watch workflow failed"
                        );
                        notify_crate::send(
                            "Nebo",
                            &format!("{} failed: {}", binding_name, e),
                        );
                    }
                }
            }
            _ = cancel.cancelled() => {
                info!(
                    agent = %agent_id,
                    binding = %binding_name,
                    "folder watcher cancelled"
                );
                break;
            }
        }
    }
}

/// Persistent channel loop — spawns a plugin process that bridges an external
/// messaging platform to the agent's chat.
///
/// The plugin binary writes inbound messages as NDJSON to stdout.
/// This loop reads each message, dispatches it to `run_chat()` via the
/// `ChannelDispatcher`, and writes the response as NDJSON to the plugin's stdin.
///
/// Lifecycle is identical to `watch_loop`: auto-restart with backoff on crash,
/// auth error detection, cancellation token.
async fn channel_loop(
    binary_path: std::path::PathBuf,
    plugin_slug: String,
    channel_def: napp::plugin::PluginChannel,
    plugin_store: Arc<PluginStore>,
    agent_id: String,
    agent_name: String,
    dispatcher: Arc<dyn ChannelDispatcher>,
    cancel: CancellationToken,
    store: Arc<Store>,
    notify_fn: Option<NotifyFn>,
    spawn_semaphore: Arc<tokio::sync::Semaphore>,
    agent_config: std::collections::HashMap<String, String>,
) {
    let channel_name = plugin_slug.clone();
    let mut backoff_secs = channel_def.restart_delay_secs;
    let max_backoff_secs = 300;

    loop {
        if cancel.is_cancelled() {
            break;
        }

        let args = match shlex::split(&channel_def.command) {
            Some(a) => a,
            None => {
                error!(
                    agent = %agent_id,
                    channel = %channel_name,
                    command = %channel_def.command,
                    "failed to parse channel command, stopping"
                );
                break;
            }
        };

        let runtime = napp::PluginRuntime::new(
            &plugin_slug,
            binary_path.clone(),
            plugin_store.clone(),
        )
        .with_home()
        .with_agent_config(agent_config.clone())
        .with_permissions();

        let mut cmd = tokio::process::Command::new(&binary_path);
        cmd.args(&args);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.stdin(Stdio::piped());
        cmd.kill_on_drop(true);
        cmd.env_clear();
        for (k, v) in runtime.build_env() {
            cmd.env(k, v);
        }

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let has_own_creds = !agent_config.is_empty();
        info!(
            agent = %agent_id,
            channel = %channel_name,
            binary = %binary_path.display(),
            command = %channel_def.command,
            per_agent_creds = %has_own_creds,
            "spawning channel process"
        );

        let _spawn_permit = spawn_semaphore.acquire().await;

        // Reap any pre-existing instance of this channel bridge — orphans
        // would race for the same upstream socket (e.g. Slack Socket Mode),
        // each posting placeholders for the same inbound message.
        napp::child_guard::reap_existing_for(&binary_path);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                error!(
                    agent = %agent_id,
                    channel = %channel_name,
                    error = %e,
                    "failed to spawn channel process, retrying in {}s",
                    backoff_secs
                );
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
                    _ = cancel.cancelled() => break,
                }
                backoff_secs = (backoff_secs * 2).min(max_backoff_secs);
                continue;
            }
        };

        drop(_spawn_permit);

        let child_pid = child.id().unwrap_or(0);
        napp::child_guard::register_child(child_pid);

        let spawn_time = std::time::Instant::now();

        let stdout = child.stdout.take().expect("stdout piped");
        let stdin = child.stdin.take().expect("stdin piped");
        let mut lines = BufReader::new(stdout).lines();
        let stdin = Arc::new(tokio::sync::Mutex::new(stdin));

        // Register an agent-side stdin forwarder for plugin_tool messaging ops
        // (post/upload/dm/reply). The forwarder reads ops off an mpsc channel
        // and writes them as NDJSON lines through the same `stdin` mutex the
        // inbound-reply writer uses — the mutex serializes the two producers
        // so they never interleave inside a single line.
        let (bridge_tx, mut bridge_rx) =
            tokio::sync::mpsc::channel::<serde_json::Value>(64);
        let bridge_stdin = stdin.clone();
        let bridge_agent = agent_id.clone();
        let bridge_channel = channel_name.clone();
        let bridge_forwarder = tokio::spawn(async move {
            while let Some(op) = bridge_rx.recv().await {
                let line = match serde_json::to_string(&op) {
                    Ok(s) => format!("{s}\n"),
                    Err(e) => {
                        warn!(
                            agent = %bridge_agent,
                            channel = %bridge_channel,
                            error = %e,
                            "bridge forwarder: failed to serialize op"
                        );
                        continue;
                    }
                };
                let mut guard = bridge_stdin.lock().await;
                if let Err(e) = guard.write_all(line.as_bytes()).await {
                    warn!(
                        agent = %bridge_agent,
                        channel = %bridge_channel,
                        error = %e,
                        "bridge forwarder: failed to write to plugin stdin"
                    );
                    break;
                }
                let _ = guard.flush().await;
            }
        });

        // Insert into the global bridge registry so plugin_tool can route ops
        // through this bridge. Removed on child exit / cancel below.
        // `pending_ops` correlates op req_ids with their `op_result` events
        // from the bridge's stdout — see `channel_bridge.rs` for the protocol.
        let bridge_key = tools::channel_bridge_key(&agent_id, &plugin_slug);
        let pending_ops = tools::new_pending_ops();
        if let Some(registry) = tools::channel_bridges() {
            let handle = tools::ChannelBridgeHandle {
                stdin_tx: bridge_tx.clone(),
                agent_id: agent_id.clone(),
                plugin_slug: plugin_slug.clone(),
                pending_ops: pending_ops.clone(),
            };
            registry.write().await.insert(bridge_key.clone(), handle);
            info!(
                agent = %agent_id,
                channel = %channel_name,
                key = %bridge_key,
                "channel bridge: registered for messaging ops"
            );
        } else {
            warn!(
                agent = %agent_id,
                channel = %channel_name,
                "channel bridge: registry not wired yet — bridge spawned without registration"
            );
        }

        // Stderr collector for auth error detection
        let stderr = child.stderr.take();
        let stderr_agent = agent_id.clone();
        let stderr_channel = channel_name.clone();
        let stderr_collected = Arc::new(tokio::sync::Mutex::new(String::new()));
        let stderr_buf = stderr_collected.clone();
        let stderr_handle = tokio::spawn(async move {
            if let Some(stderr) = stderr {
                let mut stderr_lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = stderr_lines.next_line().await {
                    // Sidecar stderr is its diagnostic/progress stream, not an error
                    // channel — log at debug. Auth errors are still detected below via
                    // the collected buffer + is_auth_error().
                    debug!(
                        agent = %stderr_agent,
                        channel = %stderr_channel,
                        "channel stderr: {}",
                        line
                    );
                    let mut buf = stderr_buf.lock().await;
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
        });

        // Bridge liveness watchdog. Per the canonical bridge protocol every
        // channel plugin emits `{"event":"keepalive", "status":...}` on
        // stdout at least every 10s. If we go > 30s without seeing one, the
        // bridge process is presumed hung — cancel the inner loop so we
        // kill + respawn. Each plugin handles its own protocol-level
        // keepalive (Slack WS ping, future Discord op-1 heartbeat, IMAP
        // NOOP); Nebo only watches the cross-plugin keepalive event.
        // See docs/publishers-guide/channel-plugins.md "Bridge Keepalive".
        let last_keepalive = Arc::new(tokio::sync::Mutex::new(std::time::Instant::now()));
        let bridge_stale = CancellationToken::new();
        let watchdog_last = last_keepalive.clone();
        let watchdog_signal = bridge_stale.clone();
        let watchdog_outer = cancel.clone();
        let watchdog_agent = agent_id.clone();
        let watchdog_channel = channel_name.clone();
        let watchdog_handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(10));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Skip the immediate tick — give the bridge time to emit its
            // first keepalive (which our initial timestamp already covers
            // for a 10s grace).
            tick.tick().await;
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        let elapsed = watchdog_last.lock().await.elapsed();
                        if elapsed > std::time::Duration::from_secs(30) {
                            warn!(
                                agent = %watchdog_agent,
                                channel = %watchdog_channel,
                                elapsed_secs = elapsed.as_secs(),
                                "bridge watchdog: no keepalive for > 30s, triggering respawn"
                            );
                            watchdog_signal.cancel();
                            return;
                        }
                    }
                    _ = watchdog_outer.cancelled() => return,
                }
            }
        });

        // Read inbound NDJSON, dispatch to agent, write response to stdin
        loop {
            tokio::select! {
                line_result = lines.next_line() => {
                    match line_result {
                        Ok(Some(line)) => {
                            let line = line.trim().to_string();
                            if line.is_empty() {
                                continue;
                            }

                            let payload: serde_json::Value = match serde_json::from_str(&line) {
                                Ok(v) => v,
                                Err(e) => {
                                    warn!(
                                        agent = %agent_id,
                                        channel = %channel_name,
                                        error = %e,
                                        "invalid JSON from channel process, skipping"
                                    );
                                    continue;
                                }
                            };

                            // Bridge keepalive: reset watchdog. Status is
                            // logged for observability; future versions can
                            // surface it in the UI ("Slack reconnecting...")
                            // but Nebo doesn't act on the value yet — a
                            // bridge in `disconnected` state is still
                            // alive and reconnecting under its own logic.
                            if payload.get("event").and_then(|v| v.as_str())
                                == Some("keepalive")
                            {
                                *last_keepalive.lock().await = std::time::Instant::now();
                                if let Some(status) = payload
                                    .get("status")
                                    .and_then(|v| v.as_str())
                                {
                                    debug!(
                                        agent = %agent_id,
                                        channel = %channel_name,
                                        status = %status,
                                        "bridge keepalive"
                                    );
                                }
                                continue;
                            }

                            // Bridge → Nebo op_result correlation: when the
                            // bridge finishes a stdin-routed op (post / upload /
                            // dm / reply), it writes
                            // `{event:"op_result", req_id, ok, error?}` here.
                            // Route it to the waiting plugin_tool caller via
                            // the per-bridge pending_ops map.
                            if payload.get("event").and_then(|v| v.as_str())
                                == Some("op_result")
                            {
                                if let Some(req_id) = payload
                                    .get("req_id")
                                    .and_then(|v| v.as_str())
                                {
                                    let ok = payload
                                        .get("ok")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false);
                                    let error = payload
                                        .get("error")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string());
                                    if let Some(sender) =
                                        pending_ops.lock().await.remove(req_id)
                                    {
                                        let _ = sender.send(tools::OpResult {
                                            ok,
                                            error,
                                        });
                                    }
                                }
                                continue;
                            }

                            // Skip error payloads
                            if payload.get("error").is_some() {
                                warn!(
                                    agent = %agent_id,
                                    channel = %channel_name,
                                    "skipping error payload from channel process"
                                );
                                continue;
                            }

                            let text = payload.get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();

                            if text.is_empty() {
                                debug!(
                                    agent = %agent_id,
                                    channel = %channel_name,
                                    "channel message had empty text, skipping"
                                );
                                continue;
                            }

                            // Build session key from channel + platform channel ID
                            let platform_channel = payload.get("channel")
                                .and_then(|v| v.as_str())
                                .unwrap_or("default");
                            let session_key = format!(
                                "agent:{}:{}:{}",
                                agent_id, channel_name, platform_channel
                            );

                            // Build channel context — tools (notably the plugin
                            // tool) use this to target the right channel/thread
                            // for uploads etc. without the agent having to look
                            // up IDs.
                            let channel_ctx = tools::ChannelContext {
                                kind: channel_name.clone(),
                                channel_id: platform_channel.to_string(),
                                thread_ts: payload
                                    .get("thread_ts")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string()),
                            };

                            info!(
                                agent = %agent_id,
                                channel = %channel_name,
                                session = %session_key,
                                text_len = %text.len(),
                                "channel inbound: received message"
                            );
                            debug!(
                                agent = %agent_id,
                                channel = %channel_name,
                                session = %session_key,
                                text = %text,
                                payload = %payload,
                                "channel inbound: full payload"
                            );

                            // Dispatch to run_chat and get response
                            let dispatch = dispatcher.clone();
                            let agent = agent_id.clone();
                            let ch = channel_name.clone();
                            let stdin_ref = stdin.clone();
                            let reply_payload = payload.clone();

                            let agent_display = agent_name.clone();
                            tokio::spawn(async move {
                                // Dispatch with silent retries on transient failures.
                                // Root cause of the original "Thinking..." hang was missing HTTP
                                // timeouts in the AI provider clients — fixed in crates/ai/src/http.rs.
                                // These retries handle anything that slips through (provider 5xx,
                                // dropped streams) without bothering the user.
                                const MAX_ATTEMPTS: u32 = 3;
                                const BACKOFFS_MS: &[u64] = &[150, 400];

                                let mut response: Option<String> = None;
                                for attempt in 0..MAX_ATTEMPTS {
                                    let started = std::time::Instant::now();
                                    info!(
                                        agent = %agent,
                                        channel = %ch,
                                        session = %session_key,
                                        attempt = attempt + 1,
                                        "channel dispatch: start"
                                    );
                                    let result = dispatch.dispatch(&agent, &session_key, channel_ctx.clone(), &text).await;
                                    let elapsed_ms = started.elapsed().as_millis();
                                    match result {
                                        Ok(r) if !r.is_empty() => {
                                            info!(
                                                agent = %agent,
                                                channel = %ch,
                                                session = %session_key,
                                                attempt = attempt + 1,
                                                elapsed_ms,
                                                response_len = r.len(),
                                                "channel dispatch: ok"
                                            );
                                            debug!(
                                                agent = %agent,
                                                channel = %ch,
                                                session = %session_key,
                                                response = %r,
                                                "channel dispatch: full response"
                                            );
                                            response = Some(r);
                                            break;
                                        }
                                        Ok(_) => {
                                            warn!(
                                                agent = %agent,
                                                channel = %ch,
                                                session = %session_key,
                                                attempt = attempt + 1,
                                                elapsed_ms,
                                                "channel dispatch: returned empty response"
                                            );
                                        }
                                        Err(e) => {
                                            warn!(
                                                agent = %agent,
                                                channel = %ch,
                                                session = %session_key,
                                                attempt = attempt + 1,
                                                elapsed_ms,
                                                error = %e,
                                                "channel dispatch: failed"
                                            );
                                        }
                                    }
                                    if let Some(&delay) = BACKOFFS_MS.get(attempt as usize) {
                                        tokio::time::sleep(
                                            std::time::Duration::from_millis(delay),
                                        )
                                        .await;
                                    }
                                }

                                let Some(response) = response else {
                                    // All retries exhausted — log and stay silent rather than
                                    // posting an apology. The "_Thinking..._" placeholder remains
                                    // on the user's screen. Operators see this in logs.
                                    warn!(
                                        agent = %agent,
                                        channel = %ch,
                                        session = %session_key,
                                        attempts = MAX_ATTEMPTS,
                                        "channel dispatch: exhausted retries; no reply posted"
                                    );
                                    return;
                                };

                                // Build reply: echo back routing fields + response text.
                                // The `op: "reply"` discriminator selects the bridge's
                                // reply handler — see docs/publishers-guide/channel-plugins.md
                                // for the full op protocol (reply/post/upload/dm).
                                //
                                // `user_ts` is the inbound user message's timestamp; the
                                // bridge uses it to clear the 👀 working-indicator reaction
                                // before posting a fresh response. This replaces the old
                                // placeholder_ts/chat.update pattern which caused
                                // `(edited)` and suppressed Slack notifications.
                                let mut reply = serde_json::Map::new();
                                reply.insert(
                                    "op".into(),
                                    serde_json::Value::String("reply".into()),
                                );
                                for key in &["channel", "thread_ts", "user"] {
                                    if let Some(v) = reply_payload.get(*key) {
                                        reply.insert((*key).to_string(), v.clone());
                                    }
                                }
                                if let Some(v) = reply_payload.get("ts") {
                                    reply.insert("user_ts".into(), v.clone());
                                }
                                reply.insert("text".into(), serde_json::Value::String(response));
                                reply.insert(
                                    "username".into(),
                                    serde_json::Value::String(agent_display),
                                );

                                let reply_line = format!("{}\n", serde_json::Value::Object(reply));
                                let reply_bytes = reply_line.len();
                                let write_started = std::time::Instant::now();
                                let mut stdin_lock = stdin_ref.lock().await;
                                match stdin_lock.write_all(reply_line.as_bytes()).await {
                                    Ok(()) => {
                                        let _ = stdin_lock.flush().await;
                                        info!(
                                            agent = %agent,
                                            channel = %ch,
                                            session = %session_key,
                                            bytes = reply_bytes,
                                            write_ms = write_started.elapsed().as_millis(),
                                            "channel outbound: reply written to plugin stdin"
                                        );
                                        debug!(
                                            agent = %agent,
                                            channel = %ch,
                                            session = %session_key,
                                            payload = %reply_line.trim(),
                                            "channel outbound: full payload"
                                        );
                                    }
                                    Err(e) => {
                                        warn!(
                                            agent = %agent,
                                            channel = %ch,
                                            session = %session_key,
                                            error = %e,
                                            "channel outbound: failed to write reply to plugin stdin"
                                        );
                                    }
                                }
                            });
                        }
                        Ok(None) => {
                            info!(agent = %agent_id, channel = %channel_name, "channel process stdout closed");
                            break;
                        }
                        Err(e) => {
                            warn!(agent = %agent_id, channel = %channel_name, error = %e, "channel stdout read error");
                            break;
                        }
                    }
                }
                _ = bridge_stale.cancelled() => {
                    warn!(
                        agent = %agent_id,
                        channel = %channel_name,
                        "channel bridge flagged stale by keepalive watchdog, killing for respawn"
                    );
                    let _ = child.kill().await;
                    break;
                }
                _ = cancel.cancelled() => {
                    info!(agent = %agent_id, channel = %channel_name, "channel cancelled, killing process");
                    let _ = child.kill().await;
                    stderr_handle.abort();
                    bridge_forwarder.abort();
                    watchdog_handle.abort();
                    if let Some(registry) = tools::channel_bridges() {
                        registry.write().await.remove(&bridge_key);
                    }
                    return;
                }
            }
        }

        let _ = child.wait().await;
        napp::child_guard::unregister_child(child_pid);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        stderr_handle.abort();
        bridge_forwarder.abort();
        watchdog_handle.abort();
        if let Some(registry) = tools::channel_bridges() {
            registry.write().await.remove(&bridge_key);
        }

        if cancel.is_cancelled() {
            break;
        }

        // Check for auth errors
        {
            let stderr_text = stderr_collected.lock().await;
            if tools::plugin_tool::is_auth_error(&stderr_text) {
                warn!(
                    agent = %agent_id,
                    channel = %channel_name,
                    plugin = %plugin_slug,
                    "channel failed: plugin not authenticated, pausing until auth completes"
                );

                let notif_id = format!("auth-required:{}:{}", agent_id, plugin_slug);
                if let Err(e) = store.create_notification_if_not_exists(
                    &notif_id,
                    "",
                    "warning",
                    &format!("{} needs authentication", plugin_slug),
                    Some(&format!(
                        "Connect your {} account to enable the {} channel. Go to Settings → Plugins.",
                        plugin_slug, channel_name
                    )),
                    Some("/settings/plugins"),
                    None,
                ) {
                    warn!(error = %e, "failed to create auth notification");
                }

                if let Some(ref notify) = notify_fn {
                    notify(
                        "notification",
                        serde_json::json!({
                            "id": notif_id,
                            "type": "warning",
                            "title": format!("{} needs authentication", plugin_slug),
                            "body": format!("Connect your {} account to enable the {} channel.", plugin_slug, channel_name),
                            "link": "/settings/plugins",
                        }),
                    );
                }

                break;
            }
        }

        if spawn_time.elapsed() > std::time::Duration::from_secs(30) {
            backoff_secs = channel_def.restart_delay_secs;
        }

        info!(
            agent = %agent_id,
            channel = %channel_name,
            backoff_secs = backoff_secs,
            "channel process exited, restarting"
        );
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
            _ = cancel.cancelled() => break,
        }
        backoff_secs = (backoff_secs * 2).min(max_backoff_secs);
    }
}

/// Shared channel loop — one bridge process serves all agents for a plugin.
///
/// Messages are routed to agents by matching agent names in the message text.
/// Each agent's reply is posted with its display name as the username.
async fn shared_channel_loop(
    binary_path: std::path::PathBuf,
    plugin_slug: String,
    channel_def: napp::plugin::PluginChannel,
    plugin_store: Arc<PluginStore>,
    shared_bridges: Arc<SharedBridgeRegistry>,
    dispatcher: Arc<dyn ChannelDispatcher>,
    cancel: CancellationToken,
    notify_fn: Option<NotifyFn>,
    spawn_semaphore: Arc<tokio::sync::Semaphore>,
) {
    let channel_name = plugin_slug.clone();
    let mut backoff_secs = channel_def.restart_delay_secs;
    let max_backoff_secs = 300;

    loop {
        if cancel.is_cancelled() {
            break;
        }

        let args = match shlex::split(&channel_def.command) {
            Some(a) => a,
            None => {
                error!(
                    plugin = %plugin_slug,
                    command = %channel_def.command,
                    "failed to parse shared channel command, stopping"
                );
                break;
            }
        };

        let runtime = napp::PluginRuntime::new(
            &plugin_slug,
            binary_path.clone(),
            plugin_store.clone(),
        )
        .with_home()
        .with_permissions();

        let mut cmd = tokio::process::Command::new(&binary_path);
        cmd.args(&args);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.stdin(Stdio::piped());
        cmd.kill_on_drop(true);
        cmd.env_clear();
        for (k, v) in runtime.build_env() {
            cmd.env(k, v);
        }

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        info!(
            plugin = %plugin_slug,
            binary = %binary_path.display(),
            "spawning shared channel process"
        );

        let _spawn_permit = spawn_semaphore.acquire().await;

        // Reap any pre-existing instance of this shared channel bridge.
        napp::child_guard::reap_existing_for(&binary_path);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                error!(
                    plugin = %plugin_slug,
                    error = %e,
                    "failed to spawn shared channel process, retrying in {}s",
                    backoff_secs
                );
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
                    _ = cancel.cancelled() => break,
                }
                backoff_secs = (backoff_secs * 2).min(max_backoff_secs);
                continue;
            }
        };

        drop(_spawn_permit);

        let child_pid = child.id().unwrap_or(0);
        napp::child_guard::register_child(child_pid);

        let spawn_time = std::time::Instant::now();

        let stdout = child.stdout.take().expect("stdout piped");
        let stdin = child.stdin.take().expect("stdin piped");
        let stdin = Arc::new(tokio::sync::Mutex::new(stdin));
        let mut lines = BufReader::new(stdout).lines();

        // Mark bridge as running in the shared registry
        shared_bridges.mark_started(&plugin_slug, cancel.clone()).await;

        // Stderr collector
        let stderr = child.stderr.take();
        let stderr_channel = channel_name.clone();
        let stderr_collected = Arc::new(tokio::sync::Mutex::new(String::new()));
        let stderr_buf = stderr_collected.clone();
        let stderr_handle = tokio::spawn(async move {
            if let Some(stderr) = stderr {
                let mut stderr_lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = stderr_lines.next_line().await {
                    // Sidecar stderr is its diagnostic/progress stream, not an error
                    // channel — log at debug. Auth errors are still detected below via
                    // the collected buffer + is_auth_error().
                    debug!(
                        channel = %stderr_channel,
                        "shared channel stderr: {}",
                        line
                    );
                    let mut buf = stderr_buf.lock().await;
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
        });

        // Bridge liveness watchdog — same contract as per-agent channel_loop.
        // Required across plugin types because a hung shared bridge starves
        // every agent registered against it.
        let last_keepalive = Arc::new(tokio::sync::Mutex::new(std::time::Instant::now()));
        let bridge_stale = CancellationToken::new();
        let watchdog_last = last_keepalive.clone();
        let watchdog_signal = bridge_stale.clone();
        let watchdog_outer = cancel.clone();
        let watchdog_channel = channel_name.clone();
        let watchdog_handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(10));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.tick().await;
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        let elapsed = watchdog_last.lock().await.elapsed();
                        if elapsed > std::time::Duration::from_secs(30) {
                            warn!(
                                channel = %watchdog_channel,
                                elapsed_secs = elapsed.as_secs(),
                                "shared bridge watchdog: no keepalive for > 30s, triggering respawn"
                            );
                            watchdog_signal.cancel();
                            return;
                        }
                    }
                    _ = watchdog_outer.cancelled() => return,
                }
            }
        });

        // Read inbound NDJSON, route to correct agent, write response to stdin
        loop {
            tokio::select! {
                line_result = lines.next_line() => {
                    match line_result {
                        Ok(Some(line)) => {
                            let line = line.trim().to_string();
                            if line.is_empty() {
                                continue;
                            }

                            let payload: serde_json::Value = match serde_json::from_str(&line) {
                                Ok(v) => v,
                                Err(e) => {
                                    warn!(
                                        channel = %channel_name,
                                        error = %e,
                                        "invalid JSON from shared channel process"
                                    );
                                    continue;
                                }
                            };

                            // Bridge keepalive resets the watchdog. Must be
                            // checked BEFORE the catch-all event drop below.
                            if payload.get("event").and_then(|v| v.as_str())
                                == Some("keepalive")
                            {
                                *last_keepalive.lock().await = std::time::Instant::now();
                                continue;
                            }

                            if payload.get("error").is_some() || payload.get("event").is_some() {
                                continue;
                            }

                            let text = payload.get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();

                            if text.is_empty() {
                                continue;
                            }

                            // Route: find the best matching agent by name
                            let agents = shared_bridges.get_agents(&plugin_slug).await;
                            let (target_id, target_name) = route_to_agent(&text, &agents);

                            let platform_channel = payload.get("channel")
                                .and_then(|v| v.as_str())
                                .unwrap_or("default");
                            let session_key = format!(
                                "agent:{}:{}:{}",
                                target_id, channel_name, platform_channel
                            );

                            let channel_ctx = tools::ChannelContext {
                                kind: channel_name.clone(),
                                channel_id: platform_channel.to_string(),
                                thread_ts: payload
                                    .get("thread_ts")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string()),
                            };

                            let dispatch = dispatcher.clone();
                            let ch = channel_name.clone();
                            let stdin_ref = stdin.clone();
                            let reply_payload = payload.clone();

                            tokio::spawn(async move {
                                match dispatch.dispatch(&target_id, &session_key, channel_ctx, &text).await {
                                    Ok(response) => {
                                        if response.is_empty() {
                                            return;
                                        }
                                        let mut reply = serde_json::Map::new();
                                        reply.insert(
                                            "op".into(),
                                            serde_json::Value::String("reply".into()),
                                        );
                                        for key in &["channel", "thread_ts", "user"] {
                                            if let Some(v) = reply_payload.get(*key) {
                                                reply.insert((*key).to_string(), v.clone());
                                            }
                                        }
                                        if let Some(v) = reply_payload.get("ts") {
                                            reply.insert("user_ts".into(), v.clone());
                                        }
                                        reply.insert(
                                            "text".into(),
                                            serde_json::Value::String(response),
                                        );
                                        reply.insert(
                                            "username".into(),
                                            serde_json::Value::String(target_name),
                                        );

                                        let reply_line = format!(
                                            "{}\n",
                                            serde_json::Value::Object(reply)
                                        );
                                        let mut stdin_lock = stdin_ref.lock().await;
                                        if let Err(e) = stdin_lock.write_all(reply_line.as_bytes()).await {
                                            warn!(
                                                channel = %ch,
                                                error = %e,
                                                "failed to write reply to shared channel stdin"
                                            );
                                        }
                                        let _ = stdin_lock.flush().await;
                                    }
                                    Err(e) => {
                                        warn!(
                                            channel = %ch,
                                            error = %e,
                                            "shared channel dispatch failed"
                                        );
                                    }
                                }
                            });
                        }
                        Ok(None) => {
                            info!(channel = %channel_name, "shared channel stdout closed");
                            break;
                        }
                        Err(e) => {
                            warn!(channel = %channel_name, error = %e, "shared channel read error");
                            break;
                        }
                    }
                }
                _ = bridge_stale.cancelled() => {
                    warn!(
                        channel = %channel_name,
                        "shared channel bridge flagged stale by keepalive watchdog, killing for respawn"
                    );
                    let _ = child.kill().await;
                    break;
                }
                _ = cancel.cancelled() => {
                    info!(channel = %channel_name, "shared channel cancelled");
                    let _ = child.kill().await;
                    stderr_handle.abort();
                    watchdog_handle.abort();
                    return;
                }
            }
        }

        let _ = child.wait().await;
        napp::child_guard::unregister_child(child_pid);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        stderr_handle.abort();
        watchdog_handle.abort();

        if cancel.is_cancelled() {
            break;
        }

        // Check for auth errors
        {
            let stderr_text = stderr_collected.lock().await;
            if tools::plugin_tool::is_auth_error(&stderr_text) {
                warn!(
                    plugin = %plugin_slug,
                    "shared channel failed: plugin not authenticated"
                );

                if let Some(ref notify) = notify_fn {
                    notify(
                        "notification",
                        serde_json::json!({
                            "type": "warning",
                            "title": format!("{} needs authentication", plugin_slug),
                            "body": format!("Connect your {} account to enable channels.", plugin_slug),
                            "link": "/settings/plugins",
                        }),
                    );
                }
                break;
            }
        }

        shared_bridges.mark_stopped(&plugin_slug).await;

        if spawn_time.elapsed() > std::time::Duration::from_secs(30) {
            backoff_secs = channel_def.restart_delay_secs;
        }

        info!(
            channel = %channel_name,
            backoff_secs = backoff_secs,
            "shared channel process exited, restarting"
        );
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
            _ = cancel.cancelled() => break,
        }
        backoff_secs = (backoff_secs * 2).min(max_backoff_secs);
    }
}

/// Route an incoming message to the best matching agent.
///
/// Checks if the message text contains an agent name (case-insensitive).
/// Falls back to the first agent if no match is found.
fn route_to_agent(text: &str, agents: &HashMap<String, String>) -> (String, String) {
    let text_lower = text.to_lowercase();

    // Check for agent name mentions (case-insensitive)
    for (agent_id, agent_name) in agents {
        if text_lower.contains(&agent_name.to_lowercase()) {
            return (agent_id.clone(), agent_name.clone());
        }
    }

    // No match — use first agent (deterministic via sorted keys)
    if let Some((id, name)) = agents.iter().next() {
        return (id.clone(), name.clone());
    }

    // Shouldn't happen — but fallback
    ("default".to_string(), "Nebo".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("30m"), std::time::Duration::from_secs(1800));
    }

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("1h"), std::time::Duration::from_secs(3600));
    }

    #[test]
    fn test_parse_duration_combined() {
        assert_eq!(
            parse_duration("1h30m"),
            std::time::Duration::from_secs(5400)
        );
    }

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("90s"), std::time::Duration::from_secs(90));
    }

    #[test]
    fn test_parse_duration_bare_number() {
        assert_eq!(parse_duration("5"), std::time::Duration::from_secs(300));
    }

    #[test]
    fn test_parse_heartbeat_simple() {
        let (d, w) = parse_heartbeat("30m");
        assert_eq!(d, std::time::Duration::from_secs(1800));
        assert!(w.is_none());
    }

    #[test]
    fn test_parse_heartbeat_with_window() {
        let (d, w) = parse_heartbeat("30m|08:00-18:00");
        assert_eq!(d, std::time::Duration::from_secs(1800));
        let (start, end) = w.unwrap();
        assert_eq!(start, chrono::NaiveTime::from_hms_opt(8, 0, 0).unwrap());
        assert_eq!(end, chrono::NaiveTime::from_hms_opt(18, 0, 0).unwrap());
    }

    #[test]
    fn test_parse_time_window() {
        let w = parse_time_window("09:30-17:00").unwrap();
        assert_eq!(w.0, chrono::NaiveTime::from_hms_opt(9, 30, 0).unwrap());
        assert_eq!(w.1, chrono::NaiveTime::from_hms_opt(17, 0, 0).unwrap());
    }

    #[test]
    fn test_parse_time_window_invalid() {
        assert!(parse_time_window("invalid").is_none());
    }

    #[test]
    fn test_substitute_inputs() {
        let inputs = serde_json::json!({
            "gcp_project": "my-project-123",
            "poll_interval": "30"
        });
        let template = "gmail +watch --project {{gcp_project}} --poll-interval {{poll_interval}}";
        let result = substitute_inputs(template, &inputs);
        assert_eq!(
            result,
            "gmail +watch --project my-project-123 --poll-interval 30"
        );
    }

    #[test]
    fn test_substitute_inputs_no_match() {
        let inputs = serde_json::json!({});
        let template = "gmail +watch --project {{gcp_project}}";
        let result = substitute_inputs(template, &inputs);
        assert_eq!(result, template); // unchanged
    }

    #[test]
    fn test_watch_trigger_config_deserialize() {
        let json = r#"{"plugin":"gws","command":"gmail +watch","restart_delay_secs":10}"#;
        let cfg: WatchTriggerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.plugin, "gws");
        assert_eq!(cfg.command, "gmail +watch");
        assert!(cfg.event.is_none());
        assert!(!cfg.multiplexed);
        assert_eq!(cfg.restart_delay_secs, 10);
    }

    #[test]
    fn test_watch_trigger_config_default_delay() {
        let json = r#"{"plugin":"gws","command":"gmail +watch"}"#;
        let cfg: WatchTriggerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.restart_delay_secs, 5);
    }

    #[test]
    fn test_watch_trigger_config_with_event() {
        let json = r#"{"plugin":"gws","event":"email.new","multiplexed":false}"#;
        let cfg: WatchTriggerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.plugin, "gws");
        assert_eq!(cfg.event.as_deref(), Some("email.new"));
        assert!(cfg.command.is_empty());
        assert!(!cfg.multiplexed);
    }

    #[test]
    fn test_watch_trigger_config_backward_compat() {
        // Existing configs without event/multiplexed fields should work
        let json = r#"{"plugin":"gws","command":"gmail +watch","restart_delay_secs":5}"#;
        let cfg: WatchTriggerConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.event.is_none());
        assert!(!cfg.multiplexed);
        assert_eq!(cfg.command, "gmail +watch");
    }
}
