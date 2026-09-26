use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use ai::ToolDefinition;

use crate::gate::{GateVerdict, PermissionGate, ResolvedCall};
use crate::origin::ToolContext;
use crate::process::ProcessRegistry;

// ── Resource Permits ────────────────────────────────────────────────

/// Physical resource kinds that require serialized access.
///
/// Tools that control physical resources (screen, browser) must declare
/// which resource they need via [`DynTool::resource_permit`]. The registry
/// acquires a per-resource mutex before executing, preventing concurrent
/// agents/workflows from fighting over the same physical device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    /// Mouse, keyboard, accessibility, app control, screenshots.
    Screen,
    /// CDP session (Chrome extension automation).
    Browser,
}

/// Per-resource mutexes for serializing physical device access.
///
/// Each `Mutex<()>` acts as a max-1 permit — the guard auto-releases
/// when the tool execution finishes. Upgradeable to `Semaphore` later
/// if we ever need >1 concurrent sessions per resource.
pub struct ResourcePermits {
    screen: tokio::sync::Mutex<()>,
    browser: tokio::sync::Mutex<()>,
}

impl ResourcePermits {
    pub fn new() -> Self {
        Self {
            screen: tokio::sync::Mutex::new(()),
            browser: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn acquire(&self, kind: ResourceKind) -> tokio::sync::MutexGuard<'_, ()> {
        match kind {
            ResourceKind::Screen => self.screen.lock().await,
            ResourceKind::Browser => self.browser.lock().await,
        }
    }
}

/// Result of a tool execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// Upstream HTTP status for tools that make HTTP calls (e.g. web fetch), so a
    /// programmatic caller can branch on 429/403/4xx without string-parsing `content`.
    /// `None` for non-HTTP tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// Terminal error: this failure cannot be recovered by retrying or trying a
    /// different approach (auth expired, account not connected, permission off).
    /// The runner ends the turn and surfaces `content` to the user instead of
    /// feeding it back for the model to improvise around — see FRAMES.md Phase 1.
    /// This is error *classification*, not a failure counter.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub terminal: bool,
    /// Optional structured rendering payload for the app UI, alongside the
    /// model-facing `content` text (which stays the source of truth for the
    /// model). `{"kind": "...", ...}` — the frontend renders known kinds as
    /// rich cards (e.g. `search_results`) and ignores unknown ones. ONE channel
    /// for every producer; never a second text format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    /// On a terminal result: what only the owner can supply before this can
    /// work (a plugin, an account on one), named by the tool that knows it.
    /// A workflow run blocked on it tells the owner from this, never from
    /// `content`'s words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub need: Option<types::OwnerNeed>,
    /// The call did not run: the permission check parked it on the owner.
    /// The ask's id, so a workflow activity can suspend on the same ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parked_ask: Option<String>,
    /// Untrusted content the result carries beyond its tool's own class —
    /// a helper's report of what it read. The run that reads the result has
    /// read that content, and takes its taint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub taint: Vec<types::provenance::ProvenanceClass>,
    /// Deferred tools this result loads, as the model was shown them: from
    /// the next step they are declared like any other tool. `find_tools`
    /// loads what it found; an invalid call to a deferred tool the model was
    /// never sent loads that tool with its error.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loads: Vec<ToolDefinition>,
}

impl ToolResult {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            ..Default::default()
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            ..Default::default()
        }
    }

    /// A terminal (unrecoverable) error: ends the turn and is surfaced to the
    /// user. Use for auth/permission/connection failures the agent cannot fix by
    /// retrying or improvising (FRAMES.md Phase 1).
    pub fn terminal(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            terminal: true,
            ..Default::default()
        }
    }

    /// Attach an upstream HTTP status (builder; chains off `ok`/`error`).
    pub fn with_http_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }

    /// Attach a produced file/artifact (absolute path, `/api/v1/files/<name>` URL, or
    /// `data:` URI). chat_dispatch normalizes + materializes it under `<data_dir>/files/`
    /// and surfaces it to the app as a "Work" artifact.
    pub fn with_image_url(mut self, url: impl Into<String>) -> Self {
        self.image_url = Some(url.into());
        self
    }

    /// Name what only the owner can supply (builder; chains off `terminal`).
    pub fn with_need(mut self, need: types::OwnerNeed) -> Self {
        self.need = Some(need);
        self
    }

    /// Attach a structured rendering payload for the app UI (builder).
    pub fn with_payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = Some(payload);
        self
    }

    /// The untrusted content the result carries (builder).
    pub fn with_taint(mut self, taint: Vec<types::provenance::ProvenanceClass>) -> Self {
        self.taint = taint;
        self
    }
}

/// Tool interface that all tools must implement.
pub trait Tool: Send + Sync {
    /// Tool's unique name.
    fn name(&self) -> &str;

    /// Description for the AI.
    fn description(&self) -> String;

    /// JSON schema for the tool's input.
    fn schema(&self) -> serde_json::Value;

    /// Execute the tool with the given input.
    fn execute(
        &self,
        ctx: &ToolContext,
        input: serde_json::Value,
    ) -> impl std::future::Future<Output = ToolResult> + Send;
}

/// The persistence threshold every tool's `max_result_chars` is capped at:
/// a larger result is saved to the session's `tool-results/` and the model
/// gets a preview (see [`crate::result_shape`]).
pub const PERSIST_THRESHOLD_CHARS: usize = 50_000;

/// A tool's result threshold when it declares none of its own.
pub const DEFAULT_MAX_RESULT_CHARS: usize = 100_000;

/// The tool interface: identity, the model-facing definition, and every
/// attribute other code needs about a tool. Nothing outside a tool keys on
/// its name: the loop, the permission check, provenance, trimming, labels
/// and the chat's image filter read these.
///
/// Rule keys are names of the current tool set. A tool that still carries
/// several jobs behind `action`/`resource` answers per call with the name
/// of the tool that job has in the current set (`read_file`,
/// `run_command`, …), so a rule, an origin limit or a guard written for a
/// name holds whichever tool runs the job.
pub trait DynTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> String;
    fn schema(&self) -> serde_json::Value;
    /// 3–8 words the tool search scores beside the name and description.
    fn search_hint(&self) -> &str {
        ""
    }
    /// Deferred tools are listed by name until `find_tools` loads them; the
    /// core set is always loaded. Default: deferred.
    fn should_defer(&self) -> bool {
        true
    }
    /// The call changes nothing outside this process.
    fn read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }
    /// The call may run alongside other concurrency-safe calls of the same
    /// response. Default: read-only calls are.
    fn concurrency_safe(&self, input: &serde_json::Value) -> bool {
        self.read_only(input)
    }
    /// The key permission rules, origin limits and guards match for this
    /// call: a tool name of the current set, or a catalog operation.
    fn rule_key(&self, _input: &serde_json::Value) -> String {
        self.name().to_string()
    }
    /// The call's value a rule matches beside its key: a command prefix, a
    /// folder, a web domain or a recipient.
    fn rule_field(&self, _input: &serde_json::Value) -> Option<types::permissions::RuleField> {
        None
    }
    /// The one thing the call acts on, for a restricted run's `tool:subject`
    /// allowlist entry. Default: the call's `resource`, on the tools that
    /// still dispatch on one.
    fn subject(&self, input: &serde_json::Value) -> Option<String> {
        input.get("resource").and_then(|v| v.as_str()).map(str::to_string)
    }
    /// The job capability the call belongs to (`file`, `shell`, `web`,
    /// `browser`, `desktop`, `media`, `system`, `contacts`, or an interfaces
    /// catalog term). `None` is basic work.
    fn capability(&self, _input: &serde_json::Value) -> Option<&'static str> {
        None
    }
    /// What the call does outside its own work, as far as its input shows.
    /// An effect the input can't show is `Unknown`, never a guess.
    fn effects(&self, input: &serde_json::Value) -> types::permissions::CallEffects {
        if self.read_only(input) {
            types::permissions::CallEffects::none()
        } else {
            types::permissions::CallEffects::unknown()
        }
    }
    /// Whether the registry validates a call against `schema()` before the
    /// tool runs. Every tool of the new interface does. A pre-interface tool
    /// that settles its own call shapes (an inferred `resource`/`action`, its
    /// alias corrections) answers `false` until its package replaces it; the
    /// invariant test holds every other tool to `true`.
    fn validates_input(&self) -> bool {
        true
    }
    /// Checks the tool makes after schema validation and before permission;
    /// the error is the model-facing message.
    fn validate_input(&self, _input: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    /// A result longer than this (capped at [`PERSIST_THRESHOLD_CHARS`]) is
    /// saved to disk and previewed. `None`: the tool pages its own output
    /// and its results are never persisted.
    fn max_result_chars(&self, _input: &serde_json::Value) -> Option<usize> {
        Some(DEFAULT_MAX_RESULT_CHARS)
    }
    /// The owner-facing line while the call runs ("reading notes.md").
    fn activity(&self, input: &serde_json::Value) -> String {
        crate::humanize::call_labels(self.name(), input).0
    }
    /// The owner-facing line once it ran ("Read notes.md").
    fn outcome(&self, input: &serde_json::Value) -> String {
        crate::humanize::call_labels(self.name(), input).1
    }
    /// The untrusted-content class the call's result brings into the run.
    fn taint(&self, _input: &serde_json::Value) -> Option<types::provenance::ProvenanceClass> {
        None
    }
    /// The call's result can be got again (a file read or change, a
    /// command, a web search or fetch: it can be run again), so it
    /// may be cleared under context pressure
    /// (`agent::harness::compact::trim`).
    fn clearable(&self, _input: &serde_json::Value) -> bool {
        false
    }
    /// An image this call returns is media the owner asked for, attached to
    /// the reply, rather than the tool looking at the screen or a page.
    fn emits_image(&self, _input: &serde_json::Value) -> bool {
        false
    }
    /// The call as it will run, with every shorthand the tool accepts
    /// resolved (an inferred action or resource written in). The registry
    /// applies it before any gate reads the call, so a gate never judges a
    /// different call than the one that executes. Must be idempotent.
    /// Default: the input unchanged.
    fn normalize_input(&self, input: serde_json::Value) -> serde_json::Value {
        input
    }
    /// Declare which physical resource this tool call needs exclusive access to.
    ///
    /// Return `Some(ResourceKind)` to serialize access — the registry will acquire
    /// the corresponding permit before executing. Default: `None` (no serialization).
    fn resource_permit(&self, _input: &serde_json::Value) -> Option<ResourceKind> {
        None
    }
    /// The typed interface operation this call performs
    /// (`capability.resource.action`), when it performs one.
    ///
    /// The permission check asks the TOOL this question instead of matching
    /// on a tool name, so any tool can declare that one of its calls performs
    /// a catalog operation and be decided by the rules keyed on it. The tool
    /// never decides anything itself: it says what the call is.
    ///
    /// Default: `None` — this call performs no typed operation and the gate
    /// does not apply.
    fn operation_performed(&self, _input: &serde_json::Value) -> Option<String> {
        None
    }
    /// For MCP proxy tools: the `(integration_id, original tool name)` this
    /// proxy forwards to — what the runner's approval gate uses to look up the
    /// server's tri-state tool permissions. `None` for every built-in tool.
    fn mcp_proxy_info(&self) -> Option<(String, String)> {
        None
    }
    /// This call's working-time limit, parked time not counted. `None`: the
    /// call runs until it finishes or the turn is stopped; the loop has no
    /// blanket tool budget of its own.
    fn execution_timeout(&self, _input: &serde_json::Value) -> Option<std::time::Duration> {
        None
    }
    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>>;
}

/// Why a call can't run as written.
enum Invalid {
    Unparsed(String),
    Schema { input: serde_json::Value, issues: Vec<String> },
    Tool(String),
}

