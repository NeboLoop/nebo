use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use ai::ToolDefinition;

use crate::origin::ToolContext;
use crate::policy::Policy;
use crate::process::ProcessRegistry;
use crate::safeguard;

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
}

/// Tool interface that all tools must implement.
pub trait Tool: Send + Sync {
    /// Tool's unique name.
    fn name(&self) -> &str;

    /// Description for the AI.
    fn description(&self) -> String;

    /// JSON schema for the tool's input.
    fn schema(&self) -> serde_json::Value;

    /// Whether this tool needs user approval.
    fn requires_approval(&self) -> bool;

    /// Per-resource approval check. Override for tools with mixed approval per resource.
    fn requires_approval_for(&self, _input: &serde_json::Value) -> bool {
        self.requires_approval()
    }

    /// Execute the tool with the given input.
    fn execute(
        &self,
        ctx: &ToolContext,
        input: serde_json::Value,
    ) -> impl std::future::Future<Output = ToolResult> + Send;
}

/// Type-erased tool wrapper for dynamic dispatch.
pub trait DynTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> String;
    fn schema(&self) -> serde_json::Value;
    fn requires_approval(&self) -> bool;
    /// Per-resource approval check. Override for tools with mixed approval per resource.
    fn requires_approval_for(&self, _input: &serde_json::Value) -> bool {
        self.requires_approval()
    }
    /// Declare which physical resource this tool call needs exclusive access to.
    ///
    /// Return `Some(ResourceKind)` to serialize access — the registry will acquire
    /// the corresponding permit before executing. Default: `None` (no serialization).
    fn resource_permit(&self, _input: &serde_json::Value) -> Option<ResourceKind> {
        None
    }
    /// Whether this tool call is safe to run concurrently with other tools.
    ///
    /// Read-only operations (file read, web search, skill catalog) return `true`
    /// and can run in parallel. Write operations (file write, shell exec, plugin exec)
    /// return `false` and are executed serially after all concurrent tools finish.
    ///
    /// Default: `false` (assume writes). Override for read-only operations.
    fn is_concurrent_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }
    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>>;
}

/// Registry manages available tools.
pub struct Registry {
    tools: Arc<RwLock<HashMap<String, Box<dyn DynTool>>>>,
    /// Cached tool definitions (description + schema) computed at registration time.
    /// Avoids regenerating descriptions and JSON schemas on every LLM iteration.
    def_cache: Arc<RwLock<HashMap<String, ToolDefinition>>>,
    /// Tools marked as deferred — not sent to LLM until keyword-activated or first called.
    deferred: Arc<RwLock<HashSet<String>>>,
    /// Maps agent_id → set of tool names owned by that agent's sidecar.
    agent_tools: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    policy: Arc<RwLock<Policy>>,
    process_registry: Arc<ProcessRegistry>,
    bridge: std::sync::RwLock<Option<Arc<mcp::Bridge>>>,
    plugin_store: std::sync::RwLock<Option<Arc<napp::plugin::PluginStore>>>,
    agent_loader: std::sync::RwLock<Option<Arc<napp::AgentLoader>>>,
    /// DB store for MCP proxy tools (OAuth token refresh during tool calls).
    store: std::sync::RwLock<Option<Arc<db::Store>>>,
    /// Browser manager, for closing a session's tab/page when a sub-agent finishes.
    browser_manager: std::sync::RwLock<Option<Arc<browser::Manager>>>,
    resource_permits: ResourcePermits,
}

impl Registry {
    pub fn new(policy: Policy) -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            def_cache: Arc::new(RwLock::new(HashMap::new())),
            deferred: Arc::new(RwLock::new(HashSet::new())),
            agent_tools: Arc::new(RwLock::new(HashMap::new())),
            policy: Arc::new(RwLock::new(policy)),
            process_registry: Arc::new(ProcessRegistry::new()),
            bridge: std::sync::RwLock::new(None),
            plugin_store: std::sync::RwLock::new(None),
            agent_loader: std::sync::RwLock::new(None),
            store: std::sync::RwLock::new(None),
            browser_manager: std::sync::RwLock::new(None),
            resource_permits: ResourcePermits::new(),
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

