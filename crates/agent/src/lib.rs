pub mod advisors;
pub mod agent_worker;
pub mod chunking;
pub mod concurrency;
pub mod db_context;
pub mod dedupe;
pub mod memory_consolidation;
pub mod fuzzy;
pub mod guardrails;
pub mod harness;
pub mod heartbeat_triage;
pub mod hooks;
pub mod lanes;
pub mod large_input;
pub mod memory;

/// Approximate chars per token — the ONE token-estimate heuristic (was
/// defined identically in pruning, memory_flush, and large_input; a budget
/// change had to land three times).
pub const CHARS_PER_TOKEN: usize = 4;
pub mod memory_flush;
pub mod personality;
pub mod review_fork;
pub mod proactive;
pub mod prompt;
pub mod pruning;
pub mod worktree;
pub mod shell_hooks;
pub mod research;
pub mod sanitize;
pub mod search;
pub mod search_adapter;
pub mod selector;
pub mod session;
pub mod uploads;
#[cfg(test)]
mod test_home;
pub mod sidecar;
pub mod structured;
pub mod structured_agent;
pub mod summarizer;
pub mod tool_credentials;
pub mod testing;
pub mod transcript;

// Link the BLAS provider for turbovec's ndarray backend: blas-src's contract
// requires a crate in the graph to reference it, or rustc drops the rlib and
// its `-framework Accelerate` directive, leaving cblas_* undefined at link
// time in artifacts (like test binaries) that don't pull Accelerate elsewhere.
#[cfg(target_os = "macos")]
use blas_src as _;

pub use agent_worker::{AgentWorkerRegistry, ChannelDispatcher, running_watchers};
pub use concurrency::ConcurrencyController;
pub use lanes::LaneManager;
pub use proactive::PresenceTracker;
pub use tool_credentials::{RunGrant, ToolCredentials};
pub use harness::permissions::{migrate::migrate_legacy, resolve_grant, Check};
pub use harness::after_turn::ChatTitleSink;
pub use harness::session_gate::RunProgress;
pub use harness::{Harness, Outlets, TurnRequest};
pub use selector::ModelSelector;
pub use session::SessionManager;