/// Registry manages available tools.
pub struct Registry {
    // Arc, not Box: execute() clones the handle and drops the map lock BEFORE
    // awaiting the tool (snapshot-then-release). A Box'd map forced holding
    // the read guard across the whole tool future — so a tool parked on an
    // ask card (install/connect) deadlocked any concurrent register/
    // unregister (POST /codes re-registers the plugin tool mid-install).
    tools: Arc<RwLock<HashMap<String, Arc<dyn DynTool>>>>,
    /// Cached tool definitions (description + schema) computed at registration time.
    /// Avoids regenerating descriptions and JSON schemas on every LLM iteration.
    def_cache: Arc<RwLock<HashMap<String, ToolDefinition>>>,
    /// Each tool's compiled input schema, built with its definition.
    validators: Arc<RwLock<HashMap<String, Arc<jsonschema::Validator>>>>,
    /// Deferred tools (`DynTool::should_defer`): listed by name until
    /// `find_tools` loads them.
    deferred: Arc<RwLock<HashSet<String>>>,
    /// Maps agent_id → set of tool names owned by that agent's sidecar.
    agent_tools: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    /// The permission check every call passes before it runs.
    gate: Arc<dyn PermissionGate>,
    process_registry: Arc<ProcessRegistry>,
    bridge: std::sync::RwLock<Option<Arc<mcp::Bridge>>>,
    plugin_store: std::sync::RwLock<Option<Arc<napp::plugin::PluginStore>>>,
    agent_loader: std::sync::RwLock<Option<Arc<napp::AgentLoader>>>,
    /// The file tool's snapshot ledger, kept here so the runner can sweep it
    /// for outside edits without reaching into the tool.
    read_state: std::sync::RwLock<Option<crate::file_tool::ReadState>>,
    /// DB store for MCP proxy tools (OAuth token refresh during tool calls).
    store: std::sync::RwLock<Option<Arc<db::Store>>>,
    /// The plugin runner behind the plugin and operation tools, once a
    /// plugin store is wired.
    plugin_runner: std::sync::RwLock<Option<Arc<crate::plugin_tool::PluginRunner>>>,
    /// The `plugin__<slug>` and operation tools the last refresh registered;
    /// held across a refresh so two never interleave.
    plugin_tools: tokio::sync::Mutex<HashSet<String>>,
    /// Browser manager, for closing a session's tab/page when a sub-agent finishes.
    browser_manager: std::sync::RwLock<Option<Arc<browser::Manager>>>,
    /// Canonical marketplace-code installer (server-implemented). `Arc`-wrapped so the
    /// SAME cell is shared with `PersonaTool` at registration and filled LATE by the
    /// server once `AppState` exists (registration runs before `AppState` is built).
    code_installer: Arc<std::sync::RwLock<Option<Arc<dyn crate::bot_tool::CodeInstaller>>>>,
    /// The permission system's side of making and changing jobs, shared
    /// with `PersonaTool` and filled LATE like `code_installer`.
    job_consent: crate::needs::JobConsentCell,
    /// Broadcast callback (wired to ClientHub by the server), shared with MessageTool
    /// so owner alerts reach the frontend bell + desktop HUD. Filled LATE like
    /// `code_installer` (registration runs before `AppState`/hub exist).
    notify_fn: Arc<std::sync::RwLock<Option<crate::message_tool::NotifyFn>>>,
    /// Coworker message rail (server-implemented dispatch of agent→agent
    /// messages), shared with MessageTool. Filled LATE like `notify_fn`.
    coworker_rail: crate::coworker::CoworkerRailCell,
    /// The workflow manager, filled when the workflow tools register
    /// ([`Registry::register_workflows`]); `stop_task` shares the cell to
    /// stop workflow runs.
    workflows: crate::workflows::WorkflowManagerCell,
    /// The harness's agreed goal, bound LATE once the harness exists; the
    /// `suggest_goal` tool shares the handle.
    goals: crate::goal_tool::GoalHandle,
    resource_permits: ResourcePermits,
    /// This process's lease (`comm::lease`): the gate in `execute`.
    lease: &'static comm::lease::Lease,
}

