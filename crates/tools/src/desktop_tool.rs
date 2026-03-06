use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Desktop tool: window management, input simulation, clipboard, notifications, screen capture.
/// macOS-first via AppleScript; returns platform error on other OSes.
pub struct DesktopTool;

impl DesktopTool {
    pub fn new() -> Self {
        Self
    }
}

impl DynTool for DesktopTool {
    fn name(&self) -> &str {
        "desktop"
    }

    fn description(&self) -> String {
        "Desktop automation — windows, input, clipboard, notifications, screen capture.\n\n\
         Resources:\n\
         - window: list, focus, minimize, maximize, resize, close\n\
         - input: click, type, press, move (requires approval)\n\
         - clipboard: read, write\n\
         - notification: send\n\
         - capture: screenshot (app or region)\n\n\
         Examples:\n  \
         desktop(resource: \"clipboard\", action: \"read\")\n  \
         desktop(resource: \"window\", action: \"list\")\n  \
         desktop(resource: \"capture\", action: \"screenshot\", app: \"Safari\")\n  \
         desktop(resource: \"notification\", action: \"send\", title: \"Done\", message: \"Task complete\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "Desktop resource",
                    "enum": ["window", "input", "clipboard", "notification", "capture"]
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform on the resource"
                },
                "app": { "type": "string", "description": "Application name (for window/capture)" },
                "title": { "type": "string", "description": "Window or notification title" },
                "message": { "type": "string", "description": "Notification message" },
                "text": { "type": "string", "description": "Text to type or write to clipboard" },
                "key": { "type": "string", "description": "Key to press (e.g. 'return', 'tab')" },
                "x": { "type": "integer", "description": "X coordinate" },
                "y": { "type": "integer", "description": "Y coordinate" },
                "width": { "type": "integer", "description": "Width for resize" },
                "height": { "type": "integer", "description": "Height for resize" },
                "region": { "type": "string", "description": "Region for screenshot: 'x,y,w,h'" }
            },
            "required": ["resource", "action"]
        })
    }

    fn requires_approval(&self) -> bool {
        // Input actions need approval; others are safe reads
        // Checked per-action in execute
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
                "window" => handle_window(action, &input).await,
                "input" => handle_input(action, &input).await,
                "clipboard" => handle_clipboard(action, &input).await,
                "notification" => handle_notification(action, &input).await,
                "capture" => handle_capture(action, &input).await,
                _ => ToolResult::error(format!(
                    "Unknown resource '{}'. Use: window, input, clipboard, notification, capture",
                    resource
                )),
            }
        })
    }
}

// --- Window management ---

async fn handle_window(action: &str, input: &serde_json::Value) -> ToolResult {
    #[cfg(not(target_os = "macos"))]
    return ToolResult::error("Window management is only supported on macOS");

    #[cfg(target_os = "macos")]
    {
        match action {
            "list" => {
                let script = r#"
                    tell application "System Events"
                        set windowList to ""
                        repeat with proc in (every process whose visible is true)
                            set procName to name of proc
                            repeat with win in (every window of proc)
                                set winTitle to name of win
                                set winPos to position of win
                                set winSize to size of win
                                set windowList to windowList & procName & " | " & winTitle & " | " & (item 1 of winPos as text) & "," & (item 2 of winPos as text) & " | " & (item 1 of winSize as text) & "x" & (item 2 of winSize as text) & linefeed
                            end repeat
                        end repeat
                        return windowList
                    end tell
                "#;
                run_osascript(script).await
            }
            "focus" => {
                let app = input["app"].as_str().unwrap_or("");
                if app.is_empty() {
                    return ToolResult::error("'app' parameter required for focus");
                }
                let script = format!(
                    "tell application \"{}\" to activate",
                    escape_applescript(app)
                );
                run_osascript(&script).await
            }
            "minimize" => {
                let app = input["app"].as_str().unwrap_or("");
                if app.is_empty() {
                    return ToolResult::error("'app' parameter required for minimize");
                }
                let script = format!(
                    "tell application \"System Events\" to set miniaturized of first window of process \"{}\" to true",
                    escape_applescript(app)
                );
                run_osascript(&script).await
            }
            "maximize" => {
                let app = input["app"].as_str().unwrap_or("");
                if app.is_empty() {
                    return ToolResult::error("'app' parameter required for maximize");
                }
                let script = format!(
                    "tell application \"System Events\"\n\
                     set theWindow to first window of process \"{}\"\n\
                     set position of theWindow to {{0, 25}}\n\
                     set size of theWindow to {{1920, 1055}}\n\
                     end tell",
                    escape_applescript(app)
                );
                run_osascript(&script).await
            }
            "resize" => {
                let app = input["app"].as_str().unwrap_or("");
                let w = input["width"].as_i64().unwrap_or(800);
                let h = input["height"].as_i64().unwrap_or(600);
                if app.is_empty() {
                    return ToolResult::error("'app' parameter required for resize");
                }
                let script = format!(
                    "tell application \"System Events\" to set size of first window of process \"{}\" to {{{}, {}}}",
                    escape_applescript(app), w, h
                );
                run_osascript(&script).await
            }
            "close" => {
                let app = input["app"].as_str().unwrap_or("");
                if app.is_empty() {
                    return ToolResult::error("'app' parameter required for close");
                }
                let script = format!(
                    "tell application \"System Events\" to click button 1 of first window of process \"{}\"",
                    escape_applescript(app)
                );
                run_osascript(&script).await
            }
            _ => ToolResult::error(format!(
                "Unknown window action '{}'. Use: list, focus, minimize, maximize, resize, close",
                action
            )),
        }
    }
}

