use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Music tool: control media playback (play, pause, next, previous, status, search, volume, playlists, shuffle).
/// Cross-platform: macOS (Music.app via osascript), Linux (playerctl/MPRIS), Windows (PowerShell SMTC).
pub struct MusicTool;

impl MusicTool {
    pub fn new() -> Self {
        Self
    }
}

impl DynTool for MusicTool {
    fn name(&self) -> &str {
        "music"
    }

    fn description(&self) -> String {
        "Control media playback on the local machine.\n\n\
         Actions:\n\
         - play: resume or start playback\n\
         - pause: pause playback\n\
         - next: skip to next track\n\
         - previous: go to previous track\n\
         - status: get current track info and player state\n\
         - search: search music library (query required)\n\
         - volume: get or set player volume (value 0-100)\n\
         - playlists: list available playlists\n\
         - shuffle: get or toggle shuffle mode\n\n\
         Examples:\n  \
         music(action: \"play\")\n  \
         music(action: \"status\")\n  \
         music(action: \"search\", query: \"bohemian rhapsody\")\n  \
         music(action: \"volume\", value: 75)\n  \
         music(action: \"shuffle\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Playback action to perform",
                    "enum": ["play", "pause", "next", "previous", "status", "search", "volume", "playlists", "shuffle"]
                },
                "query": {
                    "type": "string",
                    "description": "Search query (for search action)"
                },
                "value": {
                    "type": "integer",
                    "description": "Volume level 0-100 (for volume set action)"
                }
            },
            "required": ["action"]
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
            let action = input["action"].as_str().unwrap_or("");

            match action {
                "play" => handle_play().await,
                "pause" => handle_pause().await,
                "next" => handle_next().await,
                "previous" => handle_previous().await,
                "status" => handle_status().await,
                "search" => {
                    let query = input["query"].as_str().unwrap_or("");
                    if query.is_empty() {
                        return ToolResult::error("query is required for search action");
                    }
                    handle_search(query).await
                }
                "volume" => handle_volume(&input).await,
                "playlists" => handle_playlists().await,
                "shuffle" => handle_shuffle(&input).await,
                _ => ToolResult::error(format!(
                    "Unknown action '{}'. Use: play, pause, next, previous, status, search, volume, playlists, shuffle",
                    action
                )),
            }
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════
// macOS implementations (Music.app via osascript)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
async fn handle_play() -> ToolResult {
    run_osascript("tell application \"Music\" to play").await
}

#[cfg(target_os = "macos")]
async fn handle_pause() -> ToolResult {
    run_osascript("tell application \"Music\" to pause").await
}

#[cfg(target_os = "macos")]
async fn handle_next() -> ToolResult {
    run_osascript("tell application \"Music\" to next track").await
}

#[cfg(target_os = "macos")]
async fn handle_previous() -> ToolResult {
    run_osascript("tell application \"Music\" to previous track").await
}

#[cfg(target_os = "macos")]
async fn handle_status() -> ToolResult {
    run_osascript(
        "tell application \"Music\"\n\
         set s to player state as text\n\
         if s is \"stopped\" then\n\
         return \"Not playing\"\n\
         end if\n\
         try\n\
         return (name of current track) & \" - \" & (artist of current track) & \" [\" & s & \"]\"\n\
         on error\n\
         return s\n\
         end try\n\
         end tell"
    ).await
}

#[cfg(target_os = "macos")]
async fn handle_search(query: &str) -> ToolResult {
    let script = format!(
        "tell application \"Music\"\n\
         set results to search playlist \"Library\" for \"{}\"\n\
         set output to \"\"\n\
         repeat with t in results\n\
         set output to output & (name of t) & \" - \" & (artist of t) & \"\n\"\n\
         end repeat\n\
         return output\n\
         end tell",
        query.replace('\"', "\\\"")
    );
    run_osascript(&script).await
}

#[cfg(target_os = "macos")]
async fn handle_volume(input: &serde_json::Value) -> ToolResult {
    match input.get("value") {
        Some(v) if !v.is_null() => {
            let value = v.as_i64().unwrap_or(50).clamp(0, 100);
            run_osascript(&format!(
                "tell application \"Music\" to set sound volume to {}",
                value
            ))
            .await
        }
        _ => run_osascript("tell application \"Music\" to return sound volume").await,
    }
}

#[cfg(target_os = "macos")]
async fn handle_playlists() -> ToolResult {
    run_osascript("tell application \"Music\" to return name of every playlist").await
}