impl Registry {
    /// A registry whose calls are all decided by `gate`: there is no way to
    /// run a tool without it.
    pub fn new(gate: Arc<dyn PermissionGate>) -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            def_cache: Arc::new(RwLock::new(HashMap::new())),
            validators: Arc::new(RwLock::new(HashMap::new())),
            deferred: Arc::new(RwLock::new(HashSet::new())),
            agent_tools: Arc::new(RwLock::new(HashMap::new())),
            gate,
            process_registry: Arc::new(ProcessRegistry::new()),
            bridge: std::sync::RwLock::new(None),
            plugin_store: std::sync::RwLock::new(None),
            agent_loader: std::sync::RwLock::new(None),
            read_state: std::sync::RwLock::new(None),
            store: std::sync::RwLock::new(None),
            plugin_runner: std::sync::RwLock::new(None),
            plugin_tools: tokio::sync::Mutex::new(HashSet::new()),
            browser_manager: std::sync::RwLock::new(None),
            code_installer: Arc::new(std::sync::RwLock::new(None)),
            job_consent: Arc::new(std::sync::RwLock::new(None)),
            notify_fn: Arc::new(std::sync::RwLock::new(None)),
            coworker_rail: crate::coworker::new_rail_cell(),
            workflows: Default::default(),
            goals: crate::goal_tool::new_handle(),
            resource_permits: ResourcePermits::new(),
            lease: comm::lease::process(),
        }
    }

    /// Close the browser tab/page a session opened — the canonical cleanup for a
    /// finished sub-agent. No-op when no browser manager is wired or the session
    /// never opened anything. Routes to whichever backend served the calls.
    pub async fn close_browser_session(&self, session_id: &str) {
        let mgr = self.browser_manager.read().unwrap().clone();
        if let Some(mgr) = mgr {
            if let Some(exec) = mgr.executor() {
                exec.close_session(session_id).await;
            }
        }
    }

    /// Set the MCP bridge for proxy tool execution.
    pub fn set_bridge(&self, bridge: Arc<mcp::Bridge>) {
        *self.bridge.write().unwrap() = Some(bridge);
    }

    /// Set the DB store (used by MCP proxy tools for OAuth token refresh).
    pub fn set_store(&self, store: Arc<db::Store>) {
        *self.store.write().unwrap() = Some(store);
    }

    /// Set the plugin store for injecting plugin binary env vars into subprocesses.
    pub fn set_plugin_store(&self, ps: Arc<napp::plugin::PluginStore>) {
        *self.plugin_store.write().unwrap() = Some(ps);
    }

    /// Register the workflow tools over `manager`, the one workflow manager;
    /// `stop_task` stops its runs from then on.
    pub async fn register_workflows(&self, manager: Arc<dyn crate::workflows::WorkflowManager>) {
        *self.workflows.write().unwrap() = Some(manager.clone());
        for tool in crate::workflows::tools(manager) {
            self.register(Box::new(tool)).await;
        }
    }

    /// Set the canonical marketplace-code installer. Called LATE by the server (after
    /// `AppState` is built) — `PersonaTool` already shares this cell, so its `install`
    /// action picks up the installer at runtime.
    pub fn set_code_installer(&self, installer: Arc<dyn crate::bot_tool::CodeInstaller>) {
        *self.code_installer.write().unwrap() = Some(installer);
    }

    /// Set the permission system's job consent. Called LATE by the server;
    /// `PersonaTool` shares this cell, so create and update draft and grant
    /// jobs through it from then on.
    pub fn set_job_consent(&self, consent: Arc<dyn crate::needs::JobConsent>) {
        *self.job_consent.write().unwrap() = Some(consent);
    }

    /// Set the broadcast callback (wired to ClientHub). Called LATE by the server
    /// once the hub exists; MessageTool reads it at construction to surface owner
    /// alerts to the frontend (bell + desktop HUD).
    pub fn set_notify_fn(&self, f: crate::message_tool::NotifyFn) {
        *self.notify_fn.write().unwrap() = Some(f);
    }

    /// Set the coworker message rail. Called LATE by the server once `AppState`
    /// exists; MessageTool shares the cell and reads it at execution time.
    pub fn set_coworker_rail(&self, rail: Arc<dyn crate::coworker::CoworkerRail>) {
        *self.coworker_rail.write().unwrap() = Some(rail);
    }

    /// Bind the harness's agreed goal: `suggest_goal` calls go to it. Until
    /// it is bound, the tool says goals can't be set here.
    pub fn bind_goals(&self, goals: Arc<dyn crate::goal_tool::GoalSuggester>) {
        let _ = self.goals.set(goals);
    }

    /// Set the agent loader for PersonaTool filesystem access.
    pub fn set_agent_loader(&self, loader: Arc<napp::AgentLoader>) {
        *self.agent_loader.write().unwrap() = Some(loader);
    }

    /// Cross-file diagnostics the language servers published since the last
    /// call, deduplicated and capped (`diagnostics_feed`), or `None`. Costs
    /// nothing while no server is running.
    pub fn new_diagnostics_note(&self) -> Option<String> {
        crate::diagnostics_feed::take_new(crate::lsp::global().pending_diagnostics())
    }

    /// Files `session_key` has seen that someone else changed since, one
    /// reminder each, each reported once. Empty until the file tools are registered.
    pub fn external_edit_notes(&self, session_key: &str) -> Vec<String> {
        let state = match self.read_state.read() {
            Ok(guard) => guard.clone(),
            Err(_) => None, // poisoned: a panic elsewhere; no notes this pass
        };
        state
            .map(|state| crate::file_tool::external_edit_notes(&state, session_key))
            .unwrap_or_default()
    }

    /// Register a tool: its definition, its compiled schema, and whether
    /// it is deferred, all from the tool's spec.
    pub async fn register(&self, tool: Box<dyn DynTool>) {
        let name = tool.name().to_string();
        let def = ToolDefinition {
            name: name.clone(),
            description: tool.description(),
            input_schema: tool.schema(),
        };
        let validator = crate::input_schema::compile(&name, &def.input_schema).map(Arc::new);
        let deferred = tool.should_defer();
        let mut tools = self.tools.write().await;
        if tools.contains_key(&name) {
            warn!(tool = %name, "tool already registered, overwriting");
        }
        tools.insert(name.clone(), Arc::from(tool));
        drop(tools);
        self.def_cache.write().await.insert(name.clone(), def);
        let mut validators = self.validators.write().await;
        match validator {
            Some(v) => validators.insert(name.clone(), v),
            None => validators.remove(&name),
        };
        drop(validators);
        let mut deferred_set = self.deferred.write().await;
        if deferred {
            deferred_set.insert(name.clone());
        } else {
            deferred_set.remove(&name);
        }
        debug!(tool = %name, deferred, "registered tool");
    }

    /// Register a tool as belonging to an agent's sidecar.
    pub async fn register_for_agent(&self, agent_id: &str, tool: Box<dyn DynTool>) {
        let name = tool.name().to_string();
        self.register(tool).await;
        self.agent_tools
            .write()
            .await
            .entry(agent_id.to_string())
            .or_default()
            .insert(name);
    }

    /// Get the set of tool names owned by an agent's sidecar.
    pub async fn agent_tool_names(&self, agent_id: &str) -> HashSet<String> {
        self.agent_tools
            .read()
            .await
            .get(agent_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Unregister a tool by name.
    pub async fn unregister(&self, name: &str) {
        let mut tools = self.tools.write().await;
        if tools.remove(name).is_some() {
            self.deferred.write().await.remove(name);
            self.def_cache.write().await.remove(name);
            self.validators.write().await.remove(name);
            debug!(tool = %name, "unregistered tool");
        }
    }

    /// Unregister all tools belonging to an agent's sidecar.
    pub async fn unregister_agent_tools(&self, agent_id: &str) {
        let names = {
            let mut at = self.agent_tools.write().await;
            at.remove(agent_id).unwrap_or_default()
        };
        if !names.is_empty() {
            let mut tools = self.tools.write().await;
            let mut cache = self.def_cache.write().await;
            let mut validators = self.validators.write().await;
            let mut deferred = self.deferred.write().await;
            for name in &names {
                tools.remove(name);
                cache.remove(name);
                validators.remove(name);
                deferred.remove(name);
            }
            debug!(agent = %agent_id, tools = ?names, "unregistered agent sidecar tools");
        }
    }

    /// Check if a tool is deferred.
    pub async fn is_deferred(&self, name: &str) -> bool {
        self.deferred.read().await.contains(name)
    }

    /// Get names of all deferred tools.
    pub async fn get_deferred_names(&self) -> HashSet<String> {
        self.deferred.read().await.clone()
    }

    /// Get a tool by name (returns None if not found).
    pub async fn get_tool_names(&self) -> Vec<String> {
        self.tools.read().await.keys().cloned().collect()
    }

    /// List all tools as AI tool definitions.
    /// Every tool definition, in name order. The order reaches the request's
    /// `tools` array, which providers hash as part of the cached prefix: a
    /// HashMap's order is not a contract, and a reordered array is a cache
    /// miss on every turn (coding parity, Stage 3).
    pub async fn list(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<ToolDefinition> = self.def_cache.read().await.values().cloned().collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Get a single tool's definition by name (for callers that offer a curated tool
    /// subset to a sub-agent, e.g. the deep-research harness).
    pub async fn definition(&self, name: &str) -> Option<ToolDefinition> {
        self.def_cache.read().await.get(name).cloned()
    }

    /// List only non-deferred tools as full AI tool definitions.
    /// Deferred tools are excluded — use `list_deferred_stubs()` for compact listings.
    pub async fn list_active(&self, activated: &HashSet<String>) -> Vec<ToolDefinition> {
        let deferred = self.deferred.read().await;
        let cache = self.def_cache.read().await;
        let mut defs: Vec<ToolDefinition> = cache
            .values()
            .filter(|def| !deferred.contains(&def.name) || activated.contains(&def.name))
            .cloned()
            .collect();
        // Name order: see `list`.
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Every deferred tool as `find_tools` searches it, in name order.
    pub async fn deferred_entries(&self) -> Vec<crate::find_tools::DeferredEntry> {
        let deferred = self.deferred.read().await;
        let cache = self.def_cache.read().await;
        let tools = self.tools.read().await;
        let mut entries: Vec<crate::find_tools::DeferredEntry> = deferred
            .iter()
            .filter_map(|name| {
                Some(crate::find_tools::DeferredEntry {
                    definition: cache.get(name)?.clone(),
                    search_hint: tools.get(name)?.search_hint().to_string(),
                })
            })
            .collect();
        entries.sort_by(|a, b| a.definition.name.cmp(&b.definition.name));
        entries
    }

    /// Refresh the cached definition for a tool (e.g. after plugin install/uninstall).
    pub async fn refresh_definition(&self, name: &str) {
        let tools = self.tools.read().await;
        if let Some(tool) = tools.get(name) {
            let def = ToolDefinition {
                name: name.to_string(),
                description: tool.description(),
                input_schema: tool.schema(),
            };
            drop(tools);
            match crate::input_schema::compile(name, &def.input_schema) {
                Some(v) => self.validators.write().await.insert(name.to_string(), Arc::new(v)),
                None => self.validators.write().await.remove(name),
            };
            self.def_cache.write().await.insert(name.to_string(), def);
            debug!(tool = %name, "refreshed cached tool definition");
        }
    }

    /// Re-derive the plugin tools from what is installed and connected: one
    /// `plugin__<slug>` per installed plugin and one operation tool per
    /// catalog operation a connected plugin binds. Called at registration and
    /// whenever a plugin is installed, removed, toggled, connected or
    /// disconnected; a tool no longer backed by a plugin is removed.
    pub async fn refresh_plugin_tools(&self) {
        let Some(runner) = self.plugin_runner.read().unwrap().clone() else {
            return;
        };
        let mut registered = self.plugin_tools.lock().await;
        let mut next: Vec<Box<dyn DynTool>> = runner
            .installed_slugs()
            .iter()
            .map(|slug| Box::new(crate::plugin_tools::PluginCliTool::new(runner.clone(), slug)) as Box<dyn DynTool>)
            .collect();
        let providers: Vec<Arc<dyn crate::operation_tools::OperationProvider>> = runner
            .active_slugs()
            .iter()
            .map(|slug| {
                Arc::new(crate::operation_tools::PluginProvider::new(runner.clone(), slug))
                    as Arc<dyn crate::operation_tools::OperationProvider>
            })
            .collect();
        next.extend(
            crate::operation_tools::operation_tools(&providers)
                .into_iter()
                .map(|t| Box::new(t) as Box<dyn DynTool>),
        );
        // A name another tool already holds stays that tool's.
        let mut names = HashSet::new();
        let mut keep: Vec<Box<dyn DynTool>> = Vec::new();
        for tool in next {
            let name = tool.name().to_string();
            if !registered.contains(&name) && self.get(&name).await.is_some() {
                warn!(tool = %name, "a plugin tool's name is taken by another tool; not registered");
                continue;
            }
            if names.insert(name) {
                keep.push(tool);
            }
        }
        for stale in registered.difference(&names) {
            self.unregister(stale).await;
        }
        for tool in keep {
            self.register(tool).await;
        }
        *registered = names;
    }

    /// The operation tools an employee binds through `requires.interfaces`:
    /// every registered operation tool whose catalog term it names. They are
    /// named in that employee's session context.
    pub async fn operation_tools_for(&self, interfaces: &[String]) -> HashSet<String> {
        if interfaces.is_empty() {
            return HashSet::new();
        }
        let empty = serde_json::json!({});
        self.tools
            .read()
            .await
            .iter()
            .filter(|(_, t)| {
                t.operation_performed(&empty)
                    .is_some_and(|op| interfaces.iter().any(|i| op.split('.').next() == Some(i.as_str())))
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Get the full description of a specific tool (used for steering injection on first use).
    pub async fn get_tool_description(&self, name: &str) -> Option<String> {
        let cache = self.def_cache.read().await;
        cache.get(name).map(|def| def.description.clone())
    }

    /// The `(integration_id, original tool name)` behind a registered MCP proxy
    /// tool (`mcp__<server>__<tool>`), or `None` for any other tool. The runner's
    /// approval gate uses this to resolve the server's tri-state tool permissions.
    pub async fn mcp_proxy_info(&self, name: &str) -> Option<(String, String)> {
        let tools = self.tools.read().await;
        tools.get(name).and_then(|t| t.mcp_proxy_info())
    }

    /// The typed interface operation a call performs, as the tool declares it
    /// (see `DynTool::operation_performed`). `None` for a tool that performs no
    /// typed operation, an unknown tool, or a call that performs none.
    pub async fn operation_performed(
        &self,
        name: &str,
        input: &serde_json::Value,
    ) -> Option<String> {
        let tools = self.tools.read().await;
        tools
            .get(name)
            .and_then(|t| t.operation_performed(input))
            .filter(|op| !op.is_empty())
    }

    /// Per-call execution budget override (see DynTool::execution_timeout).
    /// `None` = the runner's default applies.
    pub async fn execution_timeout(
        &self,
        name: &str,
        input: &serde_json::Value,
    ) -> Option<std::time::Duration> {
        let tools = self.tools.read().await;
        tools.get(name).and_then(|t| t.execution_timeout(input))
    }

    /// A registered tool, for reading its spec.
    pub async fn get(&self, name: &str) -> Option<Arc<dyn DynTool>> {
        self.tools.read().await.get(name).cloned()
    }

    /// Whether this call may run alongside the other concurrency-safe calls
    /// of its response. `false` for an unknown tool.
    /// Whether the call may run alongside other concurrency-safe calls of
    /// its response: the tool says so for the call as it will run, and the
    /// call is valid. An unknown tool or invalid input is not safe.
    pub async fn concurrency_safe(&self, tool_name: &str, input: &serde_json::Value) -> bool {
        let Some(tool) = self.get(tool_name).await else {
            return false;
        };
        match self.settle(tool.as_ref(), tool_name, input.clone()).await {
            Ok(input) => tool.concurrency_safe(&input),
            Err(_) => false,
        }
    }

    /// The call as it will run, or why it can't: arguments that never
    /// parsed, input the schema refuses (a parameter's help text sent back
    /// as its value included), or the tool's own check. Stringified
    /// values are repaired against the schema and the tool settles the call's
    /// shape first, so every check (and the tool) sees the call that runs.
    async fn settle(&self, tool: &dyn DynTool, name: &str, mut input: serde_json::Value) -> Result<serde_json::Value, Invalid> {
        // Arguments that never parsed arrive as `{"_raw": "..."}` (the
        // provider's salvage of a cut or malformed stream).
        if let Some(raw) = unparsed_arguments(&input) {
            return Err(Invalid::Unparsed(raw.to_string()));
        }
        let def = self.definition(name).await;
        if let Some(def) = &def {
            crate::mcp_tool::coerce_schema_types(&mut input, &def.input_schema);
        }
        let input = tool.normalize_input(input);
        let validator = if tool.validates_input() {
            self.validators.read().await.get(name).cloned()
        } else {
            None
        };
        let mut issues = match validator {
            Some(validator) => crate::input_schema::issues(&validator, &input),
            None => Vec::new(),
        };
        // Every tool: a parameter's help text sent back as its value.
        if let Some(def) = &def {
            issues.extend(crate::input_schema::echoed_descriptions(&def.input_schema, &input));
        }
        if !issues.is_empty() {
            return Err(Invalid::Schema { input, issues });
        }
        tool.validate_input(&input).map_err(Invalid::Tool)?;
        Ok(input)
    }

    /// Whether this call changes nothing outside this process. `false` for
    /// an unknown tool.
    pub async fn read_only(&self, tool_name: &str, input: &serde_json::Value) -> bool {
        self.get(tool_name).await.is_some_and(|tool| tool.read_only(input))
    }

    /// The call resolved for the permission check: its rule key, operation,
    /// capability, rule field, read-only flag and effects, from the tool's
    /// spec. `None` for an unknown tool.
    pub async fn target(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> Option<types::permissions::Target> {
        let tool = self.get(tool_name).await?;
        Some(target_of(tool.as_ref(), input))
    }

    /// The owner-facing (activity, outcome) lines for a call. A name no
    /// tool is registered under (a CLI provider's own tools) gets the
    /// generic wording.
    pub async fn labels(&self, tool_name: &str, input: &serde_json::Value) -> (String, String) {
        match self.get(tool_name).await {
            Some(tool) => (tool.activity(input), tool.outcome(input)),
            None => crate::humanize::call_labels(tool_name, input),
        }
    }

    /// The call as the named tool will run it (see
    /// [`DynTool::normalize_input`]). The runner applies this before its
    /// approval, capability and operation gates; [`Registry::execute`]
    /// applies it before its own. Unknown tools pass through unchanged.
    pub async fn normalize_input(&self, tool_name: &str, input: serde_json::Value) -> serde_json::Value {
        let tools = self.tools.read().await;
        match tools.get(tool_name) {
            Some(tool) => tool.normalize_input(input),
            None => input,
        }
    }

    /// Whether this call changes something outside this process — the ONE
    /// answer to that question: the runner's guardrail judges exactly these
    /// calls, and the lease gate in [`Registry::execute`] refuses exactly
    /// these while a cloud bot is frozen. The tool's `read_only`.
    pub async fn has_side_effects(&self, tool_name: &str, input: &serde_json::Value) -> bool {
        !self.read_only(tool_name, input).await
    }

    /// Execute a tool and return the result, shaped for the model.
    ///
    /// The one door every call goes through, in order: resolve the tool,
    /// repair and settle the input, validate it against the schema and the
    /// tool's own checks, the permission check (hard limits, ceiling, rules,
    /// mode), the lease, then run it under its resource permit and shape the
    /// result (the one spill path).
    pub async fn execute(
        &self,
        ctx: &ToolContext,
        tool_name: &str,
        input: serde_json::Value,
    ) -> ToolResult {
        debug!(tool = %tool_name, "executing tool");

        let name = tool_name;

        let Some(tool) = self.get(name).await else {
            warn!(tool = %name, "unknown tool");
            return ToolResult::error(crate::result_shape::unknown_tool(name));
        };

        let input = match self.settle(tool.as_ref(), name, input).await {
            Ok(input) => input,
            Err(invalid) => return self.invalid_call(ctx, name, invalid).await,
        };

        // The permission check: hard limits, the ceiling, the rules and the
        // mode, decided on the call as it will run.
        let call = ResolvedCall {
            tool: tool.as_ref(),
            input: &input,
            target: target_of(tool.as_ref(), &input),
        };
        match self.gate.check(ctx, &call).await {
            GateVerdict::Run(_) => {}
            GateVerdict::Refuse(result) | GateVerdict::Parked(result) => return result,
        }

        // Lease gate: while this bot's lease is not held (a cloud bot
        // that lost its NeboAI connection, or was replaced by another
        // running copy) nothing that changes anything runs — another
        // process may be the bot now. Reads still run.
        if self.lease.frozen() && !tool.read_only(&input) {
            warn!(tool = %name, "lease not held: side-effecting call paused");
            return ToolResult::error(format!("{name}: {}", comm::lease::PAUSED));
        }

        let permit_kind = tool.resource_permit(&input);
        let _permit_guard = match permit_kind {
            Some(kind) => {
                debug!(tool = %name, resource = ?kind, "acquiring resource permit");
                Some(self.resource_permits.acquire(kind).await)
            }
            None => None,
        };

        // The handle is an `Arc` snapshot: no registry lock is held across
        // the tool future (a tool can park for minutes on an ask card while
        // plugin installs re-register tools).
        let threshold = crate::result_shape::threshold(tool.max_result_chars(&input));
        let mut result = tool.execute_dyn(ctx, input.clone()).await;
        if !result.is_error {
            self.gate.ran(ctx, &call, &result).await;
        }
        crate::result_shape::shape(
            name,
            &crate::result_shape::results_dir(&ctx.session_id),
            threshold,
            &mut result,
        );
        result
    }

    /// The error for a call that won't run as written: what is wrong with
    /// it (the smallest valid call when nothing was sent), and for a
    /// deferred tool the model was never sent, its definition. That error
    /// loads the tool, so the retry is the next step, with no `find_tools`
    /// step between.
    async fn invalid_call(&self, ctx: &ToolContext, name: &str, invalid: Invalid) -> ToolResult {
        let definition = self.definition(name).await;
        let mut result = ToolResult::error(match invalid {
            Invalid::Unparsed(raw) => bad_json_error(name, &raw),
            Invalid::Schema { input, mut issues } => {
                if input.as_object().is_some_and(|o| o.is_empty())
                    && let Some(minimal) = definition.as_ref().and_then(|d| crate::input_schema::minimal_call(&d.input_schema))
                {
                    issues.push(minimal);
                }
                crate::result_shape::input_validation(name, &issues)
            }
            Invalid::Tool(message) => crate::result_shape::call_error(&message),
        });
        let unloaded = ctx.declared_tools.as_ref().is_some_and(|declared| !declared.contains(name))
            && self.is_deferred(name).await
            && crate::find_tools::may_load(name, ctx.withheld_tools.as_deref(), crate::desktop_available());
        if unloaded && let Some(definition) = definition {
            result.content.push_str(&format!(
                "\n{name} wasn't loaded, so its definition was never sent. This error loads it: its \
                 definition is below, and your next call can use it.\n{}",
                crate::find_tools::functions_block([&definition])
            ));
            result.loads.push(definition);
        }
        result
    }

    /// Get a reference to the process registry.
    pub fn process_registry(&self) -> &Arc<ProcessRegistry> {
        &self.process_registry
    }

    /// Register the tools that need no database: the file and command
    /// tools and the os tool.
    pub async fn register_defaults(&self) {
        let helpers = crate::command_tools::Helpers {
            orchestrator: crate::orchestrator::new_handle(),
            store: None,
            runs: None,
            workflows: self.workflows.clone(),
        };
        self.register_files_and_commands(helpers).await;
        let mut os_tool = crate::os_tool::OsTool::new();
        let ps_opt = self.plugin_store.read().unwrap().clone();
        if let Some(ps) = ps_opt {
            os_tool = os_tool.with_plugin_store(ps);
        }
        self.register(Box::new(os_tool)).await;
    }

    /// The file and command tools, on one [`crate::file_tools::Machine`]
    /// so they share the read ledger the runner sweeps for outside edits.
    async fn register_files_and_commands(&self, helpers: crate::command_tools::Helpers) {
        use crate::command_tools::*;
        use crate::file_tools::*;
        let plugins = self.plugin_store.read().unwrap().clone();
        let machine = Arc::new(Machine::new(self.process_registry.clone(), plugins));
        // Startup: a poisoned lock here is a bug to surface, not a state to handle.
        *self.read_state.write().unwrap() = Some(machine.file.read_state());
        let tools: Vec<Box<dyn DynTool>> = vec![
            Box::new(ReadFileTool(machine.clone())),
            Box::new(EditFileTool(machine.clone())),
            Box::new(WriteFileTool(machine.clone())),
            Box::new(ShareFileTool(machine.clone())),
            Box::new(ConvertFileTool),
            Box::new(CheckpointFilesTool(machine.clone())),
            Box::new(ListCheckpointsTool(machine.clone())),
            Box::new(RestoreCheckpointTool(machine.clone())),
            Box::new(WritePlanTool(machine.clone())),
            Box::new(CheckPlanTool::new(machine.clone(), self.gate.clone())),
            Box::new(RunCommandTool(machine.clone())),
            Box::new(ReadOutputTool { machine: machine.clone(), helpers: helpers.clone() }),
            Box::new(StopTaskTool { machine: machine.clone(), helpers }),
            Box::new(ListProcessesTool(machine.clone())),
            Box::new(SendInputTool(machine)),
        ];
        for tool in tools {
            self.register(tool).await;
        }
    }

    /// Register all domain tools including those that need DB access.
    pub async fn register_all(
        &self,
        store: Arc<db::Store>,
        orchestrator: crate::OrchestratorHandle,
    ) {
        self.register_all_with_browser(store, None, orchestrator, None, None, None)
            .await;
    }

    /// Register all domain tools with optional browser manager.
    pub async fn register_all_with_browser(
        &self,
        store: Arc<db::Store>,
        browser_manager: Option<Arc<browser::Manager>>,
        orchestrator: crate::OrchestratorHandle,
        skill_loader: Option<Arc<crate::skills::Loader>>,
        advisor_runner: Option<Arc<dyn crate::bot_tool::AdvisorDeliberator>>,
        hybrid_searcher: Option<Arc<dyn crate::bot_tool::HybridSearcher>>,
    ) {
        self.register_all_with_permissions(
            store,
            browser_manager,
            orchestrator,
            skill_loader,
            advisor_runner,
            hybrid_searcher,
            None, // memory_embedder
            None, // structured_agent
            None, // workflow_manager
            None, // permissions
            None, // plan_tier
            None, // sandbox_manager
            None, // comm_plugin
            None, // active_agent
            None, // broadcaster
            None, // run_querier
        )
        .await;
    }

    /// Register domain tools filtered by capability permissions.
    /// When `permissions` is None, all tools are registered (no filtering).
    /// When `permissions` is Some, only categories with `true` values are registered.
    pub async fn register_all_with_permissions(
        &self,
        store: Arc<db::Store>,
        browser_manager: Option<Arc<browser::Manager>>,
        orchestrator: crate::OrchestratorHandle,
        skill_loader: Option<Arc<crate::skills::Loader>>,
        advisor_runner: Option<Arc<dyn crate::bot_tool::AdvisorDeliberator>>,
        hybrid_searcher: Option<Arc<dyn crate::bot_tool::HybridSearcher>>,
        memory_embedder: Option<Arc<dyn crate::bot_tool::MemoryEmbedder>>,
        structured_agent: Option<Arc<dyn crate::bot_tool::StructuredAgent>>,
        workflow_manager: Option<Arc<dyn crate::workflows::WorkflowManager>>,
        permissions: Option<&HashMap<String, bool>>,
        plan_tier: Option<Arc<tokio::sync::RwLock<String>>>,
        sandbox_manager: Option<Arc<sandbox_runtime::SandboxManager>>,
        comm_plugin: Option<Arc<dyn comm::CommPlugin>>,
        active_agent: Option<crate::agent_tool::ActiveAgentState>,
        broadcaster: Option<crate::web_tool::Broadcaster>,
        run_querier: Option<crate::run_querier::RunQuerierHandle>,
    ) {
        let allowed = |category: &str| -> bool {
            match permissions {
                None => true, // No permissions map = allow all
                Some(map) => *map.get(category).unwrap_or(&false),
            }
        };

        // Keep a handle to the browser manager so finished sub-agents can close
        // their tab/page via `close_browser_session` (the web tool takes ownership below).
        *self.browser_manager.write().unwrap() = browser_manager.clone();

        // Files and commands: read_file, edit_file, write_file and
        // run_command are core; the rest of the family is deferred.
        self.register_files_and_commands(crate::command_tools::Helpers {
            orchestrator: orchestrator.clone(),
            store: Some(store.clone()),
            runs: run_querier.clone(),
            workflows: self.workflows.clone(),
        })
        .await;

        // OS tool (desktop, apps, settings, music, keychain, search, PIM).
        let mut os_tool = crate::os_tool::OsTool::new().with_store(store.clone());
        let ps_opt = self.plugin_store.read().unwrap().clone();
        if let Some(ps) = ps_opt {
            os_tool = os_tool.with_plugin_store(ps);
        }
        self.register(Box::new(os_tool)).await;

        // Code tool (tree-sitter outline/symbols/parse_check/query/context) — deferred.
        // Read-only intel, no capability gate: same tier as os file read/grep
        // (it declares no capability, so it is ungated).
        self.register(Box::new(crate::code_tool::CodeTool::new()))
            .await;

        // The web and browser tools (search, fetch, HTTP, the browser) —
        // requires "web" permission. They share one core.
        if allowed("web") {
            let mut web = crate::web_tool::WebCore::new().with_store(store.clone());
            // Platform search via Janus: resolve the gateway URL (honoring the
            // NEBOAI_JANUS_URL env override) and the bot identity so search_web
            // hits a real search API instead of scraping engines through a browser.
            if let Ok(cfg) = config::Config::load_embedded() {
                let bot_id = config::read_bot_id().unwrap_or_default();
                web = web.with_janus_search(cfg.neboai.janus_url.clone(), bot_id);
            }
            if let Some(mgr) = browser_manager {
                web = web.with_browser(mgr);
            }
            if let Some(ref bc) = broadcaster {
                web = web.with_broadcaster(bc.clone());
            }
            for tool in crate::web_tool::tools(web) {
                self.register(Box::new(tool)).await;
            }
        }

        // The packs this company works by (R8): create, add, list, show, remove.
        self.register(Box::new(crate::pack_tool::PackTool)).await;

        // The seat's own context section (R15): the write half of the layers.
        self.register(Box::new(crate::rules_tool::RulesTool::new(store.clone(), active_agent.clone()))).await;

        // Memory (core), helpers (delegate core), tasks and runs, past
        // conversations, the advisor panel, research, the profile, asking
        // and reaching the owner, and the agreed goal.
        let run_querier = run_querier.unwrap_or_else(crate::run_querier::new_handle);
        // The teams on this Nebo: a local object that works with no hub; the
        // comm handle, when present, only adds the optional hub mirror.
        // send_message posts into teams through the same core.
        let teams = Arc::new(crate::team_tool::Teams::new(
            Some(store.clone()),
            comm_plugin.clone(),
            broadcaster.clone(),
            self.coworker_rail.clone(),
        ));
        let families = [
            crate::memory_tools::Memory::new(store.clone(), hybrid_searcher, memory_embedder).tools(),
            crate::helper_tools::Helpers::new(store.clone(), orchestrator.clone(), teams.clone(), self.coworker_rail.clone()).tools(),
            crate::task_tools::Tasks::new(store.clone(), run_querier).tools(),
            crate::history_tools::History::new(store.clone()).tools(),
            crate::advisor_tools::Advisors::new(store.clone(), advisor_runner).tools(),
            crate::research_tools::Research::new(structured_agent, orchestrator.clone()).tools(),
            crate::profile_tools::Profile::new(store.clone(), self.notify_fn.clone()).tools(),
            crate::owner_tools::Owner::new(store.clone(), self.notify_fn.clone()).tools(),
            vec![
                Box::new(crate::ask_owner_tool::AskOwnerTool::new(store.clone(), self.coworker_rail.clone())) as Box<dyn DynTool>,
                Box::new(crate::goal_tool::SuggestGoalTool::new(self.goals.clone())),
                Box::new(crate::file_tools::ExitPlanModeTool::new(store.clone())),
            ],
        ];
        for tool in families.into_iter().flatten() {
            self.register(tool).await;
        }

        // The employee tools (deferred): the roster, hiring, and making,
        // changing and removing employees.
        {
            let agent_reg = active_agent.unwrap_or_else(|| {
                std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()))
            });
            let agent_loader = self
                .agent_loader
                .read()
                .unwrap()
                .clone()
                .unwrap_or_else(|| {
                    let data = config::data_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                    Arc::new(napp::AgentLoader::new(
                        data.join("nebo").join("agents"),
                        data.join("user").join("agents"),
                    ))
                });
            let persona =
                crate::agent_tool::PersonaTool::new(store.clone(), agent_reg, agent_loader)
                    .with_code_installer(self.code_installer.clone())
                    .with_job_consent(self.job_consent.clone());
            for tool in crate::employee_tools::tools(persona) {
                self.register(Box::new(tool)).await;
            }
        }

        // The schedule tools (reminders and recurring jobs).
        for tool in crate::event_tool::tools(store.clone()) {
            self.register(Box::new(tool)).await;
        }

        // The skill tools: use_skill (core) and the deferred family, sharing
        // one core over the skill loader.
        let loader = skill_loader.clone().unwrap_or_else(|| {
            let data = config::data_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            Arc::new(crate::skills::Loader::new(data.join("nebo").join("skills"), data.join("user").join("skills")))
        });
        let mut skills = crate::skill_tool::SkillCore::new(loader)
            .with_notify_fn(self.notify_fn.clone())
            .with_code_installer(self.code_installer.clone());
        if skill_loader.is_some() {
            skills = skills.with_store(store.clone());
        }
        // Wire the plugin registry so a skill search can say when the query
        // named a plugin.
        let ps_opt = self.plugin_store.read().unwrap().clone();
        if let Some(ps) = ps_opt {
            skills = skills.with_plugin_store(ps);
        }
        for tool in crate::skill_tool::tools(skills) {
            self.register(Box::new(tool)).await;
        }

        // Execute tool (script execution) — deferred (only activated when user mentions scripts/code)
        if let (Some(loader), Some(tier)) = (&skill_loader, &plan_tier) {
            let mut execute_tool = crate::execute_tool::ExecuteTool::new(
                loader.clone(),
                tier.clone(),
                sandbox_manager.clone(),
            )
            .with_store(store.clone());
            if let Some(ps) = self.plugin_store.read().unwrap().clone() {
                execute_tool = execute_tool.with_plugin_store(ps);
            }
            self.register(Box::new(execute_tool)).await;
        }

        // Message tool (coworker messages + SMS) — always registered (core)
        self.register(Box::new(crate::message_tool::MessageTool::new(store.clone())))
        .await;

        // The workflow tools (lifecycle and runs).
        if let Some(manager) = workflow_manager {
            self.register_workflows(manager).await;
        }

        self.register(Box::new(crate::publisher_tool::PublisherTool::new(
            store.clone(),
        )))
        .await;

        // Notebook tool (.ipynb cell editing) — deferred (activated when the user
        // mentions notebooks / Jupyter / .ipynb).
        self.register(Box::new(crate::notebook_tool::NotebookTool::new()))
            .await;

        // The workflow `exit` primitive — deferred like any tool, so every
        // run declares the same tools: a workflow activity lists and loads
        // it, and the harness refuses it to any other run.
        self.register(Box::new(crate::exit_tool::ExitTool::new())).await;

        // Plugins: the marketplace search and the events reader whenever a
        // plugin store exists (zero plugins installed included), then one
        // tool per installed plugin and per operation a connected one binds.
        let ps_opt = self.plugin_store.read().unwrap().clone();
        if let Some(ps) = ps_opt {
            let mut runner = crate::plugin_tool::PluginRunner::new(ps, store.clone());
            if let Some(ref bc) = broadcaster {
                runner = runner.with_broadcaster(bc.clone());
            }
            let runner = Arc::new(runner);
            *self.plugin_runner.write().unwrap() = Some(runner.clone());
            self.register(Box::new(crate::plugin_tools::FindPluginsTool::new(runner.clone()))).await;
            self.register(Box::new(crate::plugin_tools::ReadPluginEventsTool::new(runner))).await;
            self.refresh_plugin_tools().await;
        }

        // VM tool (isolated Linux environment for builds/toolchains) — deferred
        // Activated when agent needs Go, gcc, Docker, or clean build env
        self.register(Box::new(crate::vm_tool::VmTool::new()))
            .await;

        // The team tools (teams of local employees), on the core above.
        for tool in crate::team_tool::tools(teams) {
            self.register(Box::new(tool)).await;
        }

        // Authority tool (standing authority inside the constitution). Deferred:
        // it reaches the model when a turn loads it with find_tools (a seat's
        // `requires.tools` names "authority" for the General Manager).
        self.register(Box::new(crate::authority_tool::AuthorityTool::new(store.clone())))
            .await;

        // The NeboAI loop tools (hub messages, channels, loops, topics) —
        // require the "loop" permission. The comm handle exists from startup;
        // each call's `is_connected()` check reflects the live connection
        // state, so they are registered whenever the handle is available
        // (even before NeboAI connects).
        if allowed("loop")
            && let Some(ref comm) = comm_plugin
        {
            let core = crate::loop_tool::LoopCore::new(comm.clone(), Some(store.clone()));
            for tool in crate::loop_tool::tools(core) {
                self.register(Box::new(tool)).await;
            }
        }
    }
}

impl mcp::bridge::ProxyToolRegistry for Registry {
    fn register_proxy(&self, name: &str, def: &mcp::McpToolDef, integration_id: &str) {
        let bridge = match self.bridge.read().unwrap().as_ref() {
            Some(b) => b.clone(),
            None => {
                warn!(tool = %name, "cannot register MCP proxy: bridge not set");
                return;
            }
        };
        let store = match self.store.read().unwrap().as_ref() {
            Some(s) => s.clone(),
            None => {
                warn!(tool = %name, "cannot register MCP proxy: store not set");
                return;
            }
        };
        let tool = crate::mcp_tool::McpProxyTool::new(name, def, integration_id, bridge, store);
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(self.register(Box::new(tool)));
            });
        }
    }

    fn unregister_proxy(&self, name: &str) {
        if tokio::runtime::Handle::try_current().is_ok() {
            let name = name.to_string();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(self.unregister(&name));
            });
        }
    }

    fn tools_synced(&self, integration_id: &str, server_slug: &str, tools: &[(String, String)]) {
        use types::permissions::{Effect, Rule, RuleKey, RuleSource, Scope, Writer};
        let store = match self.store.read().unwrap().as_ref() {
            Some(s) => s.clone(),
            None => {
                warn!(integration = %integration_id, "cannot settle MCP tool rules: store not set");
                return;
            }
        };
        // Company Memory (the platform-authenticated server) is governed on
        // the KB page, which the shard enforces; it takes no rule here.
        let platform = store
            .get_mcp_integration(integration_id)
            .ok()
            .flatten()
            .is_some_and(|i| i.auth_type == "neboai");
        if platform {
            return;
        }
        let rule = |key: RuleKey, effect: Effect| Rule {
            id: uuid::Uuid::new_v4().to_string(),
            scope: Scope::Company,
            key,
            field: None,
            effect,
            money: None,
            source: RuleSource::Owner,
            locked: false,
            created_at: chrono::Utc::now().timestamp(),
        };
        let company = match store.permission_rules_in(&Scope::Company) {
            Ok(rules) => rules,
            Err(e) => {
                warn!(integration = %integration_id, error = %e, "cannot settle MCP tool rules: rules unreadable");
                return;
            }
        };
        let own = |key: &RuleKey| company.iter().find(|r| &r.key == key && r.field.is_none());
        // A server's tools ask until the owner says otherwise: the server's
        // default rule is written once, at its first connect.
        let default_key = RuleKey::Tool(format!("mcp__{server_slug}__*"));
        let default = match own(&default_key) {
            Some(r) => r.effect,
            None => {
                if let Err(e) =
                    store.write_permission_rule(&rule(default_key, Effect::Ask), &Writer::Migration)
                {
                    warn!(integration = %integration_id, error = %e, "failed to write the MCP server's default rule");
                }
                Effect::Ask
            }
        };
        // A tool the owner has never seen doesn't ride an "Always allow"
        // default: while the default allows, each tool the server didn't
        // offer at its last sync gets its own ask, which the owner can
        // change. A tool the server stopped offering loses its rule, so if
        // it returns it is new again.
        let known = store
            .get_mcp_known_tools(integration_id)
            .unwrap_or_default();
        for (original, proxy) in tools {
            let key = RuleKey::Tool(proxy.clone());
            if default == Effect::Allow && !known.contains(original) && own(&key).is_none() {
                match store.write_permission_rule(&rule(key, Effect::Ask), &Writer::Migration) {
                    Ok(_) => {
                        info!(integration = %integration_id, tool = %proxy, "a new tool on an always-allowed server asks first")
                    }
                    Err(e) => {
                        warn!(integration = %integration_id, tool = %proxy, error = %e, "the new tool's ask did not land")
                    }
                }
            }
        }
        let family = format!("mcp__{server_slug}__");
        for r in company.iter().filter(|r| r.field.is_none() && !r.locked) {
            let RuleKey::Tool(key) = &r.key else { continue };
            if key.starts_with(&family)
                && !key.ends_with('*')
                && !tools.iter().any(|(_, proxy)| proxy == key)
            {
                if let Err(e) = store.remove_permission_rule(&r.id, &Writer::Migration) {
                    warn!(integration = %integration_id, tool = %key, error = %e, "a gone tool's rule was not removed");
                }
            }
        }
        let current: Vec<String> = tools.iter().map(|(original, _)| original.clone()).collect();
        if let Err(e) = store.set_mcp_known_tools(integration_id, &current) {
            warn!(integration = %integration_id, error = %e, "the server's tool list was not recorded");
        }
    }
}

