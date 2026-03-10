use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// App tool: application lifecycle management — list, launch, quit, activate, hide, info.
/// Cross-platform: macOS (AppleScript), Linux (wmctrl/xdotool), Windows (PowerShell).
pub struct AppTool;

impl AppTool {
    pub fn new() -> Self {
        Self
    }
}

impl DynTool for AppTool {
    fn name(&self) -> &str {
        "app"
    }

    fn description(&self) -> String {
        "Manage application lifecycle — list running apps, launch, quit, activate, hide, get info.\n\n\
         Actions:\n\
         - list: list all visible/running applications\n\
         - launch: launch an application by name\n\
         - quit: quit a specific application\n\
         - quit_all: quit all visible applications (except Finder on macOS)\n\
         - activate: bring an application to the foreground\n\
         - hide: hide an application\n\
         - info: get detailed info about an application\n\
         - frontmost: get the name of the frontmost application\n\n\
         Examples:\n  \
         app(action: \"list\")\n  \
         app(action: \"launch\", app: \"Safari\")\n  \
         app(action: \"quit\", app: \"Slack\")\n  \
         app(action: \"activate\", app: \"Terminal\")\n  \
         app(action: \"info\", app: \"Xcode\")\n  \
         app(action: \"frontmost\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["list", "launch", "quit", "quit_all", "activate", "hide", "info", "frontmost"]
                },
                "app": {
                    "type": "string",
                    "description": "Application name (required for launch, quit, activate, hide, info)"
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
            let app = input["app"].as_str().unwrap_or("");

            match action {
                "list" => handle_list().await,
                "launch" => {
                    if app.is_empty() {
                        return ToolResult::error("'app' parameter required for launch");
                    }
                    handle_launch(app).await
                }
                "quit" => {
                    if app.is_empty() {
                        return ToolResult::error("'app' parameter required for quit");
                    }
                    handle_quit(app).await
                }
                "quit_all" => handle_quit_all().await,
                "activate" => {
                    if app.is_empty() {
                        return ToolResult::error("'app' parameter required for activate");
                    }
                    handle_activate(app).await
                }
                "hide" => {
                    if app.is_empty() {
                        return ToolResult::error("'app' parameter required for hide");
                    }
                    handle_hide(app).await
                }
                "info" => {
                    if app.is_empty() {
                        return ToolResult::error("'app' parameter required for info");
                    }
                    handle_info(app).await
                }
                "frontmost" => handle_frontmost().await,
                _ => ToolResult::error(format!(
                    "Unknown action '{}'. Use: list, launch, quit, quit_all, activate, hide, info, frontmost",
                    action
                )),
            }
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════
// macOS implementations (AppleScript via osascript)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
async fn handle_list() -> ToolResult {
    run_osascript(
        "tell application \"System Events\" to get name of every process whose visible is true",
    )
    .await
}

#[cfg(target_os = "macos")]
async fn handle_launch(app: &str) -> ToolResult {
    // Try activate first (works for already-installed apps), fall back to open -a
    let script = format!(
        "try\n\
         \ttell application \"{}\" to activate\n\
         on error\n\
         \tdo shell script \"open -a '{}'\"\n\
         end try",
        escape_applescript(app),
        escape_applescript(app)
    );
    run_osascript(&script).await
}

#[cfg(target_os = "macos")]
async fn handle_quit(app: &str) -> ToolResult {
    let script = format!(
        "tell application \"{}\" to quit",
        escape_applescript(app)
    );
    run_osascript(&script).await
}

#[cfg(target_os = "macos")]
async fn handle_quit_all() -> ToolResult {
    let script = r#"
tell application "System Events"
    set appList to name of every process whose visible is true
    repeat with appName in appList
        if appName is not "Finder" then
            try
                tell application appName to quit
            end try
        end if
    end repeat
end tell
return "All visible applications have been asked to quit"
"#;
    run_osascript(script).await
}

#[cfg(target_os = "macos")]
async fn handle_activate(app: &str) -> ToolResult {
    let script = format!(
        "tell application \"{}\" to activate",
        escape_applescript(app)
    );
    run_osascript(&script).await
}

#[cfg(target_os = "macos")]
async fn handle_hide(app: &str) -> ToolResult {
    let script = format!(
        "tell application \"System Events\" to set visible of process \"{}\" to false",
        escape_applescript(app)
    );
    run_osascript(&script).await
}

#[cfg(target_os = "macos")]
async fn handle_info(app: &str) -> ToolResult {
    // Use mdls to get app metadata from /Applications
    let script = format!(
        "do shell script \"mdls -name kMDItemDisplayName -name kMDItemVersion -name kMDItemContentType '/Applications/{}.app' 2>/dev/null || echo 'Application not found in /Applications'\"",
        escape_applescript(app)
    );
    run_osascript(&script).await
}

#[cfg(target_os = "macos")]
async fn handle_frontmost() -> ToolResult {
    run_osascript(
        "tell application \"System Events\" to return name of first process whose frontmost is true",
    )
    .await
}

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
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// ═══════════════════════════════════════════════════════════════════════
// Linux implementations
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "linux")]
async fn handle_list() -> ToolResult {
    // Use ps to list processes with visible windows, or wmctrl if available
    if which("wmctrl") {
        run_command("wmctrl", &["-l"]).await
    } else {
        // Fallback: list unique process names from /proc with open displays
        run_command("ps", &["aux", "--no-header", "-o", "comm"]).await
    }
}

