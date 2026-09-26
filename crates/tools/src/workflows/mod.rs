mod manager;
pub(crate) mod work_tool;

pub use manager::{Lifetime, SaveOptions, WorkflowInfo, WorkflowManager, WorkflowRunInfo};
pub use work_tool::{WorkflowTool, tools};
#[cfg(test)]
pub(crate) use work_tool::tests::Recorder as TestManager;

/// The workflow manager, filled once the server has built it (the tools
/// are registered before it exists): shared by the workflow tools'
/// registration and `stop_task`, which stops workflow runs.
pub type WorkflowManagerCell = std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<dyn WorkflowManager>>>>;
