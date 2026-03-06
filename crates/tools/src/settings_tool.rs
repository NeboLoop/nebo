use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Settings tool: system settings like volume, brightness, wifi, bluetooth, battery.
/// macOS-first; returns platform error on other OSes.
pub struct SettingsTool;

impl SettingsTool {
    pub fn new() -> Self {
        Self
    }
}

impl DynTool for SettingsTool {
    fn name(&self) -> &str {
        "settings"
    }

    fn description(&self) -> String {
        "Read and control system settings.\n\n\
         Resources:\n\
         - volume: get, set (value 0-100)\n\
         - brightness: get, set (value 0-100)\n\
         - wifi: status, toggle\n\
         - bluetooth: status, toggle\n\
         - battery: status\n\n\
         Examples:\n  \
         settings(resource: \"volume\", action: \"get\")\n  \
         settings(resource: \"volume\", action: \"set\", value: 50)\n  \
         settings(resource: \"battery\", action: \"status\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "System setting resource",
                    "enum": ["volume", "brightness", "wifi", "bluetooth", "battery"]
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["get", "set", "status", "toggle"]
                },
                "value": {
                    "type": "integer",
                    "description": "Value to set (0-100 for volume/brightness)"
                }
            },
            "required": ["resource", "action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let resource = input["resource"].as_str().unwrap_or("");
            let action = input["action"].as_str().unwrap_or("");

            match resource {
                "volume" => handle_volume(action, &input).await,
                "brightness" => handle_brightness(action, &input).await,
                "wifi" => handle_wifi(action).await,
                "bluetooth" => handle_bluetooth(action).await,
                "battery" => handle_battery(action).await,
                _ => ToolResult::error(format!(
                    "Unknown resource '{}'. Use: volume, brightness, wifi, bluetooth, battery",
                    resource
                )),
            }
        })
    }
}

async fn handle_volume(action: &str, input: &serde_json::Value) -> ToolResult {
    #[cfg(not(target_os = "macos"))]
    return ToolResult::error("Volume control is only supported on macOS");

    #[cfg(target_os = "macos")]
    {
        match action {
            "get" => {
                run_osascript("output volume of (get volume settings)").await
            }
            "set" => {
                let value = input["value"].as_i64().unwrap_or(50);
                let value = value.clamp(0, 100);
                run_osascript(&format!("set volume output volume {}", value)).await
            }
            _ => ToolResult::error(format!("Unknown volume action '{}'. Use: get, set", action)),
        }
    }
}

async fn handle_brightness(action: &str, input: &serde_json::Value) -> ToolResult {
    #[cfg(not(target_os = "macos"))]
    return ToolResult::error("Brightness control is only supported on macOS");

    #[cfg(target_os = "macos")]
    {
        match action {
            "get" => {
                run_command("brightness", &["-l"]).await
            }
            "set" => {
                let value = input["value"].as_i64().unwrap_or(50);
                let normalized = (value.clamp(0, 100) as f64) / 100.0;
                run_command("brightness", &[&format!("{:.2}", normalized)]).await
            }
            _ => ToolResult::error(format!("Unknown brightness action '{}'. Use: get, set", action)),
        }
    }
}

async fn handle_wifi(action: &str) -> ToolResult {
    #[cfg(not(target_os = "macos"))]
    return ToolResult::error("WiFi control is only supported on macOS");

    #[cfg(target_os = "macos")]
    {
        match action {
            "status" => {
                run_command("networksetup", &["-getairportpower", "en0"]).await
            }
            "toggle" => {
                // Check current state, then toggle
                let output = tokio::process::Command::new("networksetup")
                    .args(["-getairportpower", "en0"])
                    .output()
                    .await;
                match output {
                    Ok(out) => {
                        let text = String::from_utf8_lossy(&out.stdout);
                        let new_state = if text.contains("On") { "off" } else { "on" };
                        run_command("networksetup", &["-setairportpower", "en0", new_state]).await
                    }
                    Err(e) => ToolResult::error(format!("Failed to check WiFi: {}", e)),
                }
            }
            _ => ToolResult::error(format!("Unknown wifi action '{}'. Use: status, toggle", action)),
        }
    }
}

async fn handle_bluetooth(action: &str) -> ToolResult {
    #[cfg(not(target_os = "macos"))]
    return ToolResult::error("Bluetooth control is only supported on macOS");

    #[cfg(target_os = "macos")]
    {
        match action {
            "status" => {
                run_command("blueutil", &["--power"]).await
            }
            "toggle" => {
                let output = tokio::process::Command::new("blueutil")
                    .args(["--power"])
                    .output()
                    .await;
                match output {
                    Ok(out) => {
                        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        let new_state = if text == "1" { "0" } else { "1" };
                        run_command("blueutil", &["--power", new_state]).await
                    }
                    Err(e) => ToolResult::error(format!(
                        "blueutil not found. Install with: brew install blueutil\nError: {}", e
                    )),
                }
            }
            _ => ToolResult::error(format!("Unknown bluetooth action '{}'. Use: status, toggle", action)),
        }
    }
}

async fn handle_battery(action: &str) -> ToolResult {
    #[cfg(not(target_os = "macos"))]
    return ToolResult::error("Battery status is only supported on macOS");

    #[cfg(target_os = "macos")]
    {
        match action {
            "status" => {
                run_command("pmset", &["-g", "batt"]).await
            }
            _ => ToolResult::error(format!("Unknown battery action '{}'. Use: status", action)),
        }
    }
}

// --- Helpers ---

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
async fn run_command(cmd: &str, args: &[&str]) -> ToolResult {
    match tokio::process::Command::new(cmd).args(args).output().await {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::ok(if text.is_empty() { "OK".to_string() } else { text })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::error(format!("{}{}", stdout, if stderr.is_empty() { String::new() } else { format!("\n{}", stderr) }))
        }
        Err(e) => ToolResult::error(format!("Command '{}' failed: {}", cmd, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_metadata() {
        let tool = SettingsTool::new();
        assert_eq!(tool.name(), "settings");
        assert!(tool.description().contains("volume"));
        assert!(tool.description().contains("battery"));
        let schema = tool.schema();
        assert!(schema["properties"]["resource"].is_object());
    }

    #[tokio::test]
    async fn test_unknown_resource() {
        let tool = SettingsTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"resource": "unknown", "action": "get"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown resource"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_battery_status() {
        let result = handle_battery("status").await;
        assert!(!result.is_error);
    }
}