#[cfg(target_os = "linux")]
async fn handle_launch(app: &str) -> ToolResult {
    // Try gtk-launch first (uses .desktop files), then xdg-open, then direct exec
    if which("gtk-launch") {
        let result = run_command("gtk-launch", &[app]).await;
        if !result.is_error {
            return result;
        }
    }
    if which("xdg-open") {
        run_command("xdg-open", &[app]).await
    } else {
        // Try launching directly
        match tokio::process::Command::new(app)
            .spawn()
        {
            Ok(_) => ToolResult::ok(format!("Launched '{}'", app)),
            Err(e) => ToolResult::error(format!("Failed to launch '{}': {}", app, e)),
        }
    }
}

#[cfg(target_os = "linux")]
async fn handle_quit(app: &str) -> ToolResult {
    // Find PID by name and send SIGTERM
    let output = tokio::process::Command::new("pgrep")
        .args(["-f", app])
        .output()
        .await;
    match output {
        Ok(out) if out.status.success() => {
            let pids = String::from_utf8_lossy(&out.stdout);
            let first_pid = pids.lines().next().unwrap_or("").trim();
            if first_pid.is_empty() {
                return ToolResult::error(format!("No process found for '{}'", app));
            }
            run_command("kill", &["-TERM", first_pid]).await
        }
        _ => ToolResult::error(format!("No process found for '{}'", app)),
    }
}

#[cfg(target_os = "linux")]
async fn handle_quit_all() -> ToolResult {
    if which("wmctrl") {
        // Get list of windows and close each
        let output = tokio::process::Command::new("wmctrl")
            .args(["-l"])
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => {
                let lines = String::from_utf8_lossy(&out.stdout);
                let mut closed = 0;
                for line in lines.lines() {
                    if let Some(wid) = line.split_whitespace().next() {
                        let _ = tokio::process::Command::new("wmctrl")
                            .args(["-i", "-c", wid])
                            .output()
                            .await;
                        closed += 1;
                    }
                }
                ToolResult::ok(format!("Closed {} windows", closed))
            }
            _ => ToolResult::error("Failed to list windows via wmctrl"),
        }
    } else {
        ToolResult::error("quit_all requires wmctrl on Linux (install with: sudo apt install wmctrl)")
    }
}

#[cfg(target_os = "linux")]
async fn handle_activate(app: &str) -> ToolResult {
    if which("wmctrl") {
        run_command("wmctrl", &["-a", app]).await
    } else if which("xdotool") {
        let output = tokio::process::Command::new("xdotool")
            .args(["search", "--name", app])
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => {
                let wid = String::from_utf8_lossy(&out.stdout);
                let first = wid.lines().next().unwrap_or("").trim();
                if first.is_empty() {
                    return ToolResult::error(format!("No window found for '{}'", app));
                }
                run_command("xdotool", &["windowactivate", first]).await
            }
            _ => ToolResult::error(format!("No window found for '{}'", app)),
        }
    } else {
        ToolResult::error("Window activation requires wmctrl or xdotool on Linux")
    }
}