    /// Set the agent loader for PersonaTool filesystem access.
    pub fn set_agent_loader(&self, loader: Arc<napp::AgentLoader>) {
        *self.agent_loader.write().unwrap() = Some(loader);
    }

    /// Register a tool.
    pub async fn register(&self, tool: Box<dyn DynTool>) {
        let name = tool.name().to_string();
        let def = ToolDefinition {
            name: name.clone(),
            description: tool.description(),
            input_schema: tool.schema(),
        };
        let mut tools = self.tools.write().await;
        if tools.contains_key(&name) {
            warn!(tool = %name, "tool already registered, overwriting");
        }
        tools.insert(name.clone(), tool);
        drop(tools);
        self.def_cache.write().await.insert(name.clone(), def);
        debug!(tool = %name, "registered tool");
    }

    /// Register a tool and mark it as deferred (not sent to LLM until activated).
    pub async fn register_deferred(&self, tool: Box<dyn DynTool>) {
        let name = tool.name().to_string();
        self.deferred.write().await.insert(name.clone());
        self.register(tool).await;
        debug!(tool = %name, "registered as deferred");
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
            for name in &names {
                tools.remove(name);
                cache.remove(name);
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
    pub async fn list(&self) -> Vec<ToolDefinition> {
        self.def_cache.read().await.values().cloned().collect()
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
        cache
            .values()
            .filter(|def| !deferred.contains(&def.name) || activated.contains(&def.name))
            .cloned()
            .collect()
    }

    /// List deferred tools that haven't been activated yet as compact stubs.
    /// Returns (name, first_line_of_description) pairs for system prompt listing.
    pub async fn list_deferred_stubs(&self, activated: &HashSet<String>) -> Vec<(String, String)> {
        let deferred = self.deferred.read().await;
        let cache = self.def_cache.read().await;
        deferred
            .iter()
            .filter(|name| !activated.contains(name.as_str()))
            .filter_map(|name| {
                cache.get(name.as_str()).map(|def| {
                    let short = def.description.lines().next().unwrap_or("").to_string();
                    (name.clone(), short)
                })
            })
            .collect()
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
            self.def_cache.write().await.insert(name.to_string(), def);
            debug!(tool = %name, "refreshed cached tool definition");
        }
    }

    /// Get the full description of a specific tool (used for steering injection on first use).
    pub async fn get_tool_description(&self, name: &str) -> Option<String> {
        let cache = self.def_cache.read().await;
        cache.get(name).map(|def| def.description.clone())
    }

    /// Check whether a tool call is safe to run concurrently with other tools.
    ///
    /// Returns `true` for read-only operations, `false` for writes/mutations.
    /// Returns `false` for unknown tools (conservative default).
    pub async fn is_concurrent_safe(&self, tool_name: &str, input: &serde_json::Value) -> bool {
        let tools = self.tools.read().await;
        tools
            .get(tool_name)
            .or_else(|| tools.get(strip_mcp_prefix(tool_name)))
            .map_or(false, |tool| tool.is_concurrent_safe(input))
    }

    /// List tools filtered by per-entity permissions.
    /// Tools whose category is denied are excluded from the list sent to the LLM.
    pub async fn list_with_permissions(
        &self,
        permissions: Option<&std::collections::HashMap<String, bool>>,
    ) -> Vec<ToolDefinition> {
        let cache = self.def_cache.read().await;
        cache
            .values()
            .filter(|def| {
                if let Some(perms) = permissions {
                    let cat = tool_category(&def.name);
                    if let Some(&allowed) = perms.get(cat) {
                        return allowed;
                    }
                }
                true // no permission set = allowed
            })
            .cloned()
            .collect()
    }

    /// Execute a tool and return the result.
    ///
    /// Uses a two-phase approach to avoid holding the `tools` read-lock
    /// while waiting for a resource permit:
    ///
    /// 1. **Validate** — read-lock tools, check safeguard + policy, call
    ///    `resource_permit()` to determine which physical resource (if any)
    ///    the tool needs. Drop the read-lock.
    /// 2. **Acquire permit** — if a resource is needed, block until the
    ///    corresponding `ResourcePermits` mutex is free.
    /// 3. **Execute** — re-read-lock tools and run `execute_dyn()`. The
    ///    permit guard stays alive for the duration of execution.
    pub async fn execute(
        &self,
        ctx: &ToolContext,
        tool_name: &str,
        input: serde_json::Value,
    ) -> ToolResult {
        // Try full name first (for MCP proxy tools like mcp__monument_sh__project),
        // then fall back to stripped name (for external MCP clients calling STRAP tools).
        let name = tool_name;

        debug!(tool = %name, "executing tool");

        // ── Phase 1: Validate + determine resource permit ──────────
        let (name, input) = if let Some((strap_name, params)) = resolve_flat_alias(name) {
            let mut merged = input;
            if let Some(obj) = merged.as_object_mut() {
                for (k, v) in params {
                    obj.entry(&k).or_insert(v);
                }
            }
            debug!(alias = %tool_name, resolved = %strap_name, "flat-name alias resolved");
            (strap_name, merged)
        } else {
            (name.to_string(), input)
        };
        let name = name.as_str();

        let permit_kind = {
            let tools = self.tools.read().await;
            let tool = match tools
                .get(name)
                .or_else(|| tools.get(strip_mcp_prefix(name)))
            {
                Some(t) => t,
                None => {
                    warn!(tool = %name, "unknown tool");
                    let available: Vec<&str> = tools.keys().map(|s| s.as_str()).collect();
                    let correction = tool_correction(name);
                    return ToolResult::error(format!(
                        "TOOL ERROR: {:?} does not exist. You do NOT have that tool. Do NOT call it again.\n\n{}\nYour available tools are: {}",
                        name,
                        correction,
                        available.join(", ")
                    ));
                }
            };

            // Hard safety guard — unconditional, cannot be overridden
            if let Some(err) = safeguard::check_safeguard(name, &input) {
                warn!(tool = %name, error = %err, "safeguard blocked");
                return ToolResult::error(err);
            }

            // Path scope guard — restrict file/shell to allowed directories
            if let Some(err) = safeguard::check_path_scope(name, &input, &ctx.allowed_paths) {
                warn!(tool = %name, error = %err, "path scope blocked");
                return ToolResult::error(err);
            }

            // Check origin-based deny list
            let resource = input.get("resource").and_then(|v| v.as_str());
            {
                let policy = self.policy.read().await;
                if policy.is_denied_for_origin(ctx.origin, name, resource) {
                    return ToolResult::error(format!(
                        "Tool '{}' is not permitted from {:?} origin",
                        name, ctx.origin
                    ));
                }
            }

            tool.resource_permit(&input)
        }; // ← tools read-lock dropped

        // ── Phase 1b: Entity permission check ─────────────────────
        if let Some(ref perms) = ctx.entity_permissions {
            let category = tool_category(name);
            if let Some(&allowed) = perms.get(category) {
                if !allowed {
                    return ToolResult::error(format!(
                        "Tool '{}' is denied for this entity (category '{}')",
                        name, category
                    ));
                }
            }
        }

        // ── Phase 1c: Entity resource grant check ─────────────────
        if let Some(ref grants) = ctx.resource_grants {
            if let Some(kind) = &permit_kind {
                let resource_name = match kind {
                    ResourceKind::Screen => "screen",
                    ResourceKind::Browser => "browser",
                };
                if let Some(grant) = grants.get(resource_name) {
                    if grant == "deny" {
                        return ToolResult::error(format!(
                            "Resource '{}' is denied for this entity",
                            resource_name
                        ));
                    }
                }
            }
        }

        // ── Phase 2: Acquire resource permit (may block) ───────────
        let _permit_guard = if let Some(kind) = permit_kind {
            debug!(tool = %name, resource = ?kind, "acquiring resource permit");
            Some(self.resource_permits.acquire(kind).await)
        } else {
            None
        };

        // ── Phase 3: Re-acquire lock and execute ───────────────────
        let tools = self.tools.read().await;
        match tools
            .get(name)
            .or_else(|| tools.get(strip_mcp_prefix(name)))
        {
            Some(tool) => tool.execute_dyn(ctx, input).await,
            None => ToolResult::error(format!(
                "Tool '{}' was unregistered during permit acquisition",
                name
            )),
        }
    }

    /// Update the policy.
    pub async fn set_policy(&self, policy: Policy) {
        *self.policy.write().await = policy;
    }

    /// Get a reference to the process registry.
    pub fn process_registry(&self) -> &Arc<ProcessRegistry> {
        &self.process_registry
    }

    /// Get a reference to the policy.
    pub async fn policy(&self) -> Policy {
        self.policy.read().await.clone()
    }

    /// Register the default set of tools (os tool only — no DB access).
    pub async fn register_defaults(&self) {
        let policy = self.policy.read().await.clone();
        let mut os_tool = crate::os_tool::OsTool::new(policy, self.process_registry.clone());
        let ps_opt = self.plugin_store.read().unwrap().clone();
        if let Some(ps) = ps_opt {
            os_tool = os_tool.with_plugin_store(ps);
        }
        self.register(Box::new(os_tool)).await;
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
            None, // vision_analyzer
            hybrid_searcher,
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
        vision_analyzer: Option<Arc<dyn crate::bot_tool::VisionAnalyzer>>,
        hybrid_searcher: Option<Arc<dyn crate::bot_tool::HybridSearcher>>,
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

        // OS tool (file, shell, desktop, apps, settings, music, keychain, search, PIM) — CORE.
        // The file/shell meta-tool is the agent's primary way to act; it must always be
        // visible. It previously was deferred to save ~8-10K schema tokens, but that left
        // the model blind to its own core capability — it had to tool_search to discover
        // os, and the tool unloaded when that discovery message was evicted from the sliding
        // window, causing mid-task thrashing. The system-prompt prefix is cached
        // (Anthropic cache_control / Janus prefix caching), so the schema costs ~10% on
        // cache reads — far cheaper than the discovery round-trips and context pollution
        // that deferral caused. Reserve deferral for genuinely optional surface
        // (per-skill, MCP, niche platform tools).
        let policy = self.policy.read().await.clone();
        let mut os_tool = crate::os_tool::OsTool::new(policy, self.process_registry.clone())
            .with_store(store.clone());
        let ps_opt = self.plugin_store.read().unwrap().clone();
        if let Some(ps) = ps_opt {
            os_tool = os_tool.with_plugin_store(ps);
        }
        self.register(Box::new(os_tool)).await;

        // Web tool (HTTP fetch + search + browser) — requires "web" permission
        if allowed("web") {
            let mut web_tool = crate::web_tool::WebTool::new().with_store(store.clone());
            if let Some(mgr) = browser_manager {
                web_tool = web_tool.with_browser(mgr);
            }
            if let Some(ref bc) = broadcaster {
                web_tool = web_tool.with_broadcaster(bc.clone());
            }
            self.register(Box::new(web_tool)).await;
        }

        // Agent tool (memory, tasks, sessions, context, advisors, ask, runs, registry) — always registered (core)
        let mut agent_tool = crate::bot_tool::AgentTool::new(store.clone(), orchestrator.clone());
        let runner_for_events = advisor_runner.clone();
        if let Some(runner) = advisor_runner {
            agent_tool = agent_tool.with_advisor_runner(runner);
        }
        if let Some(vision) = vision_analyzer {
            agent_tool = agent_tool.with_vision_analyzer(vision);
        }
        if let Some(searcher) = hybrid_searcher {
            agent_tool = agent_tool.with_hybrid_searcher(searcher);
        }
        if let Some(sa) = structured_agent {
            agent_tool = agent_tool.with_structured_agent(sa);
        }
        if let Some(rq) = run_querier {
            agent_tool = agent_tool.with_run_querier(rq);
        }

        // Persona/registry resource — agent management, delegation, installed agents
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
            let persona = crate::agent_tool::PersonaTool::new(
                store.clone(),
                agent_reg,
                agent_loader,
                orchestrator.clone(),
            );
            agent_tool = agent_tool.with_persona(persona);
        }

        self.register(Box::new(agent_tool)).await;

        // Event tool (scheduled tasks / cron) — always registered (core)
        let mut event_tool = crate::event_tool::EventTool::new(store.clone());
        if let Some(runner) = runner_for_events {
            event_tool = event_tool.with_runner(runner);
        }
        self.register(Box::new(event_tool)).await;

        // Skill tool (skill management) — always registered (core)
        if let Some(ref loader) = skill_loader {
            let mut skill_tool =
                crate::skill_tool::SkillTool::new(loader.clone()).with_store(store.clone());
            // Wire the plugin registry so skill discover/help can redirect
            // when the LLM confuses a plugin slug for a skill name.
            let ps_opt = self.plugin_store.read().unwrap().clone();
            if let Some(ps) = ps_opt {
                skill_tool = skill_tool.with_plugin_store(ps);
            }
            self.register(Box::new(skill_tool)).await;
        } else {
            let data = config::data_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let installed_dir = data.join("nebo").join("skills");
            let user_dir = data.join("user").join("skills");
            let loader_default = Arc::new(crate::skills::Loader::new(installed_dir, user_dir));
            let mut skill_tool = crate::skill_tool::SkillTool::new(loader_default);
            let ps_opt = self.plugin_store.read().unwrap().clone();
            if let Some(ps) = ps_opt {
                skill_tool = skill_tool.with_plugin_store(ps);
            }
            self.register(Box::new(skill_tool)).await;
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
            self.register_deferred(Box::new(execute_tool)).await;
        }

        // Message tool (owner notifications) — always registered (core)
        self.register(Box::new(crate::message_tool::MessageTool::new(
            store.clone(),
        )))
        .await;

        // Work tool (workflow lifecycle + execution) — deferred (only activated when user mentions workflows)
        if let Some(manager) = workflow_manager {
            self.register_deferred(Box::new(crate::workflows::WorkTool::new(manager)))
                .await;
        }

        self.register_deferred(Box::new(crate::publisher_tool::PublisherTool::new(
            store.clone(),
        )))
        .await;

        // Notebook tool (.ipynb cell editing) — deferred (activated when the user
        // mentions notebooks / Jupyter / .ipynb).
        self.register_deferred(Box::new(crate::notebook_tool::NotebookTool::new()))
            .await;

        // Plugin tool — ALWAYS registered when a plugin store exists, even with ZERO
        // plugins installed, so the agent can always list/discover/install plugins from
        // the system prompt (no chicken-and-egg where the tool only appears after the
        // first install). Its description() handles the zero-state and points at discover.
        let ps_opt = self.plugin_store.read().unwrap().clone();
        if let Some(ps) = ps_opt {
            let mut pt = crate::plugin_tool::PluginTool::new(ps, store.clone());
            if let Some(ref bc) = broadcaster {
                pt = pt.with_broadcaster(bc.clone());
            }
            self.register(Box::new(pt)).await;
        }

        // VM tool (isolated Linux environment for builds/toolchains) — deferred
        // Activated when agent needs Go, gcc, Docker, or clean build env
        self.register_deferred(Box::new(crate::vm_tool::VmTool::new()))
            .await;

        // Loop tool (NeboAI comms: dm, channel, group, topic) — requires "loop" permission.
        // The comm handle exists from startup; the real LoopTool's per-action
        // `is_connected()` check reflects the live connection state, so it is always
        // registered when the handle is available (even before NeboAI connects).
        if allowed("loop") {
            if let Some(ref comm) = comm_plugin {
                self.register(Box::new(crate::loop_tool::LoopTool::new(comm.clone())))
                    .await;
            }
        }
    }
}

