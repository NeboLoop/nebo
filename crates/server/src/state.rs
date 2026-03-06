use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, oneshot};

use config::Config;
use db::Store;
use auth::AuthService;
use agent::{LaneManager, Runner};
use tools::Registry;

use comm::PluginManager;

use crate::handlers::ws::ClientHub;
use crate::napp_manager::NappManagerImpl;
use crate::workflow_manager::WorkflowManagerImpl;

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
    /// Napp manager for agent tool dispatch and lifecycle
    pub napp_manager: Arc<NappManagerImpl>,
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
}