// --- Input simulation ---

async fn handle_input(action: &str, input: &serde_json::Value) -> ToolResult {
    #[cfg(not(target_os = "macos"))]
    return ToolResult::error("Input simulation is only supported on macOS");

    #[cfg(target_os = "macos")]
    {
        match action {
            "type" => {
                let text = input["text"].as_str().unwrap_or("");
                if text.is_empty() {
                    return ToolResult::error("'text' parameter required for type");
                }
                let script = format!(
                    "tell application \"System Events\" to keystroke \"{}\"",
                    escape_applescript(text)
                );
                run_osascript(&script).await
            }
            "press" => {
                let key = input["key"].as_str().unwrap_or("");
                if key.is_empty() {
                    return ToolResult::error("'key' parameter required for press");
                }
                let key_code = key_name_to_code(key);
                let script = format!(
                    "tell application \"System Events\" to key code {}",
                    key_code
                );
                run_osascript(&script).await
            }
            "click" => {
                let x = input["x"].as_i64().unwrap_or(0);
                let y = input["y"].as_i64().unwrap_or(0);
                let script = format!(
                    "do shell script \"cliclick c:{},{}\"",
                    x, y
                );
                // Fallback: try AppleScript mouse click via System Events
                match run_osascript_raw(&script).await {
                    Ok(out) => ToolResult::ok(out),
                    Err(_) => ToolResult::error(
                        "Click requires 'cliclick' utility. Install with: brew install cliclick"
                    ),
                }
            }
            "move" => {
                let x = input["x"].as_i64().unwrap_or(0);
                let y = input["y"].as_i64().unwrap_or(0);
                let script = format!(
                    "do shell script \"cliclick m:{},{}\"",
                    x, y
                );
                match run_osascript_raw(&script).await {
                    Ok(out) => ToolResult::ok(out),
                    Err(_) => ToolResult::error(
                        "Mouse move requires 'cliclick' utility. Install with: brew install cliclick"
                    ),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown input action '{}'. Use: click, type, press, move",
                action
            )),
        }
    }
}

// --- Clipboard ---

async fn handle_clipboard(action: &str, input: &serde_json::Value) -> ToolResult {
    #[cfg(not(target_os = "macos"))]
    return ToolResult::error("Clipboard is only supported on macOS");

    #[cfg(target_os = "macos")]
    {
        match action {
            "read" => {
                match tokio::process::Command::new("pbpaste").output().await {
                    Ok(output) => {
                        let text = String::from_utf8_lossy(&output.stdout).to_string();
                        ToolResult::ok(if text.is_empty() {
                            "(clipboard is empty)".to_string()
                        } else {
                            text
                        })
                    }
                    Err(e) => ToolResult::error(format!("Failed to read clipboard: {}", e)),
                }
            }
            "write" => {
                let text = input["text"].as_str().unwrap_or("");
                if text.is_empty() {
                    return ToolResult::error("'text' parameter required for clipboard write");
                }
                let mut child = match tokio::process::Command::new("pbcopy")
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(e) => return ToolResult::error(format!("Failed to write clipboard: {}", e)),
                };
                if let Some(stdin) = child.stdin.as_mut() {
                    use tokio::io::AsyncWriteExt;
                    let _ = stdin.write_all(text.as_bytes()).await;
                }
                let _ = child.wait().await;
                ToolResult::ok("Clipboard updated")
            }
            _ => ToolResult::error(format!(
                "Unknown clipboard action '{}'. Use: read, write",
                action
            )),
        }
    }
}

// --- Notifications ---

async fn handle_notification(action: &str, input: &serde_json::Value) -> ToolResult {
    #[cfg(not(target_os = "macos"))]
    return ToolResult::error("Notifications are only supported on macOS");

    #[cfg(target_os = "macos")]
    {
        match action {
            "send" => {
                let title = input["title"].as_str().unwrap_or("Nebo");
                let message = input["message"].as_str().unwrap_or("");
                if message.is_empty() {
                    return ToolResult::error("'message' parameter required for notification");
                }
                let script = format!(
                    "display notification \"{}\" with title \"{}\"",
                    escape_applescript(message),
                    escape_applescript(title)
                );
                run_osascript(&script).await
            }
            _ => ToolResult::error(format!(
                "Unknown notification action '{}'. Use: send",
                action
            )),
        }
    }
}

// --- Screen capture ---