#[cfg(target_os = "linux")]
async fn handle_hide(app: &str) -> ToolResult {
    if which("xdotool") {
        let output = tokio::process::Command::new("xdotool")
            .args(["search", "--name", app])
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => {
                let wid = String::from_utf8_lossy(&out.stdout);
                let first = wid.lines().next().unwrap_or("").trim();
                if first.is_empty() {
                    return ToolResult::error(format!("No window found for '{}'", app));
                }
                run_command("xdotool", &["windowminimize", first]).await
            }
            _ => ToolResult::error(format!("No window found for '{}'", app)),
        }
    } else {
        ToolResult::error("Window hiding requires xdotool on Linux (install with: sudo apt install xdotool)")
    }
}

#[cfg(target_os = "linux")]
async fn handle_info(app: &str) -> ToolResult {
    // Try to find .desktop file and read it
    let desktop_dirs = [
        "/usr/share/applications",
        "/usr/local/share/applications",
    ];
    let home = std::env::var("HOME").unwrap_or_default();
    let user_desktop = format!("{}/.local/share/applications", home);

    let app_lower = app.to_lowercase();
    for dir in desktop_dirs.iter().chain(std::iter::once(&user_desktop.as_str())) {
        let path = format!("{}/{}.desktop", dir, app_lower);
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            return ToolResult::ok(content);
        }
    }
    // Fallback: try to get process info
    let output = tokio::process::Command::new("ps")
        .args(["aux"])
        .output()
        .await;
    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let matches: Vec<&str> = text
                .lines()
                .filter(|l| l.to_lowercase().contains(&app_lower))
                .collect();
            if matches.is_empty() {
                ToolResult::error(format!("No info found for '{}'. App may not be running or installed.", app))
            } else {
                ToolResult::ok(matches.join("\n"))
            }
        }
        Err(e) => ToolResult::error(format!("Failed to get process info: {}", e)),
    }
}

#[cfg(target_os = "linux")]
async fn handle_frontmost() -> ToolResult {
    if which("xdotool") {
        run_command("xdotool", &["getactivewindow", "getwindowname"]).await
    } else {
        ToolResult::error("Getting frontmost window requires xdotool on Linux")
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Windows implementations (PowerShell)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "windows")]
async fn handle_list() -> ToolResult {
    let script = "Get-Process | Where-Object { $_.MainWindowTitle -ne '' } | \
                  Select-Object -Property Name, MainWindowTitle | Format-Table -AutoSize";
    run_powershell(script).await
}

#[cfg(target_os = "windows")]
async fn handle_launch(app: &str) -> ToolResult {
    let script = format!("Start-Process '{}'", escape_powershell(app));
    run_powershell(&script).await
}

#[cfg(target_os = "windows")]
async fn handle_quit(app: &str) -> ToolResult {
    // Try graceful close first, then force stop
    let script = format!(
        "$procs = Get-Process -Name '{}' -ErrorAction SilentlyContinue; \
         if ($procs) {{ $procs | ForEach-Object {{ $_.CloseMainWindow() | Out-Null }}; 'Quit signal sent' }} \
         else {{ 'No process found with name: {}' }}",
        escape_powershell(app),
        escape_powershell(app)
    );
    run_powershell(&script).await
}

#[cfg(target_os = "windows")]
async fn handle_quit_all() -> ToolResult {
    let script = "Get-Process | Where-Object { $_.MainWindowTitle -ne '' } | \
                  ForEach-Object { $_.CloseMainWindow() | Out-Null }; \
                  'All visible applications have been asked to quit'";
    run_powershell(script).await
}

