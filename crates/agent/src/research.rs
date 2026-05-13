//! Research mode prompts — re-exported from `tools::research`.
//!
//! All research types, filesystem ops, and prompts live in the tools crate
//! so bot_tool can use them directly. This module re-exports for convenience.

pub use tools::research::{CITATION_PROMPT, RESEARCH_LEAD_PROMPT, RESEARCH_WORKER_PROMPT};
