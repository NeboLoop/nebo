//! Audit logging for browser tool requests.
//!
//! Logs tool invocations for security visibility, especially sensitive
//! commands like `evaluate` (Runtime.evaluate) that can execute arbitrary JS.

use tracing::{info, warn};

/// Sensitive tools that get extra logging.
const SENSITIVE_TOOLS: &[&str] = &["evaluate", "screenshot"];

/// Log a tool request. Sensitive tools get warnings.
pub fn log_tool_request(tool: &str, args: &serde_json::Value) {
    if SENSITIVE_TOOLS.contains(&tool) {
        warn!(
            tool = tool,
            args = %args,
            "sensitive browser tool invoked"
        );
    } else {
        info!(tool = tool, "browser tool request");
    }
}
