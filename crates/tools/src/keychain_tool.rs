use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Keychain tool: cross-platform credential storage management.
/// macOS (security), Linux (secret-tool / libsecret), Windows (cmdkey).
pub struct KeychainTool;

impl KeychainTool {
    pub fn new() -> Self {
        Self
    }
}

impl DynTool for KeychainTool {
    fn name(&self) -> &str {
        "keychain"
    }

    fn description(&self) -> String {
        "Manage credential storage (keychain / secret service / credential manager).\n\n\
         Actions:\n\
         - get: retrieve a stored password by service + account\n\
         - find: search for credentials by label\n\
         - add: store a credential (service + account + password)\n\
         - delete: remove a stored credential by service + account\n\n\
         Examples:\n  \
         keychain(action: \"get\", service: \"myapp\", account: \"user@example.com\")\n  \
         keychain(action: \"find\", label: \"myapp\")\n  \
         keychain(action: \"add\", service: \"myapp\", account: \"user@example.com\", password: \"secret\")\n  \
         keychain(action: \"delete\", service: \"myapp\", account: \"user@example.com\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["get", "find", "add", "delete"]
                },
                "service": {
                    "type": "string",
                    "description": "Service or target name"
                },
                "account": {
                    "type": "string",
                    "description": "Account or username"
                },
                "password": {
                    "type": "string",
                    "description": "Password to store (for add action)"
                },
                "label": {
                    "type": "string",
                    "description": "Label to search for (for find action)"
                }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let action = input["action"].as_str().unwrap_or("");

            match action {
                "get" => handle_get(&input).await,
                "find" => handle_find(&input).await,
                "add" => handle_add(&input).await,
                "delete" => handle_delete(&input).await,
                _ => ToolResult::error(format!(
                    "Unknown action '{}'. Use: get, find, add, delete",
                    action
                )),
            }
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════
// macOS implementations (security command)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
async fn handle_get(input: &serde_json::Value) -> ToolResult {
    let service = match input["service"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => return ToolResult::error("'service' is required for get action"),
    };
    let account = match input["account"].as_str() {
        Some(a) if !a.is_empty() => a,
        _ => return ToolResult::error("'account' is required for get action"),
    };
    run_command("security", &["find-generic-password", "-s", service, "-a", account, "-w"]).await
}

#[cfg(target_os = "macos")]
async fn handle_find(input: &serde_json::Value) -> ToolResult {
    let label = match input["label"].as_str() {
        Some(l) if !l.is_empty() => l,
        _ => return ToolResult::error("'label' is required for find action"),
    };
    run_command("security", &["find-generic-password", "-l", label]).await
}

#[cfg(target_os = "macos")]
async fn handle_add(input: &serde_json::Value) -> ToolResult {
    let service = match input["service"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => return ToolResult::error("'service' is required for add action"),
    };
    let account = match input["account"].as_str() {
        Some(a) if !a.is_empty() => a,
        _ => return ToolResult::error("'account' is required for add action"),
    };
    let password = match input["password"].as_str() {
        Some(p) if !p.is_empty() => p,
        _ => return ToolResult::error("'password' is required for add action"),
    };
    run_command(
        "security",
        &["add-generic-password", "-s", service, "-a", account, "-w", password, "-U"],
    )
    .await
}

#[cfg(target_os = "macos")]
async fn handle_delete(input: &serde_json::Value) -> ToolResult {
    let service = match input["service"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => return ToolResult::error("'service' is required for delete action"),
    };
    let account = match input["account"].as_str() {
        Some(a) if !a.is_empty() => a,
        _ => return ToolResult::error("'account' is required for delete action"),
    };
    run_command("security", &["delete-generic-password", "-s", service, "-a", account]).await
}

// ═══════════════════════════════════════════════════════════════════════
// Linux implementations (secret-tool / libsecret)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "linux")]
async fn handle_get(input: &serde_json::Value) -> ToolResult {
    let service = match input["service"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => return ToolResult::error("'service' is required for get action"),
    };
    let account = match input["account"].as_str() {
        Some(a) if !a.is_empty() => a,
        _ => return ToolResult::error("'account' is required for get action"),
    };
    if !which("secret-tool") {
        return ToolResult::error("secret-tool not found (install libsecret-tools)");
    }
    run_command("secret-tool", &["lookup", "service", service, "account", account]).await
}

#[cfg(target_os = "linux")]
async fn handle_find(input: &serde_json::Value) -> ToolResult {
    let label = match input["label"].as_str() {
        Some(l) if !l.is_empty() => l,
        _ => return ToolResult::error("'label' is required for find action"),
    };
    if !which("secret-tool") {
        return ToolResult::error("secret-tool not found (install libsecret-tools)");
    }
    run_command("secret-tool", &["search", "--all", "label", label]).await
}

#[cfg(target_os = "linux")]
async fn handle_add(input: &serde_json::Value) -> ToolResult {
    let service = match input["service"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => return ToolResult::error("'service' is required for add action"),
    };
    let account = match input["account"].as_str() {
        Some(a) if !a.is_empty() => a,
        _ => return ToolResult::error("'account' is required for add action"),
    };
    let password = match input["password"].as_str() {
        Some(p) if !p.is_empty() => p,
        _ => return ToolResult::error("'password' is required for add action"),
    };
    let label = input["label"].as_str().unwrap_or(service);
    if !which("secret-tool") {
        return ToolResult::error("secret-tool not found (install libsecret-tools)");
    }
    // secret-tool store reads the secret from stdin
    let output = tokio::process::Command::new("sh")
        .args([
            "-c",
            &format!(
                "echo -n '{}' | secret-tool store --label '{}' service '{}' account '{}'",
                password, label, service, account
            ),
        ])
        .output()
        .await;
    match output {
        Ok(out) if out.status.success() => {
            ToolResult::ok(format!("Credential stored for service '{}' account '{}'", service, account))
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            ToolResult::error(format!("Failed to store credential: {}", stderr))
        }
        Err(e) => ToolResult::error(format!("Command failed: {}", e)),
    }
}

#[cfg(target_os = "linux")]
async fn handle_delete(input: &serde_json::Value) -> ToolResult {
    let service = match input["service"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => return ToolResult::error("'service' is required for delete action"),
    };
    let account = match input["account"].as_str() {
        Some(a) if !a.is_empty() => a,
        _ => return ToolResult::error("'account' is required for delete action"),
    };
    if !which("secret-tool") {
        return ToolResult::error("secret-tool not found (install libsecret-tools)");
    }
    run_command("secret-tool", &["clear", "service", service, "account", account]).await
}

// ═══════════════════════════════════════════════════════════════════════
// Windows implementations (cmdkey)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "windows")]
async fn handle_get(input: &serde_json::Value) -> ToolResult {
    let service = match input["service"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => return ToolResult::error("'service' is required for get action"),
    };
    // cmdkey cannot directly retrieve passwords; return metadata instead
    let target = format!("/list:{}", service);
    let result = run_command("cmdkey", &[&target]).await;
    if !result.is_error && result.content == "OK" {
        ToolResult::error(format!("No credential found for target '{}'", service))
    } else {
        result
    }
}

#[cfg(target_os = "windows")]
async fn handle_find(input: &serde_json::Value) -> ToolResult {
    let label = match input["label"].as_str() {
        Some(l) if !l.is_empty() => l,
        _ => return ToolResult::error("'label' is required for find action"),
    };
    // List all credentials and filter by target name
    let output = tokio::process::Command::new("cmdkey")
        .arg("/list")
        .output()
        .await;
    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let filtered: Vec<&str> = text
                .lines()
                .filter(|line| line.to_lowercase().contains(&label.to_lowercase()))
                .collect();
            if filtered.is_empty() {
                ToolResult::error(format!("No credentials found matching '{}'", label))
            } else {
                ToolResult::ok(filtered.join("\n"))
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            ToolResult::error(format!("Failed to list credentials: {}", stderr))
        }
        Err(e) => ToolResult::error(format!("Command failed: {}", e)),
    }
}