/// MCP proxy tool that delegates execution to the bridge.
struct McpProxyTool {
    proxy_name: String,
    original_name: String,
    tool_description: String,
    tool_schema: Option<serde_json::Value>,
    integration_id: String,
    bridge: Arc<mcp::Bridge>,
    store: Arc<db::Store>,
}

impl DynTool for McpProxyTool {
    fn name(&self) -> &str {
        &self.proxy_name
    }

    fn description(&self) -> String {
        self.tool_description.clone()
    }

    fn schema(&self) -> serde_json::Value {
        self.tool_schema
            .clone()
            .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}))
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a crate::origin::ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            crate::mcp_tool::call_mcp_tool(
                &self.store,
                &self.bridge,
                &self.integration_id,
                &self.original_name,
                input,
            )
            .await
        })
    }
}

impl mcp::bridge::ProxyToolRegistry for Registry {
    fn register_proxy(
        &self,
        name: &str,
        original_name: &str,
        description: &str,
        schema: Option<serde_json::Value>,
        integration_id: &str,
    ) {
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
        let tool = McpProxyTool {
            proxy_name: name.to_string(),
            original_name: original_name.to_string(),
            tool_description: description.to_string(),
            tool_schema: schema,
            integration_id: integration_id.to_string(),
            bridge,
            store,
        };
        // MCP proxy tools are deferred — activated by keyword matching or direct call
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(self.register_deferred(Box::new(tool)));
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
}

/// Map a tool name to its permission category.
/// Categories match the keys used in entity_config.permissions JSON.
fn tool_category(name: &str) -> &str {
    match name {
        "web" => "web",
        "os" => "desktop",
        "agent" => "memory",
        "skill" => "filesystem",
        "work" => "filesystem",
        "loop" => "web",
        "plugin" => "desktop",
        _ => "other",
    }
}

/// Strip MCP namespace prefix from tool names.
/// `mcp__{server}__{tool}` → `{tool}`
/// Strip MCP namespace prefix for external client tool calls.
/// e.g. "mcp__nebo-agent__system" → "system" (for STRAP tools called via JSON-RPC).
/// Used as a fallback — execute() tries the full name first (for proxy tools),
/// then falls back to the stripped name (for external clients calling STRAP tools).
fn strip_mcp_prefix(name: &str) -> &str {
    if !name.starts_with("mcp__") {
        return name;
    }
    let parts: Vec<&str> = name.splitn(3, "__").collect();
    if parts.len() == 3 { parts[2] } else { name }
}

/// Resolve flat tool names (Claude Code convention) to STRAP tool + injected params.
/// Returns (strap_tool_name, params_to_inject) or None if not a known alias.
fn resolve_flat_alias(name: &str) -> Option<(String, Vec<(String, serde_json::Value)>)> {
    let lc = name.to_lowercase();
    let (tool, params): (&str, Vec<(&str, &str)>) = match lc.as_str() {
        // File operations → os
        "file_read" | "read_file" | "fileread" | "read" => {
            ("os", vec![("resource", "file"), ("action", "read")])
        }
        "file_write" | "write_file" | "filewrite" => {
            ("os", vec![("resource", "file"), ("action", "write")])
        }
        "file_edit" | "edit_file" | "fileedit" | "edit" => {
            ("os", vec![("resource", "file"), ("action", "edit")])
        }
        "grep" | "grep_tool" | "greptool" | "file_grep" => {
            ("os", vec![("resource", "file"), ("action", "grep")])
        }
        "glob" | "glob_tool" | "globtool" | "file_glob" => {
            ("os", vec![("resource", "file"), ("action", "glob")])
        }
        // Shell → os
        "bash" | "shell" | "bash_tool" | "bashtool" | "run_command" | "exec" => {
            ("os", vec![("resource", "shell"), ("action", "exec")])
        }
        // Web operations → web
        "web_search" | "websearch" | "websearchtool" | "search" => {
            ("web", vec![("action", "search")])
        }
        "web_fetch" | "webfetch" | "webfetchtool" | "fetch" | "fetch_url" => {
            ("web", vec![("action", "fetch")])
        }
        _ => return None,
    };
    let params = params
        .into_iter()
        .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
        .collect();
    Some((tool.to_string(), params))
}

/// Provide specific correction for known hallucinated tool names.
fn tool_correction(name: &str) -> String {
    match name.to_lowercase().as_str() {
        "websearch" | "web_search" => {
            "INSTEAD USE: web(action: \"search\", query: \"your search query\")".to_string()
        }
        "webfetch" | "web_fetch" => {
            "INSTEAD USE: web(action: \"fetch\", url: \"https://...\")".to_string()
        }
        "read" | "file" => {
            "INSTEAD USE: os(resource: \"file\", action: \"read\", path: \"/path/to/file\")".to_string()
        }
        "write" => {
            "INSTEAD USE: os(resource: \"file\", action: \"write\", path: \"/path\", content: \"...\")".to_string()
        }
        "edit" => {
            "INSTEAD USE: os(resource: \"file\", action: \"edit\", path: \"/path\", old_string: \"...\", new_string: \"...\")".to_string()
        }
        "grep" => {
            "INSTEAD USE: os(resource: \"file\", action: \"grep\", pattern: \"...\", path: \"/dir\")".to_string()
        }
        "glob" => {
            "INSTEAD USE: os(resource: \"file\", action: \"glob\", pattern: \"**/*.go\")".to_string()
        }
        "bash" | "shell" => {
            "INSTEAD USE: os(resource: \"shell\", action: \"exec\", command: \"...\")".to_string()
        }
        "system" => {
            "INSTEAD USE: os(resource: \"file\", action: \"read\", ...) or os(resource: \"shell\", action: \"exec\", ...) — system is now os".to_string()
        }
        "bot" => {
            "INSTEAD USE: agent(resource: \"memory\", action: \"recall\", ...) — bot is now agent".to_string()
        }
        "desktop" => {
            "INSTEAD USE: os(resource: \"window\", action: \"list\") or os(resource: \"capture\", action: \"screenshot\") — desktop is now under os".to_string()
        }
        "app" => {
            "INSTEAD USE: os(resource: \"app\", action: \"launch\", app: \"...\") — app is now under os".to_string()
        }
        "settings" => {
            "INSTEAD USE: os(resource: \"settings\", action: \"volume\", value: 50) — settings is now under os".to_string()
        }
        "music" => {
            "INSTEAD USE: os(resource: \"music\", action: \"play\") — music is now under os".to_string()
        }
        "keychain" => {
            "INSTEAD USE: os(resource: \"keychain\", action: \"get\", service: \"...\") — keychain is now under os".to_string()
        }
        "spotlight" => {
            "INSTEAD USE: os(resource: \"search\", action: \"search\", query: \"...\") — spotlight is now under os".to_string()
        }
        "organizer" => {
            "INSTEAD USE: os(resource: \"mail\", action: \"unread\") or os(resource: \"calendar\", action: \"today\") — organizer is now under os".to_string()
        }
        "gws" | "google-workspace" | "gmail" | "gcalendar" | "gdrive" | "gsheets" | "gdocs" => {
            "INSTEAD USE: plugin(resource: \"gws\", command: \"gmail +triage --max 5\") — use the plugin tool with the plugin slug as resource".to_string()
        }
        "napp" | "install" | "package" => {
            "INSTEAD USE: skill(action: \"catalog\") to see available skills, skill(action: \"install\", code: \"SKILL-XXXX-XXXX\") to install".to_string()
        }
        "workflow" | "automation" | "work_flow" => {
            "INSTEAD USE: work(action: \"list\") to see workflows, work(resource: \"name\", action: \"run\") to run".to_string()
        }
        _ => {
            if name.starts_with("mcp__") {
                "INSTEAD USE: mcp(server: \"<server>\", resource: \"<tool>\", action: \"<action>\") — call MCP tools via the mcp STRAP tool, not by their namespaced name".to_string()
            } else {
                format!(
                    "'{}' is not a recognized tool. Use skill(action: \"discover\", query: \"{}\") to find a skill, \
                     or check your available tools with tool_search(query: \"{}\").",
                    name, name, name
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_mcp_prefix() {
        assert_eq!(strip_mcp_prefix("web"), "web");
        assert_eq!(strip_mcp_prefix("mcp__nebo-agent__web"), "web");
        assert_eq!(strip_mcp_prefix("mcp__server__file"), "file");
        assert_eq!(strip_mcp_prefix("mcp__only_one"), "mcp__only_one");
    }

    #[test]
    fn test_tool_correction() {
        assert!(tool_correction("read").contains("os"));
        assert!(tool_correction("bash").contains("os"));
        assert!(tool_correction("websearch").contains("web"));
        assert!(tool_correction("system").contains("os"));
        assert!(tool_correction("bot").contains("agent"));
        assert!(tool_correction("desktop").contains("os"));
        assert!(tool_correction("music").contains("os"));
        assert!(tool_correction("unknown_tool").contains("not a recognized tool"));
        assert!(tool_correction("unknown_tool").contains("skill(action: \"discover\""));
        assert!(tool_correction("unknown_tool").contains("tool_search"));
        assert!(tool_correction("mcp__server__tool").contains("mcp(server:"));
    }
}
