use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

/// Lightweight run info for tool-level visibility (no Arc/Mutex internals).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInfo {
    pub run_id: String,
    pub entity_id: String,
    pub entity_name: String,
    pub origin: String,
    pub tool_call_count: u32,
    pub current_tool: String,
    pub elapsed_secs: u64,
    pub parent_run_id: Option<String>,
}

/// Trait for querying and controlling active runs from within tools.
///
/// Implemented by the server's RunRegistry. Scoping rules:
/// - Primary agent (entity_id "main") sees ALL runs.
/// - Persona agents see only their own run + own sub-agents.
pub trait RunQuerier: Send + Sync {
    /// List runs visible to the given caller.
    fn list_runs(
        &self,
        caller_entity_id: &str,
    ) -> Pin<Box<dyn Future<Output = Vec<RunInfo>> + Send + '_>>;

    /// Cancel a run, if the caller is authorized.
    /// Returns Ok(true) if cancelled, Ok(false) if not found, Err if unauthorized.
    fn cancel_run(
        &self,
        run_id: &str,
        caller_entity_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>>;
}

/// Late-binding handle — created empty, filled after RunRegistry is built.
pub type RunQuerierHandle = Arc<OnceLock<Box<dyn RunQuerier>>>;

/// Create a new empty handle.
pub fn new_handle() -> RunQuerierHandle {
    Arc::new(OnceLock::new())
}
