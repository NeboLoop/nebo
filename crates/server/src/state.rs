use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, oneshot};

use config::Config;
use db::Store;
use auth::AuthService;
use agent::{LaneManager, Runner};
use tools::Registry;

use comm::PluginManager;

use serde::Serialize;

use crate::handlers::ws::ClientHub;
use crate::run_registry::RunRegistry;
use crate::workflow_manager::WorkflowManagerImpl;

/// Janus AI usage stats stored in memory, updated from rate limit headers or direct API call.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JanusUsage {
    pub session_limit_tokens: u64,
    pub session_remaining_tokens: u64,
    pub session_reset_at: String,
    pub weekly_limit_tokens: u64,
    pub weekly_remaining_tokens: u64,
    pub weekly_reset_at: String,
    /// ISO 8601 timestamp of when this data was last updated
    #[serde(skip_serializing_if = "String::is_empty")]
    pub updated_at: String,
}

/// Shared application state passed to all handlers via Axum extractors.
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub store: Arc<Store>,
    pub auth: Arc<AuthService>,
    pub hub: Arc<ClientHub>,
    pub runner: Arc<Runner>,
    pub tools: Arc<Registry>,
    pub bridge: Arc<mcp::Bridge>,
    /// Tool package registry for managing installable tools (.napp packages)
    pub napp_registry: Arc<napp::Registry>,
    /// Workflow manager for workflow lifecycle and execution
    pub workflow_manager: Arc<WorkflowManagerImpl>,
    /// Models catalog loaded from models.yaml (read-only config for routing/aliases)
    pub models_config: Arc<config::ModelsConfig>,
    /// CLI tool detection results (claude, codex, gemini)
    pub cli_statuses: Arc<config::AllCliStatuses>,
    /// Lane manager for per-lane task queuing and concurrency limits
    pub lanes: Arc<LaneManager>,
    /// Snapshot store for caching browser accessibility snapshots
    pub snapshot_store: Arc<browser::SnapshotStore>,
    /// Extension bridge for Chrome extension communication
    pub extension_bridge: Arc<browser::ExtensionBridge>,
    /// Communications plugin manager (NeboLoop WebSocket, Loopback)
    pub comm_manager: Arc<PluginManager>,
    /// Pending tool approval requests: tool_call_id -> sender
    pub approval_channels: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    /// Pending ask requests: question_id -> sender
    pub ask_channels: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
    /// Staged update binary ready for apply (path + version)
    pub update_pending: Arc<Mutex<Option<(std::path::PathBuf, String)>>>,
    /// Hook dispatcher for napp hook subscriptions
    pub hooks: Arc<napp::HookDispatcher>,
    /// Shared MCP context for CLI provider tool calls (set by runner before each agentic loop)
    pub mcp_context: Arc<tokio::sync::Mutex<tools::ToolContext>>,
    /// Event bus for workflow-to-workflow and system events
    pub event_bus: tools::EventBus,
    /// Event dispatcher that matches events to role subscriptions
    pub event_dispatcher: Arc<workflow::events::EventDispatcher>,
    /// NeboLoop plan tier (free, pro, team, enterprise) — updated by AUTH_OK handler
    pub plan_tier: Arc<tokio::sync::RwLock<String>>,
    /// Skill loader for hot-reload after marketplace installs
    pub skill_loader: Arc<tools::skills::Loader>,
    /// Registry of currently active agents — each agent is its own bot with isolated persona
    pub agent_registry: tools::AgentRegistry,
    /// Autonomous agent workers — one per active agent, manages trigger lifecycle
    pub agent_workers: Arc<agent::AgentWorkerRegistry>,
    /// Janus AI usage stats (session/weekly token limits), updated from rate limit headers
    pub janus_usage: Arc<tokio::sync::RwLock<Option<JanusUsage>>>,
    /// Plugin store for shared binary management (plugins downloaded once, shared across skills)
    pub plugin_store: Arc<napp::plugin::PluginStore>,
    /// User presence tracker — per-session focused/unfocused/away state
    pub presence: Arc<agent::PresenceTracker>,
    /// Proactive inbox — in-memory queue for background task results
    pub proactive_inbox: Arc<agent::ProactiveInbox>,
    /// Global registry of all active agent runs — single source of truth
    pub run_registry: RunRegistry,
}
