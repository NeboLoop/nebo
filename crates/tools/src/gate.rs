//! The registry's one chokepoint: every call a tool runs is decided by the
//! permission gate first. `tools` defines the interface; the harness's
//! permission check (`agent::harness::permissions::Check`) is the one
//! implementation, so no door can run a tool without it.

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// A call as it will run: the tool that runs it, the settled input, and the
/// target the tool's spec resolves it to.
pub struct ResolvedCall<'a> {
    pub tool: &'a dyn DynTool,
    pub input: &'a serde_json::Value,
    pub target: types::permissions::Target,
}

impl ResolvedCall<'_> {
    /// The registered name of the tool that runs the call.
    pub fn name(&self) -> &str {
        self.tool.name()
    }
}

/// What the gate decided for one call.
#[derive(Debug, Clone)]
pub enum GateVerdict {
    /// Run it; why is recorded.
    Run(types::permissions::Why),
    /// Don't run it; the result tells the model why in plain words.
    Refuse(ToolResult),
    /// Don't run it now: it waits for the owner. The result says so.
    Parked(ToolResult),
}

#[async_trait::async_trait]
pub trait PermissionGate: Send + Sync {
    async fn check(&self, ctx: &ToolContext, call: &ResolvedCall<'_>) -> GateVerdict;

    /// A call the gate let run has succeeded: what it created is now the
    /// employee's own work. Default: nothing to keep.
    async fn ran(&self, _ctx: &ToolContext, _call: &ResolvedCall<'_>, _result: &ToolResult) {}
}

/// Tests of the tools themselves run every call: the permission check is
/// the harness's, and its own tests drive the registry through it.
#[cfg(test)]
pub(crate) fn test_gate() -> std::sync::Arc<dyn PermissionGate> {
    struct RunEverything;
    #[async_trait::async_trait]
    impl PermissionGate for RunEverything {
        async fn check(&self, _ctx: &ToolContext, _call: &ResolvedCall<'_>) -> GateVerdict {
            GateVerdict::Run(types::permissions::Why::BasicWork)
        }
    }
    std::sync::Arc::new(RunEverything)
}