#[cfg(target_os = "windows")]
async fn handle_activate(app: &str) -> ToolResult {
    let script = format!(
        "Add-Type @\"\n\
         using System;\n\
         using System.Runtime.InteropServices;\n\
         public class WinAPI {{\n\
             [DllImport(\"user32.dll\")] public static extern bool SetForegroundWindow(IntPtr hWnd);\n\
         }}\n\
         \"@\n\
         $proc = Get-Process -Name '{}' -ErrorAction SilentlyContinue | Select-Object -First 1;\n\
         if ($proc) {{ [WinAPI]::SetForegroundWindow($proc.MainWindowHandle) | Out-Null; 'Activated' }}\n\
         else {{ 'No process found with name: {}' }}",
        escape_powershell(app),
        escape_powershell(app)
    );
    run_powershell(&script).await
}

#[cfg(target_os = "windows")]
async fn handle_hide(app: &str) -> ToolResult {
    let script = format!(
        "Add-Type @\"\n\
         using System;\n\
         using System.Runtime.InteropServices;\n\
         public class WinAPI {{\n\
             [DllImport(\"user32.dll\")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);\n\
         }}\n\
         \"@\n\
         $proc = Get-Process -Name '{}' -ErrorAction SilentlyContinue | Select-Object -First 1;\n\
         if ($proc) {{ [WinAPI]::ShowWindow($proc.MainWindowHandle, 0) | Out-Null; 'Hidden' }}\n\
         else {{ 'No process found with name: {}' }}",
        escape_powershell(app),
        escape_powershell(app)
    );
    run_powershell(&script).await
}

#[cfg(target_os = "windows")]
async fn handle_info(app: &str) -> ToolResult {
    let script = format!(
        "$proc = Get-Process -Name '{}' -ErrorAction SilentlyContinue | Select-Object -First 1;\n\
         if ($proc) {{ $proc | Select-Object Name, Id, CPU, WorkingSet64, \
         MainWindowTitle, Path, StartTime | Format-List }}\n\
         else {{ 'No process found with name: {}' }}",
        escape_powershell(app),
        escape_powershell(app)
    );
    run_powershell(&script).await
}

#[cfg(target_os = "windows")]
async fn handle_frontmost() -> ToolResult {
    let script = r#"
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class WinAPI {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);
}
"@
$hwnd = [WinAPI]::GetForegroundWindow()
$sb = New-Object System.Text.StringBuilder 256
[WinAPI]::GetWindowText($hwnd, $sb, 256) | Out-Null
$sb.ToString()
"#;
    run_powershell(script).await
}

// ═══════════════════════════════════════════════════════════════════════
// Shell helpers
// ═══════════════════════════════════════════════════════════════════════

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

#[cfg(target_os = "windows")]
fn escape_powershell(s: &str) -> String {
    s.replace('\'', "''")
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
        let tool = AppTool::new();
        assert_eq!(tool.name(), "app");
        assert!(tool.description().contains("list"));
        assert!(tool.description().contains("launch"));
        assert!(tool.description().contains("quit"));
        assert!(tool.description().contains("frontmost"));
        let schema = tool.schema();
        assert!(schema["properties"]["action"].is_object());
        assert!(schema["properties"]["app"].is_object());
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let tool = AppTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "unknown"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown action"));
    }

    #[tokio::test]
    async fn test_launch_missing_app() {
        let tool = AppTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "launch"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("'app' parameter required"));
    }

    #[tokio::test]
    async fn test_quit_missing_app() {
        let tool = AppTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "quit"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("'app' parameter required"));
    }

    #[tokio::test]
    async fn test_activate_missing_app() {
        let tool = AppTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "activate"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("'app' parameter required"));
    }

    #[tokio::test]
    async fn test_hide_missing_app() {
        let tool = AppTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "hide"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("'app' parameter required"));
    }

    #[tokio::test]
    async fn test_info_missing_app() {
        let tool = AppTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "info"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("'app' parameter required"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_list_apps() {
        let result = handle_list().await;
        assert!(!result.is_error, "list should succeed: {}", result.content);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_frontmost_app() {
        let result = handle_frontmost().await;
        assert!(!result.is_error, "frontmost should succeed: {}", result.content);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_escape_applescript() {
        assert_eq!(escape_applescript("hello"), "hello");
        assert_eq!(escape_applescript("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(escape_applescript("path\\to"), "path\\\\to");
    }
}
