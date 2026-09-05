pub mod advisors;
pub mod agent_worker;
pub mod chunking;
pub mod compaction;
pub mod concurrency;
pub mod db_context;
pub mod decompose;
pub mod dedupe;
pub mod memory_consolidation;
pub mod fuzzy;
pub mod goals;
pub mod guardrails;
pub mod hooks;
pub mod lanes;
pub mod large_input;
pub mod memory;
pub mod provenance;

/// Approximate chars per token — the ONE token-estimate heuristic (was
/// defined identically in pruning, memory_flush, and large_input; a budget
/// change had to land three times).
pub const CHARS_PER_TOKEN: usize = 4;
pub mod memory_debounce;
pub mod memory_flush;
pub mod orchestrator;
pub mod workflow_loop;
pub mod personality;
pub mod review_fork;
pub mod proactive;
pub mod prompt;
pub mod pruning;
pub mod read_ledger;
pub mod worktree;
pub mod shell_hooks;
pub mod research;
pub mod runner;
pub mod sanitize;
pub mod search;
pub mod search_adapter;
pub mod selector;
pub mod session;
pub mod sidecar;
pub mod steering;
pub mod structured;
pub mod structured_agent;
pub mod summarizer;
pub mod reviewer;
pub mod task_graph;
pub mod tool_filter;
pub mod testing;
pub mod transcript;

// Link the BLAS provider for turbovec's ndarray backend: blas-src's contract
// requires a crate in the graph to reference it, or rustc drops the rlib and
// its `-framework Accelerate` directive, leaving cblas_* undefined at link
// time in artifacts (like test binaries) that don't pull Accelerate elsewhere.
#[cfg(target_os = "macos")]
use blas_src as _;

pub use agent_worker::{AgentWorkerRegistry, ChannelDispatcher};
pub use concurrency::ConcurrencyController;
pub use lanes::LaneManager;
pub use orchestrator::Orchestrator;
pub use proactive::{PresenceTracker, ProactiveInbox};
pub use runner::{ChatTitleSink, RunProgress, RunRequest, Runner};
pub use selector::ModelSelector;
pub use session::SessionManager;
