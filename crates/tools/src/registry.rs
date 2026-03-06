use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use ai::ToolDefinition;

use crate::origin::ToolContext;
use crate::policy::Policy;
use crate::process::ProcessRegistry;
use crate::safeguard;
use crate::system_tool::SystemTool;

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
}

impl ToolResult {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            image_url: None,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            image_url: None,
        }
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
    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>>;
}

/// Registry manages available tools.
pub struct Registry {
    tools: Arc<RwLock<HashMap<String, Box<dyn DynTool>>>>,
    policy: Arc<RwLock<Policy>>,
    process_registry: Arc<ProcessRegistry>,
    bridge: std::sync::RwLock<Option<Arc<mcp::Bridge>>>,
}

impl Registry {
    pub fn new(policy: Policy) -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            policy: Arc::new(RwLock::new(policy)),
            process_registry: Arc::new(ProcessRegistry::new()),
            bridge: std::sync::RwLock::new(None),
        }
    }

    /// Set the MCP bridge for proxy tool execution.
    pub fn set_bridge(&self, bridge: Arc<mcp::Bridge>) {
        *self.bridge.write().unwrap() = Some(bridge);
    }

    /// Register a tool.
    pub async fn register(&self, tool: Box<dyn DynTool>) {
        let name = tool.name().to_string();
        let mut tools = self.tools.write().await;
        if tools.contains_key(&name) {
            warn!(tool = %name, "tool already registered, overwriting");
        }
        tools.insert(name.clone(), tool);
        debug!(tool = %name, "registered tool");
    }

    /// Unregister a tool by name.
    pub async fn unregister(&self, name: &str) {
        let mut tools = self.tools.write().await;
        if tools.remove(name).is_some() {
            debug!(tool = %name, "unregistered tool");
        }
    }

    /// Get a tool by name (returns None if not found).
    pub async fn get_tool_names(&self) -> Vec<String> {
        self.tools.read().await.keys().cloned().collect()
    }

    /// List all tools as AI tool definitions.
    pub async fn list(&self) -> Vec<ToolDefinition> {
        let tools = self.tools.read().await;
        tools
            .values()
            .map(|tool| ToolDefinition {
                name: tool.name().to_string(),
                description: tool.description(),
                input_schema: tool.schema(),
            })
            .collect()
    }

    /// Execute a tool and return the result.
    pub async fn execute(
        &self,
        ctx: &ToolContext,
        tool_name: &str,
        input: serde_json::Value,
    ) -> ToolResult {
        // Strip MCP prefix if needed
        let name = strip_mcp_prefix(tool_name);

        debug!(tool = %name, "executing tool");

        let tools = self.tools.read().await;
        let tool = match tools.get(name) {
            Some(t) => t,
            None => {
                warn!(tool = %name, "unknown tool");
                let available: Vec<&str> = tools.keys().map(|s| s.as_str()).collect();
                let correction = tool_correction(name);
                return ToolResult::error(format!(
                    "TOOL ERROR: {:?} does not exist. You do NOT have that tool. Do NOT call it again.\n\n{}\nYour available tools are: {}",
                    name, correction, available.join(", ")
                ));
            }
        };

        // Hard safety guard — unconditional, cannot be overridden
        if let Some(err) = safeguard::check_safeguard(name, &input) {
            warn!(tool = %name, error = %err, "safeguard blocked");
            return ToolResult::error(err);
        }

        // Check origin-based deny list
        let resource = input
            .get("resource")
            .and_then(|v| v.as_str());
        {
            let policy = self.policy.read().await;
            if policy.is_denied_for_origin(ctx.origin, name, resource) {
                return ToolResult::error(format!(
                    "Tool '{}' is not permitted from {:?} origin",
                    name, ctx.origin
                ));
            }
        }

        tool.execute_dyn(ctx, input).await
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

    /// Register the default set of tools (system domain tool only — no DB access).
    pub async fn register_defaults(&self) {
        let policy = self.policy.read().await.clone();
        let system_tool = SystemTool::new(policy, self.process_registry.clone());
        self.register(Box::new(system_tool)).await;
    }

    /// Register all domain tools including those that need DB access.
    pub async fn register_all(&self, store: Arc<db::Store>, orchestrator: crate::OrchestratorHandle) {
        self.register_all_with_browser(store, None, orchestrator, None, None, None).await;
    }

    /// Register all domain tools with optional browser manager.
    /// The `permissions` map controls which capability categories are enabled.
    /// Keys: "chat", "file", "shell", "web", "contacts", "desktop", "media", "system".
    /// A None map registers all tools (no filtering). A missing key defaults to denied.
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
            None,
            None,
            None,
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
        napp_manager: Option<Arc<dyn crate::tools::NappManager>>,
        workflow_manager: Option<Arc<dyn crate::workflows::WorkflowManager>>,
        permissions: Option<&HashMap<String, bool>>,
    ) {
        let allowed = |category: &str| -> bool {
            match permissions {
                None => true, // No permissions map = allow all
                Some(map) => *map.get(category).unwrap_or(&false),
            }
        };

        // System tool (file + shell) — always registered (core functionality)
        let policy = self.policy.read().await.clone();
        let system_tool = SystemTool::new(policy, self.process_registry.clone());
        self.register(Box::new(system_tool)).await;

        // Web tool (HTTP fetch + search + browser) — requires "web" permission
        if allowed("web") {
            let web_tool = match browser_manager {
                Some(mgr) => crate::web_tool::WebTool::new().with_browser(mgr),
                None => crate::web_tool::WebTool::new(),
            };
            self.register(Box::new(web_tool)).await;
        }

        // Bot tool (memory, tasks, sessions, profile, sub-agents) — always registered (core)
        let mut bot_tool = crate::bot_tool::BotTool::new(store.clone(), orchestrator);
        if let Some(runner) = advisor_runner {
            bot_tool = bot_tool.with_advisor_runner(runner);
        }
        if let Some(searcher) = hybrid_searcher {
            bot_tool = bot_tool.with_hybrid_searcher(searcher);
        }
        self.register(Box::new(bot_tool)).await;

        // Event tool (scheduled tasks / cron) — always registered (core)
        self.register(Box::new(crate::event_tool::EventTool::new(store.clone()))).await;

        // Skill tool (skill management) — always registered (core)
        if let Some(loader) = skill_loader {
            self.register(Box::new(crate::skill_tool::SkillTool::new(loader))).await;
        } else {
            let skills_dir = config::data_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")).join("skills");
            let loader = Arc::new(crate::skills::Loader::new(skills_dir, None));
            self.register(Box::new(crate::skill_tool::SkillTool::new(loader))).await;
        }

        // Message tool (owner notifications) — always registered (core)
        self.register(Box::new(crate::message_tool::MessageTool::new(store))).await;

        // Desktop tool (window, input, clipboard, notification, capture) — requires "desktop" permission
        if allowed("desktop") {
            self.register(Box::new(crate::desktop_tool::DesktopTool::new())).await;
        }

        // Settings tool (volume, brightness, wifi, bluetooth, battery) — requires "system" permission
        if allowed("system") {
            self.register(Box::new(crate::settings_tool::SettingsTool::new())).await;
        }

        // Spotlight tool (file search via OS index) — requires "system" permission
        if allowed("system") {
            self.register(Box::new(crate::spotlight_tool::SpotlightTool::new())).await;
        }

        // Tool tool (napp lifecycle + dispatch) — always registered when manager is provided
        if let Some(manager) = napp_manager {
            self.register(Box::new(crate::tools::ToolTool::new(manager))).await;
        }

        // Work tool (workflow lifecycle + execution) — always registered when manager is provided
        if let Some(manager) = workflow_manager {
            self.register(Box::new(crate::workflows::WorkTool::new(manager))).await;
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
            match self
                .bridge
                .call_tool(&self.integration_id, &self.original_name, input)
                .await
            {
                Ok(result) => {
                    if result.is_error {
                        ToolResult::error(result.content)
                    } else {
                        ToolResult::ok(result.content)
                    }
                }
                Err(e) => ToolResult::error(format!("MCP tool call failed: {}", e)),
            }
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
        let tool = McpProxyTool {
            proxy_name: name.to_string(),
            original_name: original_name.to_string(),
            tool_description: description.to_string(),
            tool_schema: schema,
            integration_id: integration_id.to_string(),
            bridge: self.bridge.read().unwrap().as_ref().expect("bridge not set").clone(),
        };
        // Use Handle to bridge sync → async
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.block_on(self.register(Box::new(tool)));
        }
    }

    fn unregister_proxy(&self, name: &str) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.block_on(self.unregister(name));
        }
    }
}

