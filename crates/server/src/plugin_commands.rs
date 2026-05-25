//! Plugin slash command dispatch.
//!
//! Intercepts `/command args` prompts in chat, matches against plugin manifests
//! with `capabilities.commands[].slash = true`, and executes the plugin binary
//! directly — bypassing the LLM.

use std::sync::Arc;

use tracing::{info, warn};

use crate::state::AppState;

/// Try to dispatch a slash command to a plugin.
///
/// Returns `Some(output)` if a matching plugin command was found and executed,
/// `None` if no plugin claims this command (caller should fall through to agent).
pub async fn try_dispatch(state: &AppState, prompt: &str, session_id: &str) -> Option<String> {
    let cmd_name = prompt.split_whitespace().next().unwrap_or("");
    let args = prompt[cmd_name.len()..].trim();

    // Search installed plugin manifests for a matching slash command.
    let installed = state.plugin_store.list_installed();
    let mut seen = std::collections::HashSet::new();

    for (slug, _version, _path, _source) in &installed {
        if !seen.insert(slug.clone()) {
            continue;
        }
        let manifest = match state.plugin_store.get_manifest(slug) {
            Some(m) => m,
            None => continue,
        };
        let caps = match &manifest.capabilities {
            Some(c) => c,
            None => continue,
        };
        for cmd_def in &caps.commands {
            if !cmd_def.slash {
                continue;
            }
            // Match: "/gmail" == "/gmail" or "gmail" matches "/gmail"
            let matches = cmd_def.name == cmd_name || format!("/{}", cmd_def.name) == cmd_name;
            if !matches {
                continue;
            }

            let binary = match state.plugin_store.resolve(slug, "*") {
                Some(p) => p,
                None => {
                    warn!(plugin = %slug, command = %cmd_name, "plugin binary not found");
                    return Some(format!("Plugin '{}' binary not found", slug));
                }
            };

            info!(plugin = %slug, command = %cmd_name, args = %args, session_id, "executing plugin slash command");

            return Some(execute(&binary, &cmd_def.command, args, slug, state.plugin_store.clone()).await);
        }
    }

    None
}

/// Execute a plugin binary with the given subcommand and args.
async fn execute(
    binary: &std::path::Path,
    command: &str,
    args: &str,
    slug: &str,
    plugin_store: Arc<napp::plugin::PluginStore>,
) -> String {
    let full_command = if args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, args)
    };

    let runtime = napp::PluginRuntime::new(slug, binary.to_path_buf(), plugin_store)
        .with_permissions();
    let timeout = runtime.effective_timeout(std::time::Duration::from_secs(30));
    let mut cmd = runtime.command(&full_command);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            warn!(plugin = %slug, error = %e, "slash command spawn failed");
            return format!("Failed to run plugin command: {}", e);
        }
    };

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if output.status.success() {
                if stdout.trim().is_empty() {
                    "Command completed.".to_string()
                } else {
                    stdout.trim().to_string()
                }
            } else {
                let err = if stderr.trim().is_empty() {
                    stdout.to_string()
                } else {
                    stderr.to_string()
                };
                format!("Command failed: {}", err.trim())
            }
        }
        Ok(Err(e)) => format!("Command error: {}", e),
        Err(_) => format!("Command timed out after 30s"),
    }
}