// Tool→capability mapping and labels live in `crate::capabilities` — the single
// source of truth shared with the Settings → Permissions UI (served via the API).
// The old `tool_category`/`capability_label` here used a drifted vocabulary
// (filesystem/memory/plugin) that didn't match the persisted keys
// (file/shell/system/…), so most toggles silently gated nothing.


/// A tool's answers for one call, resolved for the permission check.
pub fn target_of(tool: &dyn DynTool, input: &serde_json::Value) -> types::permissions::Target {
    types::permissions::Target {
        tool: tool.name().to_string(),
        key: tool.rule_key(input),
        operation: tool.operation_performed(input).filter(|op| !op.is_empty()),
        capability: tool.capability(input).map(str::to_string),
        field: tool.rule_field(input),
        subject: tool.subject(input),
        read_only: tool.read_only(input),
        effects: tool.effects(input),
    }
}

/// The raw text of arguments that never parsed: the provider hands them over
/// as `{"_raw": "..."}`.
fn unparsed_arguments(input: &serde_json::Value) -> Option<&str> {
    let obj = input.as_object()?;
    if obj.len() != 1 {
        return None;
    }
    obj.get("_raw")?.as_str()
}

/// The input error for arguments that are not JSON, quoting their
/// first bytes. Past 4 KB they were cut off at the output limit, and the
/// same call will be cut off again.
fn bad_json_error(tool: &str, raw: &str) -> String {
    let issue = if raw.len() < 4096 {
        format!(
            "The arguments are not valid JSON ({} bytes): `{}`. A value is missing or \
             malformed. Resend the same call with every field filled; leave a field out \
             rather than empty.",
            raw.len(),
            crate::truncate_str(raw, 200)
        )
    } else {
        format!(
            "The arguments were cut off at the output limit ({} bytes arrived; the JSON is \
             incomplete), starting `{}`. Do not resend the same call: it will be cut off \
             again. Write large content in parts of under ~15,000 characters each.",
            raw.len(),
            crate::truncate_str(raw, 200)
        )
    };
    crate::result_shape::input_validation(tool, &[issue])
}