/// Strip MCP namespace prefix from tool names.
/// `mcp__{server}__{tool}` → `{tool}`
fn strip_mcp_prefix(name: &str) -> &str {
    if !name.starts_with("mcp__") {
        return name;
    }
    // mcp__{server}__{tool} → tool
    let parts: Vec<&str> = name.splitn(3, "__").collect();
    if parts.len() == 3 {
        parts[2]
    } else {
        name
    }
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
        "read" => {
            "INSTEAD USE: system(resource: \"file\", action: \"read\", path: \"/path/to/file\")"
                .to_string()
        }
        "write" => {
            "INSTEAD USE: system(resource: \"file\", action: \"write\", path: \"/path\", content: \"...\")"
                .to_string()
        }
        "edit" => {
            "INSTEAD USE: system(resource: \"file\", action: \"edit\", path: \"/path\", old_string: \"...\", new_string: \"...\")"
                .to_string()
        }
        "grep" => {
            "INSTEAD USE: system(resource: \"file\", action: \"grep\", pattern: \"...\", path: \"/dir\")"
                .to_string()
        }
        "glob" => {
            "INSTEAD USE: system(resource: \"file\", action: \"glob\", pattern: \"**/*.go\")"
                .to_string()
        }
        "bash" => {
            "INSTEAD USE: system(resource: \"shell\", action: \"exec\", command: \"...\")"
                .to_string()
        }
        "file" => {
            "INSTEAD USE: system(resource: \"file\", action: \"read\", path: \"...\") — file operations are under the system tool"
                .to_string()
        }
        "shell" => {
            "INSTEAD USE: system(resource: \"shell\", action: \"exec\", command: \"...\") — shell operations are under the system tool"
                .to_string()
        }
        "app" | "napp" | "install" | "package" => {
            "INSTEAD USE: tool(action: \"list\") to see installed tools, tool(action: \"install\", code: \"TOOL-XXXX-XXXX\") to install, or tool(resource: \"tool-name\", action: \"...\") to dispatch"
                .to_string()
        }
        "workflow" | "automation" | "work_flow" => {
            "INSTEAD USE: work(action: \"list\") to see workflows, work(resource: \"name\", action: \"run\") to run, or work(action: \"install\", code: \"WORK-XXXX-XXXX\") to install"
                .to_string()
        }
        _ => "Check your available tools and use the correct name.".to_string(),
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
        assert!(tool_correction("read").contains("system"));
        assert!(tool_correction("bash").contains("system"));
        assert!(tool_correction("websearch").contains("web"));
        assert!(tool_correction("unknown_tool").contains("Check your available tools"));
    }
}
