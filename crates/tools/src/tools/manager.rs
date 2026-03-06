use std::pin::Pin;
use std::future::Future;

use serde::{Deserialize, Serialize};

use crate::registry::ToolResult;
use crate::origin::ToolContext;

/// Info about an installed .napp tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NappToolInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub provides: Vec<String>,
    pub running: bool,
    pub sideloaded: bool,
}

/// Trait for managing installed .napp tools and dispatching calls to them.
///
/// Defined in tools crate, implemented in server crate (same pattern as
/// AdvisorDeliberator and HybridSearcher).
pub trait NappManager: Send + Sync {
    /// List all installed tools.
    fn list(&self) -> Pin<Box<dyn Future<Output = Vec<NappToolInfo>> + Send + '_>>;

    /// Install a tool from a marketplace code (TOOL-XXXX-XXXX).
    fn install<'a>(&'a self, code: &'a str) -> Pin<Box<dyn Future<Output = Result<NappToolInfo, String>> + Send + 'a>>;

    /// Uninstall a tool by ID.
    fn uninstall<'a>(&'a self, tool_id: &'a str) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

    /// Sideload a tool from a local development directory.
    fn sideload<'a>(&'a self, path: &'a str) -> Pin<Box<dyn Future<Output = Result<NappToolInfo, String>> + Send + 'a>>;

    /// Dispatch a call to an installed tool.
    fn dispatch<'a>(
        &'a self,
        tool_name: &'a str,
        input: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>>;

    /// Get the names of all available dispatchable tools.
    fn tool_names(&self) -> Pin<Box<dyn Future<Output = Vec<String>> + Send + '_>>;
}