/// Tool names follow `^[a-z][a-z0-9_]*$`; external families keep their
/// `plugin__`, `mcp__` or `app__` namespace.
#[cfg(test)]
pub(crate) fn is_tool_name(name: &str) -> bool {
    let plain = |n: &str| {
        let mut chars = n.chars();
        chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    match ["plugin__", "mcp__", "app__"].iter().find_map(|p| name.strip_prefix(p)) {
        Some(rest) => !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        None => plain(name),
    }
}


#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A model that serializes a structured argument as a JSON string must not
    /// reach the tool that way: `agent(action:"spawn_parallel", tasks:"[{…}]")`
    /// read as a missing `tasks` param, and the model re-sent the identical call
    /// until the spiral backstop killed the turn.
    struct ArrayParamTool;

    impl DynTool for ArrayParamTool {
        fn name(&self) -> &str {
            "arrayparam"
        }
        fn description(&self) -> String {
            String::new()
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": { "tasks": { "type": "array" }, "note": { "type": "string" } }
            })
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a ToolContext,
            input: serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
            Box::pin(async move {
                match input.get("tasks").and_then(|t| t.as_array()) {
                    Some(a) => ToolResult::ok(format!("array:{}", a.len())),
                    None => ToolResult::error("missing tasks"),
                }
            })
        }
    }

    #[tokio::test]
    async fn test_stringified_array_arg_is_coerced_before_dispatch() {
        let registry = Registry::new(crate::gate::test_gate());
        registry.register(Box::new(ArrayParamTool)).await;

        let ctx = ToolContext::default();
        let stringified = serde_json::json!({
            "tasks": "[{\"prompt\": \"a\"}, {\"prompt\": \"b\"}]",
            "note": "[not, parsed]"
        });

        let result = registry.execute(&ctx, "arrayparam", stringified).await;
        assert!(!result.is_error, "stringified array should reach the tool as an array: {}", result.content);
        assert_eq!(result.content, "array:2");
    }

    /// Side effects are each tool's own `read_only` answer: web reads look
    /// and run together; clicks, page changes and non-GET requests act and
    /// run alone; a status poll of a workflow is a read that still never
    /// runs alongside others; an emitted event acts only through its
    /// subscribers' own runs.
    #[tokio::test]
    async fn side_effects_are_each_tools_read_only_answer() {
        use serde_json::json;
        let web = crate::web_tool::tools(crate::web_tool::WebCore::new());
        let tool = |name: &str| web.iter().find(|t| t.name() == name).unwrap();
        for (name, read) in [
            ("browser_read", json!({})),
            ("search_web", json!({"queries": ["x"]})),
            ("fetch_url", json!({"url": "https://example.com"})),
            ("http_request", json!({"method": "GET", "url": "https://example.com"})),
        ] {
            assert!(tool(name).read_only(&read), "{name} {read}");
            assert!(tool(name).concurrency_safe(&read), "{name} {read}");
        }
        for (name, act) in [
            ("browser_act", json!({"action": "click", "ref": "e1"})),
            ("browser_open", json!({"url": "https://example.com"})),
            ("browser_fill_form", json!({"fields": [{"ref": "e1", "value": "x"}]})),
            ("http_request", json!({"method": "POST", "url": "https://example.com"})),
            ("http_request", json!({"method": "DELETE", "url": "https://example.com"})),
        ] {
            assert!(!tool(name).read_only(&act), "{name} {act}");
            assert!(!tool(name).concurrency_safe(&act), "a web write runs alone: {name} {act}");
        }
        let (bus, _rx) = crate::events::EventBus::new();
        let emit = crate::emit_tool::EmitTool::new(bus);
        assert!(emit.read_only(&json!({"source": "inventory.low"})));
        assert!(!emit.concurrency_safe(&json!({"source": "inventory.low"})));
        let (registry, _dir) = os_registry().await;
        assert!(!registry.has_side_effects("read_file", &json!({"path": "/tmp/a"})).await);
        assert!(registry.has_side_effects("write_file", &json!({"path": "/tmp/a", "content": "x"})).await);
        assert!(registry.has_side_effects("unknown", &json!({})).await, "unknown means acting");
    }

    /// A tool whose `read` action is read-only and whose other actions write;
    /// counts how many calls actually ran.
    struct LedgerTool(Arc<std::sync::atomic::AtomicUsize>);

    impl DynTool for LedgerTool {
        fn name(&self) -> &str {
            "ledger"
        }
        fn description(&self) -> String {
            String::new()
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn read_only(&self, input: &serde_json::Value) -> bool {
            input["action"] == "read"
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a ToolContext,
            _input: serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
            Box::pin(async move {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ToolResult::ok("done")
            })
        }
    }

    /// While a cloud bot's lease is not held, a call that changes something
    /// is not run and says so truthfully (never a success); reads still run;
    /// once the lease is held again the same call runs.
    #[tokio::test]
    async fn a_frozen_bot_runs_reads_and_pauses_everything_else() {
        let lease: &'static comm::lease::Lease = Box::leak(Box::new(comm::lease::Lease::new()));
        lease.set_fenced(true);
        lease.claim();
        let mut registry = Registry::new(crate::gate::test_gate());
        registry.lease = lease;
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        registry.register(Box::new(LedgerTool(ran.clone()))).await;
        let ctx = ToolContext::default();

        let write = registry.execute(&ctx, "ledger", serde_json::json!({"action": "post"})).await;
        assert!(write.is_error, "a paused call must not report success: {}", write.content);
        assert!(write.content.contains("Paused") && write.content.contains("nothing was sent or changed"), "{}", write.content);
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 0, "the write ran while frozen");

        let read = registry.execute(&ctx, "ledger", serde_json::json!({"action": "read"})).await;
        assert!(!read.is_error, "{}", read.content);
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 1);

        lease.granted(2, std::time::Duration::from_secs(60), std::time::Instant::now());
        let write = registry.execute(&ctx, "ledger", serde_json::json!({"action": "post"})).await;
        assert!(!write.is_error, "{}", write.content);
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    /// A registry holding the real file, command and os tools, and a
    /// scratch directory.
    async fn os_registry() -> (Registry, tempfile::TempDir) {
        let registry = Registry::new(crate::gate::test_gate());
        registry.register_defaults().await;
        (registry, tempfile::tempdir().unwrap())
    }

    /// The database directory: any command naming it is refused by the
    /// safeguard alone (the shell has no check of its own for it), and
    /// `test -d` on it changes nothing if a gate is missed.
    fn db_dir() -> String {
        config::data_dir().unwrap().join("data").to_string_lossy().into_owned()
    }

    /// The runner's gates read the call through the same door: the settled
    /// call is gated on its own key and capability, and settling it twice
    /// changes nothing.
    #[tokio::test]
    async fn the_runner_gates_read_the_settled_call() {
        let (registry, _dir) = os_registry().await;
        let target = registry
            .target("run_command", &serde_json::json!({ "command": "ls", "description": "List files" }))
            .await
            .unwrap();
        assert_eq!((target.key.as_str(), target.capability.as_deref()), ("run_command", Some("shell")));
        let settled = registry
            .normalize_input("read_file", serde_json::json!({ "file_path": "/tmp/a" }))
            .await;
        assert_eq!(settled, serde_json::json!({ "path": "/tmp/a" }));
        assert_eq!(registry.normalize_input("read_file", settled.clone()).await, settled);
        let target = registry.target("read_file", &settled).await.unwrap();
        assert_eq!((target.key.as_str(), target.capability.as_deref()), ("read_file", Some("file")));
    }

    /// An `mcp__<server>__<tool>` name is an MCP proxy or nothing: it never
    /// runs a built-in under a name no gate recognises.
    #[tokio::test]
    async fn an_mcp_prefixed_name_never_runs_a_built_in() {
        let (registry, dir) = os_registry().await;
        let marker = dir.path().join("ran");
        let result = registry
            .execute(
                &ToolContext::default(),
                "mcp__anything__run_command",
                serde_json::json!({
                    "command": format!("touch {}; test -d '{}'", marker.display(), db_dir()),
                    "description": "Touch a marker",
                }),
            )
            .await;
        assert!(result.is_error, "{}", result.content);
        assert!(!marker.exists(), "a built-in ran under an MCP name");
        assert!(
            !registry.concurrency_safe("mcp__anything__read_file", &serde_json::json!({"path": "/tmp/x"})).await,
            "an MCP name answered with a built-in's concurrency"
        );
    }

    /// The request's tool order is a cache key. Two registries built in
    /// different orders must emit identical definition lists.
    #[tokio::test]
    async fn tool_definitions_are_in_name_order() {
        let a = Registry::new(crate::gate::test_gate());
        let b = Registry::new(crate::gate::test_gate());
        let mk = |n: &str| ToolDefinition {
            name: n.into(),
            description: format!("{n} first line\nmore"),
            input_schema: serde_json::json!({"type": "object"}),
        };
        for n in ["web", "os", "agent", "skill"] {
            a.def_cache.write().await.insert(n.into(), mk(n));
        }
        for n in ["skill", "agent", "os", "web"] {
            b.def_cache.write().await.insert(n.into(), mk(n));
        }
        for r in [&a, &b] {
            r.deferred.write().await.insert("web".into());
            r.deferred.write().await.insert("skill".into());
        }
        let names = |defs: Vec<ToolDefinition>| defs.into_iter().map(|d| d.name).collect::<Vec<_>>();
        assert_eq!(names(a.list().await), vec!["agent", "os", "skill", "web"]);
        assert_eq!(names(a.list().await), names(b.list().await));
        let none = std::collections::HashSet::new();
        assert_eq!(names(a.list_active(&none).await), vec!["agent", "os"]);

    }

    /// Drift guard for the tool-rename class (TD-001, capability vocabulary,
    /// the bot() steering sweep): source strings that teach the model a
    /// deprecated tool name send it into the correction path, wasting a turn.
    /// Scans this crate and nebo-agent for deprecated call shapes.
    #[test]
    fn no_deprecated_tool_calls_in_model_facing_strings() {
        // Patterns assembled with concat! so this test file never matches itself.
        const DEPRECATED: &[&str] = &[
            concat!("bot", "(resource"),
            concat!("bot", "(action"),
            concat!("system", "(resource"),
            concat!("system", "(action"),
            concat!("desktop", "(resource"),
            concat!("desktop", "(action"),
        ];
        fn scan(dir: &std::path::Path, hits: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    scan(&path, hits);
                } else if matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("rs" | "md" | "txt")
                ) {
                    let content = std::fs::read_to_string(&path).unwrap_or_default();
                    for pat in DEPRECATED {
                        if content.contains(pat) {
                            hits.push(format!("{}: contains `{}`", path.display(), pat));
                        }
                    }
                }
            }
        }
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut hits = Vec::new();
        scan(&manifest.join("src"), &mut hits);
        scan(&manifest.join("../agent/src"), &mut hits);
        assert!(
            hits.is_empty(),
            "deprecated tool names in model-facing strings (use agent/os):\n{}",
            hits.join("\n")
        );
    }
    /// The string literals of Rust source, outside comments, as written
    /// (escapes kept), each with its line number.
    fn string_literals(src: &str) -> Vec<(usize, String)> {
        let b = src.as_bytes();
        let line_at = |i: usize| src[..i].matches('\n').count() + 1;
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            let ident_before = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
            if b[i..].starts_with(b"//") {
                i = src[i..].find('\n').map_or(b.len(), |n| i + n);
            } else if b[i..].starts_with(b"/*") {
                i = src[i..].find("*/").map_or(b.len(), |n| i + n + 2);
            } else if b[i] == b'r' && !ident_before && matches!(b.get(i + 1), Some(b'"' | b'#')) {
                let hashes = b[i + 1..].iter().take_while(|&&c| c == b'#').count();
                if b.get(i + 1 + hashes) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let start = i + 2 + hashes;
                let close = format!("\"{}", "#".repeat(hashes));
                let end = src[start..].find(&close).map_or(b.len(), |n| start + n);
                out.push((line_at(i), src[start..end].to_string()));
                i = end + close.len();
            } else if b[i] == b'"' {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                out.push((line_at(i), src[i + 1..j.min(b.len())].to_string()));
                i = j + 1;
            } else if b[i] == b'\'' {
                // A char literal ('x', '\n', '"'), or a lifetime.
                i = match (b.get(i + 1), b.get(i + 2)) {
                    (Some(b'\\'), _) => src[i + 2..].find('\'').map_or(b.len(), |n| i + 2 + n + 1),
                    (_, Some(b'\'')) => i + 3,
                    _ => i + 1,
                };
            } else {
                i += 1;
            }
        }
        out
    }

    /// The call shapes `name(key: …)`, `name("…")` and `name()` in a string.
    fn call_shapes(text: &str) -> Vec<String> {
        let b = text.as_bytes();
        let mut out = Vec::new();
        for (open, _) in text.match_indices('(') {
            let start = text[..open]
                .char_indices()
                .rev()
                .find(|&(_, c)| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
                .map_or(0, |(at, c)| at + c.len_utf8());
            let name = &text[start..open];
            let after = &text[open + 1..];
            let named_arg = after
                .split_once(':')
                .is_some_and(|(key, _)| !key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
            let joined = start > 0 && matches!(b[start - 1], b'.' | b'$' | b':' | b'_' | b'A'..=b'Z');
            if !name.is_empty()
                && name.as_bytes()[0].is_ascii_lowercase()
                && !joined
                && (named_arg || after.starts_with('"') || after.starts_with("\\\"") || after.starts_with(')'))
            {
                out.push(name.to_string());
            }
        }
        out
    }

    /// Error, correction and instruction text names only tools that exist.
    /// Proof runs of 2026-09-26: the over-budget read error said "or grep
    /// for what you need", there is no grep tool, and a run called one — a
    /// second error against a limit of one. Every string the tools and the
    /// harness can put in front of the model is checked:
    /// - each call shape `name(…)` names a tool the product registers;
    /// - the tool names other harnesses have and Nebo doesn't (grep, glob)
    ///   are never offered as a tool ("grep for", "use grep", "with grep"):
    ///   they appear only as a command, `run_command(command: "grep -n …")`.
    #[tokio::test]
    async fn model_facing_text_names_only_tools_that_exist() {
        let (registry, dir) = full_registry().await;
        // What the server registers beside `register_all`: the loop tools
        // with its hub connection, script execution with its skill loader,
        // the workflow tools with their manager, emit_event with the event
        // bus, and a2ui with its A2UI host (a server type).
        for tool in crate::loop_tool::tools(crate::loop_tool::LoopCore::new(Arc::new(comm::LoopbackPlugin::new()), None)) {
            registry.register(Box::new(tool)).await;
        }
        let loader = Arc::new(crate::skills::Loader::new(dir.path().join("skills"), dir.path().join("user_skills")));
        registry
            .register(Box::new(crate::execute_tool::ExecuteTool::new(loader, Arc::new(RwLock::new(String::new())), None)))
            .await;
        registry
            .register_workflows(Arc::new(crate::workflows::work_tool::tests::Recorder::default()))
            .await;
        registry.register(Box::new(crate::emit_tool::EmitTool::new(crate::events::EventBus::new().0))).await;
        let mut tools: HashSet<String> = registry.get_tool_names().await.into_iter().collect();
        tools.insert("a2ui".to_string());

        // Notations that read like calls and are not: a workflow trigger,
        // `call(line: …)`.
        const NOT_CALLS: &[&str] = &["call"];
        const FOREIGN: &[&str] = &["grep", "glob"];
        for name in FOREIGN {
            assert!(!tools.contains(*name), "{name} is a tool now: take it off the foreign list");
        }
        let offered = |text: &str, word: &str| {
            ["for", "tool"].iter().any(|next| text.contains(&format!("{word} {next}")))
                || ["use", "with", "call", "the", "using"].iter().any(|before| {
                    text.match_indices(&format!("{before} {word}")).any(|(at, m)| {
                        let rest = &text[at + m.len()..];
                        !(rest.starts_with(" -") || rest.starts_with(" '") || rest.starts_with(" \\\""))
                            && !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
                    })
                })
        };

        fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // The test harness and its grader talk to a judge, not an employee.
                    if path.file_name().is_some_and(|n| n != "testing") {
                        sources(&path, out);
                    }
                } else if path.extension().is_some_and(|e| e == "rs") && path.file_name().is_some_and(|n| n != "tests.rs") {
                    out.push(path);
                }
            }
        }
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        sources(&manifest.join("src"), &mut files);
        sources(&manifest.join("../agent/src"), &mut files);
        assert!(files.len() > 100, "the scan reads the sources: {}", files.len());
        let mut hits = Vec::new();
        for path in files {
            let src = std::fs::read_to_string(&path).unwrap();
            let tests_at = src.match_indices("#[cfg(test)]\n").map(|(at, m)| at + m.len()).find(|&next| {
                let line = src[next..].trim_start();
                ["mod ", "pub mod ", "pub(crate) mod "].iter().any(|p| line.starts_with(p))
            });
            let src = tests_at.map_or(src.as_str(), |at| &src[..at]);
            for (line, text) in string_literals(src) {
                for name in call_shapes(&text) {
                    if !tools.contains(&name) && !NOT_CALLS.contains(&name.as_str()) {
                        hits.push(format!("{}:{line}: {name}(…) names no tool", path.display()));
                    }
                }
                for word in FOREIGN {
                    if offered(&text, word) {
                        hits.push(format!("{}:{line}: offers {word} as a tool: {text:?}", path.display()));
                    }
                }
            }
        }
        assert!(hits.is_empty(), "model-facing text names tools that don't exist:\n{}", hits.join("\n"));
    }

    /// The scan finds what it is for, and not code or commands.
    #[test]
    fn the_scan_reads_literals_and_call_shapes() {
        let src = "let a = \"use grep for it\"; // \"not(this: 1)\"\nlet b = r#\"x(\"y\")\"#; let c = '\"'; fn f<'a>() {}\nlet d = \"run_command(command: \\\"grep -n x\\\")\";";
        let lits: Vec<String> = string_literals(src).into_iter().map(|(_, t)| t).collect();
        assert_eq!(lits, ["use grep for it", "x(\"y\")", "run_command(command: \\\"grep -n x\\\")"]);
        assert_eq!(call_shapes("Use capture(action: see), then os(resource: \"x\") or now() and $(date) or x.y(\"z\")."), ["capture", "os", "now"]);
        assert!(call_shapes("the file(s) you (probably) want").is_empty());
    }

    /// A small deferred tool with a strict schema, for the error shapes.
    struct EchoTool;

    impl DynTool for EchoTool {
        fn name(&self) -> &str {
            "echo_text"
        }
        fn description(&self) -> String {
            "Echoes text.".into()
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The words to echo back." },
                    "times": { "type": "integer" }
                },
                "required": ["text"]
            })
        }
        fn search_hint(&self) -> &str {
            "repeat text back"
        }
        fn read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn validate_input(&self, input: &serde_json::Value) -> Result<(), String> {
            if input["text"] == "" {
                return Err("`text` is empty: give the words to echo.".into());
            }
            Ok(())
        }
        fn max_result_chars(&self, _input: &serde_json::Value) -> Option<usize> {
            Some(1_000)
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a ToolContext,
            input: serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
            Box::pin(async move {
                let times = input["times"].as_u64().unwrap_or(1) as usize;
                ToolResult::ok(input["text"].as_str().unwrap_or("").repeat(times))
            })
        }
    }

    async fn echo_registry() -> Registry {
        let registry = Registry::new(crate::gate::test_gate());
        registry.register(Box::new(EchoTool)).await;
        registry
    }

    #[tokio::test]
    async fn an_unknown_tool_is_a_call_error() {
        let r = echo_registry().await;
        let out = r.execute(&ToolContext::default(), "no_such_tool", serde_json::json!({})).await;
        assert!(out.is_error);
        assert_eq!(out.content, "<call_error>There is no tool named no_such_tool.</call_error>");
    }

    #[tokio::test]
    async fn a_schema_failure_names_each_issue_and_an_empty_call_gets_the_minimal_shape() {
        let r = echo_registry().await;
        let ctx = ToolContext::default();
        let out = r.execute(&ctx, "echo_text", serde_json::json!({"text": "hi", "times": "many"})).await;
        assert!(out.is_error);
        assert!(out.content.starts_with("<call_error>Invalid input for echo_text:\n"), "{}", out.content);
        assert!(out.content.contains("The parameter `times` type is expected as `integer` but provided as `string`"), "{}", out.content);
        let out = r.execute(&ctx, "echo_text", serde_json::json!({})).await;
        assert!(out.content.contains("The required parameter `text` is missing"), "{}", out.content);
        assert!(out.content.contains("A minimal valid call: {\"text\": <string>}"), "{}", out.content);
    }

    /// Invalid input is never concurrency-safe: a call that won't run as
    /// written doesn't join a parallel batch.
    #[tokio::test]
    async fn invalid_input_is_not_concurrency_safe() {
        struct Reader;
        impl DynTool for Reader {
            fn name(&self) -> &str {
                "reader"
            }
            fn description(&self) -> String {
                String::new()
            }
            fn schema(&self) -> serde_json::Value {
                serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]})
            }
            fn read_only(&self, _input: &serde_json::Value) -> bool {
                true
            }
            fn execute_dyn<'a>(
                &'a self,
                _ctx: &'a ToolContext,
                _input: serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
                Box::pin(async { ToolResult::ok("read") })
            }
        }
        let registry = Registry::new(crate::gate::test_gate());
        registry.register(Box::new(Reader)).await;
        assert!(registry.concurrency_safe("reader", &serde_json::json!({"path": "/a"})).await);
        assert!(!registry.concurrency_safe("reader", &serde_json::json!({})).await, "missing a required field");
        assert!(!registry.concurrency_safe("reader", &serde_json::json!({"_raw": "{\"pa"})).await, "never parsed");
        assert!(!registry.concurrency_safe("nope", &serde_json::json!({})).await, "unknown tool");
    }

    /// A parameter's help text sent back as its value is refused at the
    /// door, naming the parameter; the tool never runs.
    #[tokio::test]
    async fn a_parameters_help_text_as_its_value_is_refused() {
        let r = echo_registry().await;
        let ctx = ToolContext::default();
        for echo in ["The words to echo back.", "the words to echo back", "  THE WORDS, to echo back!  "] {
            let out = r.execute(&ctx, "echo_text", serde_json::json!({ "text": echo })).await;
            assert!(out.is_error, "{echo}: {}", out.content);
            assert!(out.content.starts_with("<call_error>Invalid input for echo_text:\n"), "{}", out.content);
            assert!(out.content.contains("The parameter `text` is its own help text"), "{}", out.content);
            assert!(out.content.contains("Pass the owner's actual words for `text`"), "{}", out.content);
        }
        let ok = r.execute(&ctx, "echo_text", serde_json::json!({ "text": "The words to echo back, please." })).await;
        assert_eq!(ok.content, "The words to echo back, please.");
    }

    #[tokio::test]
    async fn validate_input_runs_after_the_schema_and_before_the_tool() {
        let r = echo_registry().await;
        let out = r.execute(&ToolContext::default(), "echo_text", serde_json::json!({"text": ""})).await;
        assert_eq!(out.content, "<call_error>`text` is empty: give the words to echo.</call_error>");
    }

    #[tokio::test]
    async fn arguments_that_are_not_json_quote_their_first_bytes() {
        let r = echo_registry().await;
        let out = r
            .execute(&ToolContext::default(), "echo_text", serde_json::json!({"_raw": "{\"text\": }"}))
            .await;
        assert!(out.content.starts_with("<call_error>Invalid input for echo_text"), "{}", out.content);
        assert!(out.content.contains("`{\"text\": }`"), "{}", out.content);
    }

    /// A deferred tool the model was never sent: a call that validates
    /// runs; one that won't run as written (bad JSON, a schema miss, the
    /// tool's own check) gets the tool's definition with its error, and that
    /// error loads it. A tool the run may not load is not loaded, and a
    /// loaded tool's errors carry no definition.
    #[tokio::test]
    async fn an_invalid_call_to_an_unloaded_deferred_tool_loads_it() {
        let r = echo_registry().await;
        let ctx = ToolContext {
            declared_tools: Some(Arc::new(HashSet::from(["read_file".to_string()]))),
            ..Default::default()
        };
        let ok = r.execute(&ctx, "echo_text", serde_json::json!({"text": "hi"})).await;
        assert_eq!(ok.content, "hi", "a call that validates simply runs");
        assert!(ok.loads.is_empty());
        let echo = r.definition("echo_text").await.unwrap();
        for input in [serde_json::json!({}), serde_json::json!({"_raw": "{\"te"}), serde_json::json!({"text": ""})] {
            let bad = r.execute(&ctx, "echo_text", input.clone()).await;
            assert!(bad.is_error, "{input}");
            assert_eq!(bad.loads.len(), 1, "{input}: {}", bad.content);
            assert_eq!(serde_json::to_string(&bad.loads[0]).unwrap(), serde_json::to_string(&echo).unwrap());
            assert!(bad.content.contains("This error loads it"), "{}", bad.content);
            assert!(bad.content.ends_with(&crate::find_tools::functions_block([&echo])), "the definition it loads is the one shown: {}", bad.content);
            assert!(!bad.content.contains("find_tools"), "no second step: {}", bad.content);
        }
        // A tool this run may not load stays unloaded.
        let withheld = ToolContext {
            withheld_tools: Some(Arc::new(HashSet::from(["echo_text".to_string()]))),
            ..ctx.clone()
        };
        let bad = r.execute(&withheld, "echo_text", serde_json::json!({})).await;
        assert!(bad.loads.is_empty() && !bad.content.contains("<functions>"), "{}", bad.content);
        // Loaded (declared) tools don't get the definition again.
        let loaded = ToolContext {
            declared_tools: Some(Arc::new(HashSet::from(["echo_text".to_string()]))),
            ..Default::default()
        };
        let bad = r.execute(&loaded, "echo_text", serde_json::json!({})).await;
        assert!(bad.loads.is_empty() && !bad.content.contains("<functions>"), "{}", bad.content);
    }

    /// Results over the tool's threshold go to the one spill path, the
    /// session's `tool-results/`, and come back as a preview.
    #[tokio::test]
    async fn a_result_over_the_threshold_is_persisted_under_the_session() {
        let r = echo_registry().await;
        let ctx = ToolContext { session_id: format!("wp0-spill-{}", uuid::Uuid::new_v4()), ..Default::default() };
        let out = r.execute(&ctx, "echo_text", serde_json::json!({"text": "abcdefghij\n", "times": 500})).await;
        assert!(out.content.starts_with(crate::result_shape::SAVED_OUTPUT), "{}", out.content);
        let path = out.content.split("Saved in full at: ").nth(1).and_then(|l| l.lines().next()).unwrap();
        let dir = crate::result_shape::results_dir(&ctx.session_id);
        assert!(std::path::Path::new(path).starts_with(&dir), "{path} not under {}", dir.display());
        assert_eq!(std::fs::read_to_string(path).unwrap().len(), 5_500);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        let short = r.execute(&ctx, "echo_text", serde_json::json!({"text": "hi"})).await;
        assert_eq!(short.content, "hi");
    }

    /// Deferral comes from the spec; the search sees name, description and
    /// hint for every deferred tool, in name order.
    #[tokio::test]
    async fn deferral_is_the_tools_own_answer() {
        let r = echo_registry().await;
        r.register(Box::new(crate::find_tools::FindToolsTool::new(Arc::new(Registry::new(crate::gate::test_gate())))))
            .await;
        assert!(r.is_deferred("echo_text").await);
        assert!(!r.is_deferred("find_tools").await);
        let entries = r.deferred_entries().await;
        assert_eq!(entries.len(), 1);
        assert_eq!((entries[0].definition.name.as_str(), entries[0].search_hint.as_str()), ("echo_text", "repeat text back"));
        r.unregister("echo_text").await;
        assert!(r.deferred_entries().await.is_empty());
    }

    /// Every tool the full roster registers, the way a bot builds it (the
    /// server adds find_tools after `register_all`), with one installed
    /// plugin that binds an operation, so its tool and the operation's are
    /// in the roster too.
    pub(crate) async fn full_registry() -> (Arc<Registry>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap());
        let version_dir = dir.path().join("plugins").join("ledgerly").join("0.1.0");
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(
            version_dir.join("plugin.json"),
            serde_json::json!({
                "id": "ledgerly", "slug": "ledgerly", "name": "Ledgerly", "version": "0.1.0", "platforms": {},
                "description": "Bookkeeping for small businesses.",
                "interfaceBindings": {"ledger.invoice.send": "invoice send {invoiceId} {sendTo?:--send-to}"},
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(version_dir.join("ledgerly"), b"#!/bin/sh\necho ok\n").unwrap();
        std::fs::create_dir_all(dir.path().join("user_plugins")).unwrap();
        let registry = Arc::new(Registry::new(crate::gate::test_gate()));
        registry.set_plugin_store(Arc::new(napp::plugin::PluginStore::new(
            dir.path().join("plugins"),
            dir.path().join("user_plugins"),
            None,
        )));
        registry.register_all(store, crate::orchestrator::new_handle()).await;
        registry.register(Box::new(crate::find_tools::FindToolsTool::new(registry.clone()))).await;
        (registry, dir)
    }

    /// The tools that still carry several jobs behind `action`/`resource`.
    /// Each tool package removes its names; nothing is ever added.
    const PRE_INTERFACE_TOOLS: &[&str] = &[
        "a2ui", "authority", "code", "execute", "exit", "message", "notebook", "os", "pack",
        "publisher", "rules", "vm",
    ];

    /// The enum-dispatch surfaces the interface allows (device surfaces).
    const ENUM_SURFACES: &[(&str, &str)] = &[("code_intel", "operation"), ("browser_act", "action")];

    /// The invariants every tool of the new interface meets (tools doc
    /// §7.2): a search hint, a lean description, a snake_case (or
    /// namespaced) name, no `action`/`resource` dispatch, at most three
    /// required parameters. Tools not yet moved onto the interface are the
    /// closed `PRE_INTERFACE_TOOLS` list.
    #[tokio::test]
    async fn every_new_interface_tool_has_a_lean_flat_spec() {
        let (registry, _dir) = full_registry().await;
        for name in registry.get_tool_names().await {
            let tool = registry.get(&name).await.unwrap();
            let def = registry.definition(&name).await.unwrap();
            if PRE_INTERFACE_TOOLS.contains(&name.as_str()) {
                continue;
            }
            assert!(is_tool_name(&name), "{name}: names are ^[a-z][a-z0-9_]*$ or plugin__/mcp__/app__");
            assert!(!tool.search_hint().trim().is_empty(), "{name}: no search_hint");
            let hint_words = tool.search_hint().split_whitespace().count();
            assert!((3..=8).contains(&hint_words), "{name}: search_hint is 3–8 words");
            assert!(!def.description.trim().is_empty() && def.description.chars().count() <= 1_600, "{name}: lean description ≤ 1,600 chars");
            assert!(tool.validates_input(), "{name}: every new-interface tool is validated against its schema");
            assert_spec_is_flat(&name, &def.input_schema);
        }
    }

    fn assert_spec_is_flat(name: &str, schema: &serde_json::Value) {
        assert_eq!(schema["type"], "object", "{name}: the schema is an object");
        let props = schema["properties"].as_object().cloned().unwrap_or_default();
        for dispatch in ["action", "resource"] {
            let allowed = ENUM_SURFACES.iter().any(|(t, p)| *t == name && *p == dispatch);
            assert!(!props.contains_key(dispatch) || allowed, "{name}: `{dispatch}` dispatch is not allowed");
        }
        let required = schema["required"].as_array().map_or(0, |r| r.len());
        assert!(required <= 3, "{name}: {required} required parameters (at most 3)");
    }

    /// The deferred listing shows each tool with what it is for (its
    /// search hint), so the owner's own words find it: every deferred tool
    /// has one.
    #[tokio::test]
    async fn every_deferred_tool_says_what_it_is_for() {
        let (registry, _dir) = full_registry().await;
        let entries = registry.deferred_entries().await;
        assert!(entries.len() > 50, "the roster is the real one: {}", entries.len());
        let silent: Vec<&str> = entries
            .iter()
            .filter(|e| e.search_hint.trim().is_empty())
            .map(|e| e.definition.name.as_str())
            .collect();
        assert!(silent.is_empty(), "deferred tools listed without a purpose: {silent:?}");
    }

    /// The checks themselves: a tool with `action`, or four required
    /// parameters, fails them.
    #[test]
    #[should_panic(expected = "`action` dispatch is not allowed")]
    fn an_action_parameter_fails_the_flat_spec_check() {
        assert_spec_is_flat("send_thing", &serde_json::json!({"type": "object", "properties": {"action": {"type": "string"}}}));
    }

    #[test]
    #[should_panic(expected = "4 required parameters")]
    fn four_required_parameters_fail_the_flat_spec_check() {
        assert_spec_is_flat("send_thing", &serde_json::json!({"type": "object", "properties": {}, "required": ["a", "b", "c", "d"]}));
    }

    #[test]
    fn the_pre_interface_list_is_closed_and_the_allowed_surfaces_are_the_device_ones() {
        assert_eq!(PRE_INTERFACE_TOOLS.len(), 12, "packages only remove names from this list");
        assert!(ENUM_SURFACES.iter().all(|(t, _)| is_tool_name(t)));
    }

    /// Characters of every always-loaded definition (description + schema).
    /// The definitions are the same on every bot, so the number is one for
    /// every platform (batch C moved the os tool's server note into the
    /// environment row). Measured at WP0: 52,728 on macOS (agent
    /// 17,451 · os 14,219 · web 8,361 · message 3,270 · skill 3,102 · team
    /// 2,747 · event 2,229 · find_tools 704 · mcp 645) and 53,032 on Linux
    /// (os 14,524 · web 8,362 · mcp 643). WP5 deferred the web family
    /// (−8,361); WP9 the schedule and team families (−4,976). WP1 moved
    /// files and commands off os: 37,495 on macOS (os 9,304 · run_command
    /// 1,087 · read_file 782 · edit_file 705 · write_file 448) and 37,798
    /// on Linux (os 9,609). WP4 swapped skill (3,102) for use_skill (585):
    /// −2,517. Tools WP2 moved helpers, memory and asking off agent and
    /// message (agent 6,005 · message 2,657 · delegate 1,702 · remember
    /// 1,039 · recall 701 · ask_owner 618 · forget 336). Tools WP3 deferred
    /// the employee family and deleted agent: −6,005. WP9 moved coworker
    /// messages off message to send_message (message 1,450: −1,207). WP6
    /// deleted the mcp tool and deferred the plugin family (−657). Batch C
    /// gave delegate its `speed` (fix plan E8, +124: delegate 1,826) and took
    /// the server note off os on server bots. Fix plan D16 gave delegate
    /// when-to-use guidance and worked examples (+630: delegate
    /// 2,456) and D19 deferred os on every bot (−9,134): 10,908. The owner,
    /// 2026-09-26, after an employee loaded 28 skills one per step: it
    /// should have fanned out and acted in parallel. delegate got a skill-survey example (schema restatements
    /// moved out: delegate 2,495) and use_skill its loading rule (674):
    /// 11,036. The owner, 2026-09-26: employees drive interactive terminal
    /// programs; run_command's `pty` (90) is the one line that needs:
    /// 11,126. The proof runs of 2026-09-26 (270 runs) loaded deferred tools
    /// mid-conversation in 84 of them, and every such load rewrites the
    /// cached prompt at more than the fresh input price (#392: 45, 47 and 87
    /// cache rewrites in the three runs). The tools loaded in the most runs
    /// (get_employee 17, search_web 15, find_plugins 13, fetch_url 11,
    /// list_employees 9, send_message 8) moved into the core, with
    /// create_schedule, which "Remind me in 3 hours" never found while
    /// deferred (3/3 → 0/3); find_tools gave back 28: 16,210. Larger or
    /// one-task tools stayed deferred (create_workflow 3,254 and
    /// update_employee 6,616 chars; convert_file, hire_employee and
    /// install_workflow loads came from the fixtures that ask for them).
    /// Each package that lands lowers the numbers; they never rise without
    /// an owner decision.
    const CORE_DEFINITION_CHARS_BUDGET: usize = 16_210;

    #[tokio::test]
    async fn the_always_loaded_set_stays_within_its_budget() {
        let (registry, _dir) = full_registry().await;
        let deferred = registry.get_deferred_names().await;
        let mut core: Vec<(String, usize)> = Vec::new();
        for def in registry.list().await {
            if deferred.contains(&def.name) {
                continue;
            }
            core.push((def.name.clone(), def.description.chars().count() + def.input_schema.to_string().chars().count()));
        }
        let total: usize = core.iter().map(|(_, n)| n).sum();
        eprintln!("core definitions: {total} chars {core:?}");
        assert!(total <= CORE_DEFINITION_CHARS_BUDGET, "always-loaded definitions are {total} chars, over the {CORE_DEFINITION_CHARS_BUDGET} budget: {core:?}");
    }

    /// Deferred: everything but the core. Code, loop, work, emit, pack,
    /// rules, a2ui, publisher, notebook, vm and authority are listed and
    /// loadable, never dropped. The pre-interface tools stay core until
    /// their package replaces them.
    #[tokio::test]
    async fn the_core_is_the_strap_tools_and_find_tools() {
        let (registry, _dir) = full_registry().await;
        let deferred = registry.get_deferred_names().await;
        let mut core: Vec<String> = registry.get_tool_names().await.into_iter().filter(|n| !deferred.contains(n)).collect();
        core.sort();
        assert_eq!(
            core,
            [
                "ask_owner", "create_schedule", "delegate", "edit_file", "fetch_url", "find_plugins",
                "find_tools", "forget", "get_employee", "list_employees", "message", "read_file",
                "recall", "remember", "run_command", "search_web", "send_message", "use_skill",
                "write_file"
            ]
        );
        // D19: the desktop tool is deferred on every bot; the listing names
        // it where a desktop exists. suggest_goal is deferred: it is needed
        // rarely, and only in the owner's own conversation.
        for name in ["os", "suggest_goal"] {
            assert!(deferred.contains(name), "{name} is deferred");
        }
        for name in ["read_output", "stop_task", "list_processes", "send_input", "share_file", "convert_file", "checkpoint_files", "list_checkpoints", "restore_checkpoint", "write_plan", "check_plan"] {
            assert!(deferred.contains(name), "{name} is deferred");
        }
        for name in ["code", "notebook", "vm", "publisher", "authority", "pack", "rules", "list_schedules", "list_teams"] {
            assert!(deferred.contains(name), "{name} is deferred");
        }
        // The plugin family: the events reader, one tool per installed
        // plugin and per operation it binds, deferred; the marketplace
        // search is core.
        for name in ["read_plugin_events", "plugin__ledgerly", "ledger_invoice_send"] {
            assert!(deferred.contains(name), "{name} is registered and deferred");
        }
    }

    /// Every rule key a tool answers is a tool name of the current set, or
    /// the catalog operation an operation tool performs.
    #[tokio::test]
    async fn rule_keys_are_tool_names() {
        let (registry, _dir) = full_registry().await;
        let calls = [
            ("run_command", serde_json::json!({"command": "ls", "description": "List files"})),
            ("read_file", serde_json::json!({"path": "/tmp/x"})),
            ("read_output", serde_json::json!({"task_id": "bg-1a2b3c4d"})),
            ("stop_task", serde_json::json!({"task_id": "sa-1"})),
            ("find_employees", serde_json::json!({"query": "bookkeeper"})),
            ("update_employee", serde_json::json!({"name": "x", "description": "d"})),
            ("remember", serde_json::json!({"key": "k", "value": "v"})),
            ("delegate", serde_json::json!({"description": "d", "prompt": "p"})),
            ("use_skill", serde_json::json!({"name": "x"})),
            ("save_skill", serde_json::json!({"name": "x", "content": "y"})),
            ("fetch_url", serde_json::json!({"url": "https://example.com"})),
            ("browser_act", serde_json::json!({"action": "click", "ref": "e1"})),
            ("message", serde_json::json!({"resource": "sms", "action": "send"})),
            ("message_owner", serde_json::json!({"message": "m"})),
            ("find_tools", serde_json::json!({"query": "x"})),
            ("find_plugins", serde_json::json!({"query": "x"})),
            ("read_plugin_events", serde_json::json!({"plugin": "ledgerly"})),
            ("plugin__ledgerly", serde_json::json!({"command": "doctor"})),
            ("ledger_invoice_send", serde_json::json!({"invoiceId": "1041"})),
        ];
        for (tool, input) in calls {
            let t = registry.target(tool, &input).await.unwrap();
            let named = is_tool_name(&t.key) && !PRE_INTERFACE_TOOLS.contains(&t.key.as_str());
            assert!(named || t.operation.as_deref() == Some(t.key.as_str()), "{tool} {input} → {}", t.key);
        }
    }


    /// Which results context pressure clears, and the taint a result brings
    /// in, are each tool's own answers: file reads and changes, commands,
    /// web searches and fetches are cleared (each can be run again); memory,
    /// skills, plans, restore points, background
    /// output, mail and calendar never are; web content and mail are
    /// untrusted.
    #[tokio::test]
    async fn trimming_and_taint_are_each_tools_answer() {
        use serde_json::json;
        use types::provenance::ProvenanceClass;
        let (registry, _dir) = full_registry().await;
        let cleared = |name: &'static str, input: serde_json::Value| {
            let registry = registry.clone();
            async move { registry.get(name).await.unwrap().clearable(&input) }
        };
        assert!(cleared("read_file", json!({"path": "/tmp/x"})).await);
        assert!(cleared("write_file", json!({"path": "/tmp/x", "content": "x"})).await);
        assert!(cleared("edit_file", json!({"path": "/tmp/x", "old_string": "a", "new_string": "b"})).await);
        assert!(cleared("run_command", json!({"command": "ls", "description": "List files"})).await);
        assert!(cleared("search_web", json!({"queries": ["x"]})).await);
        assert!(cleared("fetch_url", json!({"url": "https://example.com"})).await);
        assert!(!cleared("browser_act", json!({"action": "click", "ref": "e1"})).await);
        assert!(!cleared("os", json!({"resource": "calendar", "action": "today"})).await);
        assert!(!cleared("recall", json!({"query": "x"})).await);
        assert!(!cleared("use_skill", json!({"name": "x"})).await);
        assert!(!cleared("find_skills", json!({"query": "x"})).await);
        assert!(!cleared("write_plan", json!({"content": "x"})).await);
        assert!(!cleared("list_checkpoints", json!({})).await);
        assert!(!cleared("read_output", json!({"task_id": "cmd-1"})).await);
        let taint = |name: &'static str, input: serde_json::Value| {
            let registry = registry.clone();
            async move { registry.get(name).await.unwrap().taint(&input) }
        };
        assert_eq!(taint("fetch_url", json!({"url": "https://example.com"})).await, Some(ProvenanceClass::Web));
        assert_eq!(taint("browser_read", json!({})).await, Some(ProvenanceClass::Web));
        assert_eq!(taint("os", json!({"resource": "mail", "action": "unread"})).await, Some(ProvenanceClass::ExternalEmail));
        assert_eq!(taint("os", json!({"resource": "mail", "action": "send", "to": "a@example.com"})).await, None);
        assert_eq!(taint("message", json!({"resource": "sms", "action": "read"})).await, Some(ProvenanceClass::Channel));
        assert_eq!(taint("read_file", json!({"path": "/tmp/x"})).await, None, "the owner's own files carry no taint");
    }



}