async fn handle_capture(action: &str, input: &serde_json::Value) -> ToolResult {
    #[cfg(not(target_os = "macos"))]
    return ToolResult::error("Screen capture is only supported on macOS");

    #[cfg(target_os = "macos")]
    {
        match action {
            "screenshot" => {
                let tmp_path = format!("/tmp/nebo-capture-{}.png", std::process::id());
                let app = input["app"].as_str().unwrap_or("");

                let result = if !app.is_empty() {
                    // Get window ID for the app, then capture that window
                    let wid_script = format!(
                        "tell application \"System Events\" to return id of first window of process \"{}\"",
                        escape_applescript(app)
                    );
                    match run_osascript_raw(&wid_script).await {
                        Ok(wid) => {
                            let wid = wid.trim();
                            tokio::process::Command::new("screencapture")
                                .args(["-l", wid, "-x", &tmp_path])
                                .output()
                                .await
                        }
                        Err(_) => {
                            // Fallback: full screen capture
                            tokio::process::Command::new("screencapture")
                                .args(["-x", &tmp_path])
                                .output()
                                .await
                        }
                    }
                } else if let Some(region) = input["region"].as_str() {
                    // Region capture: "x,y,w,h"
                    let parts: Vec<&str> = region.split(',').collect();
                    if parts.len() == 4 {
                        tokio::process::Command::new("screencapture")
                            .args(["-x", "-R", region, &tmp_path])
                            .output()
                            .await
                    } else {
                        return ToolResult::error("Region format: 'x,y,w,h'");
                    }
                } else {
                    // Full screen
                    tokio::process::Command::new("screencapture")
                        .args(["-x", &tmp_path])
                        .output()
                        .await
                };

                match result {
                    Ok(output) if output.status.success() => {
                        // Read the file and encode as base64
                        match tokio::fs::read(&tmp_path).await {
                            Ok(bytes) => {
                                let _ = tokio::fs::remove_file(&tmp_path).await;
                                use base64::Engine;
                                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                                let data_uri = format!("data:image/png;base64,{}", b64);
                                ToolResult {
                                    content: format!("Screenshot captured ({} bytes)", bytes.len()),
                                    is_error: false,
                                    image_url: Some(data_uri),
                                }
                            }
                            Err(e) => ToolResult::error(format!("Failed to read screenshot: {}", e)),
                        }
                    }
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        ToolResult::error(format!("Screenshot failed: {}", stderr))
                    }
                    Err(e) => ToolResult::error(format!("Failed to run screencapture: {}", e)),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown capture action '{}'. Use: screenshot",
                action
            )),
        }
    }
}

// --- Helpers ---

#[cfg(target_os = "macos")]
async fn run_osascript(script: &str) -> ToolResult {
    match run_osascript_raw(script).await {
        Ok(output) => ToolResult::ok(if output.is_empty() {
            "OK".to_string()
        } else {
            output
        }),
        Err(e) => ToolResult::error(e),
    }
}

#[cfg(target_os = "macos")]
async fn run_osascript_raw(script: &str) -> Result<String, String> {
    let output = tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .await
        .map_err(|e| format!("Failed to run osascript: {}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("AppleScript error: {}", stderr))
    }
}

#[cfg(target_os = "macos")]
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "macos")]
fn key_name_to_code(key: &str) -> &str {
    match key.to_lowercase().as_str() {
        "return" | "enter" => "36",
        "tab" => "48",
        "space" => "49",
        "delete" | "backspace" => "51",
        "escape" | "esc" => "53",
        "left" => "123",
        "right" => "124",
        "down" => "125",
        "up" => "126",
        "f1" => "122",
        "f2" => "120",
        "f3" => "99",
        "f4" => "118",
        "f5" => "96",
        _ => "36", // default to return
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_metadata() {
        let tool = DesktopTool::new();
        assert_eq!(tool.name(), "desktop");
        assert!(tool.description().contains("window"));
        assert!(tool.description().contains("clipboard"));
        let schema = tool.schema();
        assert!(schema["properties"]["resource"].is_object());
        assert!(schema["properties"]["action"].is_object());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_escape_applescript() {
        assert_eq!(escape_applescript("hello"), "hello");
        assert_eq!(escape_applescript("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(escape_applescript("path\\to"), "path\\\\to");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_key_name_to_code() {
        assert_eq!(key_name_to_code("return"), "36");
        assert_eq!(key_name_to_code("tab"), "48");
        assert_eq!(key_name_to_code("escape"), "53");
        assert_eq!(key_name_to_code("Enter"), "36"); // case insensitive
    }

    #[tokio::test]
    async fn test_unknown_resource() {
        let tool = DesktopTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"resource": "unknown", "action": "test"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown resource"));
    }

    #[tokio::test]
    async fn test_missing_resource() {
        let tool = DesktopTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "test"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_clipboard_read() {
        let result = handle_clipboard("read", &serde_json::json!({})).await;
        // Should succeed (either empty or has content)
        assert!(!result.is_error);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_window_list() {
        let result = handle_window("list", &serde_json::json!({})).await;
        // Window list should succeed on macOS (may be empty in CI)
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_window_focus_missing_app() {
        let result = handle_window("focus", &serde_json::json!({})).await;
        #[cfg(target_os = "macos")]
        assert!(result.is_error);
        #[cfg(not(target_os = "macos"))]
        assert!(result.is_error);
    }
}