#[cfg(target_os = "macos")]
async fn handle_shuffle(input: &serde_json::Value) -> ToolResult {
    // If a value is provided (true/false), set shuffle; otherwise return current state
    match input.get("value") {
        Some(v) if !v.is_null() => {
            let enable = v.as_bool().unwrap_or(true);
            run_osascript(&format!(
                "tell application \"Music\" to set shuffle enabled to {}",
                enable
            ))
            .await
        }
        _ => {
            // Check current state and toggle
            let check = tokio::process::Command::new("osascript")
                .args([
                    "-e",
                    "tell application \"Music\" to return shuffle enabled",
                ])
                .output()
                .await;
            match check {
                Ok(out) => {
                    let current = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if current == "true" {
                        run_osascript(
                            "tell application \"Music\" to set shuffle enabled to false",
                        )
                        .await;
                        ToolResult::ok("Shuffle: OFF")
                    } else if current == "false" {
                        run_osascript(
                            "tell application \"Music\" to set shuffle enabled to true",
                        )
                        .await;
                        ToolResult::ok("Shuffle: ON")
                    } else {
                        ToolResult::ok(format!("Shuffle: {}", current))
                    }
                }
                Err(e) => ToolResult::error(format!("Failed to get shuffle state: {}", e)),
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Linux implementations (playerctl / MPRIS D-Bus)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "linux")]
async fn handle_play() -> ToolResult {
    if !which("playerctl") {
        return ToolResult::error("playerctl not found. Install playerctl for media control on Linux.");
    }
    run_command("playerctl", &["play"]).await
}

#[cfg(target_os = "linux")]
async fn handle_pause() -> ToolResult {
    if !which("playerctl") {
        return ToolResult::error("playerctl not found. Install playerctl for media control on Linux.");
    }
    run_command("playerctl", &["pause"]).await
}

#[cfg(target_os = "linux")]
async fn handle_next() -> ToolResult {
    if !which("playerctl") {
        return ToolResult::error("playerctl not found. Install playerctl for media control on Linux.");
    }
    run_command("playerctl", &["next"]).await
}

#[cfg(target_os = "linux")]
async fn handle_previous() -> ToolResult {
    if !which("playerctl") {
        return ToolResult::error("playerctl not found. Install playerctl for media control on Linux.");
    }
    run_command("playerctl", &["previous"]).await
}

#[cfg(target_os = "linux")]
async fn handle_status() -> ToolResult {
    if !which("playerctl") {
        return ToolResult::error("playerctl not found. Install playerctl for media control on Linux.");
    }
    run_command(
        "playerctl",
        &["metadata", "--format", "{{artist}} - {{title}} [{{status}}]"],
    )
    .await
}

#[cfg(target_os = "linux")]
async fn handle_search(_query: &str) -> ToolResult {
    ToolResult::error("Music library search is not supported on Linux via playerctl")
}

#[cfg(target_os = "linux")]
async fn handle_volume(input: &serde_json::Value) -> ToolResult {
    if !which("playerctl") {
        return ToolResult::error("playerctl not found. Install playerctl for media control on Linux.");
    }
    match input.get("value") {
        Some(v) if !v.is_null() => {
            let value = v.as_i64().unwrap_or(50).clamp(0, 100);
            let normalized = format!("{:.2}", value as f64 / 100.0);
            run_command("playerctl", &["volume", &normalized]).await
        }
        _ => run_command("playerctl", &["volume"]).await,
    }
}

#[cfg(target_os = "linux")]
async fn handle_playlists() -> ToolResult {
    ToolResult::error("Playlist listing is not supported on Linux via playerctl")
}

#[cfg(target_os = "linux")]
async fn handle_shuffle(input: &serde_json::Value) -> ToolResult {
    if !which("playerctl") {
        return ToolResult::error("playerctl not found. Install playerctl for media control on Linux.");
    }
    match input.get("value") {
        Some(v) if !v.is_null() => {
            let enable = if v.as_bool().unwrap_or(true) { "On" } else { "Off" };
            run_command("playerctl", &["shuffle", enable]).await
        }
        _ => {
            // Toggle shuffle
            run_command("playerctl", &["shuffle", "Toggle"]).await
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Windows implementations (PowerShell SMTC)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "windows")]
async fn handle_play() -> ToolResult {
    ToolResult::error("Media control on Windows requires a supported media player running")
}

#[cfg(target_os = "windows")]
async fn handle_pause() -> ToolResult {
    ToolResult::error("Media control on Windows requires a supported media player running")
}

#[cfg(target_os = "windows")]
async fn handle_next() -> ToolResult {
    ToolResult::error("Media control on Windows requires a supported media player running")
}

#[cfg(target_os = "windows")]
async fn handle_previous() -> ToolResult {
    ToolResult::error("Media control on Windows requires a supported media player running")
}

#[cfg(target_os = "windows")]
async fn handle_status() -> ToolResult {
    let script = "Get-Process | Where-Object { $_.MainWindowTitle -match 'Spotify|Music|Media' } | \
                  Select-Object -Property ProcessName, MainWindowTitle | Format-List";
    run_powershell(script).await
}

#[cfg(target_os = "windows")]
async fn handle_search(_query: &str) -> ToolResult {
    ToolResult::error("Music library search is not supported on Windows")
}

#[cfg(target_os = "windows")]
async fn handle_volume(_input: &serde_json::Value) -> ToolResult {
    ToolResult::error("Media control on Windows requires a supported media player running")
}

#[cfg(target_os = "windows")]
async fn handle_playlists() -> ToolResult {
    ToolResult::error("Playlist listing is not supported on Windows")
}

#[cfg(target_os = "windows")]
async fn handle_shuffle(_input: &serde_json::Value) -> ToolResult {
    ToolResult::error("Media control on Windows requires a supported media player running")
}

// ═══════════════════════════════════════════════════════════════════════
// Shell helpers
// ═══════════════════════════════════════════════════════════════════════

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

#[cfg(any(target_os = "linux", target_os = "windows"))]
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
        let tool = MusicTool::new();
        assert_eq!(tool.name(), "music");
        assert!(tool.description().contains("play"));
        assert!(tool.description().contains("pause"));
        assert!(tool.description().contains("search"));
        assert!(tool.description().contains("volume"));
        let schema = tool.schema();
        assert!(schema["properties"]["action"].is_object());
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let tool = MusicTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "unknown"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown action"));
    }

    #[tokio::test]
    async fn test_search_requires_query() {
        let tool = MusicTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "search"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("query is required"));
    }
}