#[cfg(target_os = "windows")]
async fn handle_add(input: &serde_json::Value) -> ToolResult {
    let service = match input["service"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => return ToolResult::error("'service' is required for add action"),
    };
    let account = match input["account"].as_str() {
        Some(a) if !a.is_empty() => a,
        _ => return ToolResult::error("'account' is required for add action"),
    };
    let password = match input["password"].as_str() {
        Some(p) if !p.is_empty() => p,
        _ => return ToolResult::error("'password' is required for add action"),
    };
    let target = format!("/add:{}", service);
    let user = format!("/user:{}", account);
    let pass = format!("/pass:{}", password);
    run_command("cmdkey", &[&target, &user, &pass]).await
}

#[cfg(target_os = "windows")]
async fn handle_delete(input: &serde_json::Value) -> ToolResult {
    let service = match input["service"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => return ToolResult::error("'service' is required for delete action"),
    };
    let target = format!("/delete:{}", service);
    run_command("cmdkey", &[&target]).await
}

// ═══════════════════════════════════════════════════════════════════════
// Shell helpers
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
#[allow(dead_code)]
async fn run_osascript(script: &str) -> ToolResult {
    match tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::ok(if text.is_empty() { "OK".to_string() } else { text })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            ToolResult::error(format!("AppleScript error: {}", stderr))
        }
        Err(e) => ToolResult::error(format!("Failed to run osascript: {}", e)),
    }
}

async fn run_command(cmd: &str, args: &[&str]) -> ToolResult {
    match tokio::process::Command::new(cmd).args(args).output().await {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::ok(if text.is_empty() { "OK".to_string() } else { text })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::error(format!(
                "{}{}",
                stdout,
                if stderr.is_empty() { String::new() } else { format!("\n{}", stderr) }
            ))
        }
        Err(e) => ToolResult::error(format!("Command '{}' failed: {}", cmd, e)),
    }
}

#[cfg(target_os = "windows")]
#[allow(dead_code)]
async fn run_powershell(script: &str) -> ToolResult {
    run_command("powershell", &["-NoProfile", "-Command", script]).await
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn which(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_metadata() {
        let tool = KeychainTool::new();
        assert_eq!(tool.name(), "keychain");
        assert!(tool.requires_approval());
        let schema = tool.schema();
        assert!(schema["properties"]["action"].is_object());
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let tool = KeychainTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "unknown"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown action"));
    }
}
