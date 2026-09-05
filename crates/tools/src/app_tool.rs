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
                        return ToolResult::error(crate::errors::missing_param(
                            "launch",
                            "app",
                            "app(action: \"launch\", app: \"Safari\")",
                        ));
                    }
                    handle_launch(app).await
                }
                "quit" => {
                    if app.is_empty() {
                        return ToolResult::error(crate::errors::missing_param(
                            "quit",
                            "app",
                            "app(action: \"quit\", app: \"Safari\")",
                        ));
                    }
                    handle_quit(app).await
                }
                "quit_all" => handle_quit_all().await,
                "activate" => {
                    if app.is_empty() {
                        return ToolResult::error(crate::errors::missing_param(
                            "activate",
                            "app",
                            "app(action: \"activate\", app: \"Safari\")",
                        ));
                    }
                    handle_activate(app).await
                }
                "hide" => {
                    if app.is_empty() {
                        return ToolResult::error(crate::errors::missing_param(
                            "hide",
                            "app",
                            "app(action: \"hide\", app: \"Safari\")",
                        ));
                    }
                    handle_hide(app).await
                }
                "info" => {
                    if app.is_empty() {
                        return ToolResult::error(crate::errors::missing_param(
                            "info",
                            "app",
                            "app(action: \"info\", app: \"Safari\")",
                        ));
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
         \ttell application \"{app}\" to activate\n\
         on error\n\
         \tdo shell script \"open -a '{app}'\"\n\
         end try\n\
         return \"Launch request sent to {app}; confirm with app(action: \\\"list\\\")\"",
        app = escape_applescript(app),
    );
    run_osascript(&script).await
}

#[cfg(target_os = "macos")]
async fn handle_quit(app: &str) -> ToolResult {
    let script = format!(
        "tell application \"{app}\" to quit\nreturn \"Quit request sent to {app}; confirm with app(action: \\\"list\\\")\"",
        app = escape_applescript(app)
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
        "tell application \"{app}\" to activate\nreturn \"Activate request sent to {app}; confirm with app(action: \\\"frontmost\\\")\"",
        app = escape_applescript(app)
    );
    run_osascript(&script).await
}

#[cfg(target_os = "macos")]
async fn handle_hide(app: &str) -> ToolResult {
    let script = format!(
        "tell application \"System Events\" to set visible of process \"{app}\" to false\nreturn \"Hide request sent to {app}\"",
        app = escape_applescript(app)
    );
    let result = run_osascript(&script).await;
    // -1728 ("Can't get process") means System Events has no such process:
    // the app is not running, which is the fact worth reporting.
    if result.is_error && result.content.contains("-1728") {
        return ToolResult::error(format!(
            "{} is not running (System Events has no process named '{}'); nothing to hide. Running apps: app(action: \"list\")",
            app, app
        ));
    }
    result
}

/// Directories searched for `<app>.app` by `info`, in order.
#[cfg(target_os = "macos")]
fn app_bundle_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = vec![
        std::path::PathBuf::from("/Applications"),
        std::path::PathBuf::from("/Applications/Utilities"),
        std::path::PathBuf::from("/System/Applications"),
        std::path::PathBuf::from("/System/Applications/Utilities"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(std::path::PathBuf::from(home).join("Applications"));
    }
    dirs
}

/// Relabel `mdls` output (`kMDItemVersion = "1.2"`) into plain fields.
#[cfg(target_os = "macos")]
fn relabel_mdls(raw: &str) -> Vec<String> {
    raw.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(" = ")?;
            let label = match key.trim() {
                "kMDItemDisplayName" => "Name",
                "kMDItemVersion" => "Version",
                "kMDItemCFBundleIdentifier" => "Bundle id",
                "kMDItemContentType" => "Kind",
                "kMDItemLastUsedDate" => "Last opened",
                other => other,
            };
            let value = value.trim().trim_matches('"');
            if value == "(null)" {
                return None;
            }
            Some(format!("{}: {}", label, value))
        })
        .collect()
}

#[cfg(target_os = "macos")]
async fn handle_info(app: &str) -> ToolResult {
    let dirs = app_bundle_dirs();
    let bundle = dirs
        .iter()
        .map(|d| d.join(format!("{}.app", app)))
        .find(|p| p.exists());
    let searched = dirs
        .iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let Some(bundle) = bundle else {
        return ToolResult::error(format!(
            "No '{}.app' in {}. Confirm the exact name with app(action: \"list\") (running apps only).",
            app, searched
        ));
    };

    let output = tokio::process::Command::new("mdls")
        .args([
            "-name",
            "kMDItemDisplayName",
            "-name",
            "kMDItemVersion",
            "-name",
            "kMDItemCFBundleIdentifier",
            "-name",
            "kMDItemContentType",
            "-name",
            "kMDItemLastUsedDate",
        ])
        .arg(&bundle)
        .output()
        .await;
    let mut lines = vec![format!("Path: {}", bundle.display())];
    match output {
        Ok(o) if o.status.success() => {
            lines.extend(relabel_mdls(&String::from_utf8_lossy(&o.stdout)));
        }
        Ok(o) => lines.push(format!(
            "(mdls exited {}: {}; metadata unavailable)",
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => lines.push(format!("(mdls could not run: {}; metadata unavailable)", e)),
    }
    ToolResult::ok(lines.join("\n"))
}

#[cfg(target_os = "macos")]
async fn handle_frontmost() -> ToolResult {
    let result = run_osascript(
        "tell application \"System Events\" to return name of first process whose frontmost is true",
    )
    .await;

    // No GUI session (headless, locked screen, fast-user-switched, or a
    // sandboxed test runner) means there genuinely is no frontmost process —
    // System Events reports "Invalid index" (-1719). That's a valid state to
    // report, not a tool failure: answer it cleanly instead of erroring.
    if result.is_error && (result.content.contains("-1719") || result.content.contains("Invalid index")) {
        return ToolResult::ok("No frontmost application (no active GUI session).".to_string());
    }
    result
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
            ToolResult::ok(if text.is_empty() {
                "(exit 0, no output)".to_string()
            } else {
                text
            })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let detail = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                "(no output)".to_string()
            };
            let code = output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "terminated by signal".into());
            ToolResult::error(format!("osascript exited {}: {}", code, detail))
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
        // Fallback: every process's command name (no window manager to ask)
        run_command("ps", &["-eo", "comm", "--no-headers"]).await
    }
}

#[cfg(target_os = "linux")]
async fn handle_launch(app: &str) -> ToolResult {
    // Try gtk-launch first (uses .desktop files), then xdg-open, then direct exec
    if which("gtk-launch") {
        let result = run_command("gtk-launch", &[app]).await;
        if !result.is_error {
            return ToolResult::ok(format!(
                "Launch request sent to '{}' via gtk-launch; confirm with app(action: \"list\")",
                app
            ));
        }
    }
    if which("xdg-open") {
        let result = run_command("xdg-open", &[app]).await;
        if result.is_error {
            return result;
        }
        ToolResult::ok(format!(
            "Launch request sent to '{}' via xdg-open; confirm with app(action: \"list\")",
            app
        ))
    } else {
        // Try launching directly
        match tokio::process::Command::new(app).spawn() {
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
                return ToolResult::error(format!("No process found for '{}' (pgrep -f matched nothing)", app));
            }
            let result = run_command("kill", &["-TERM", first_pid]).await;
            if result.is_error {
                return result;
            }
            ToolResult::ok(format!(
                "SIGTERM sent to pid {} ({}); confirm with app(action: \"list\")",
                first_pid, app
            ))
        }
        _ => ToolResult::error(format!("No process found for '{}' (pgrep -f matched nothing)", app)),
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
                let mut sent = 0;
                let mut failed = 0;
                for line in lines.lines() {
                    if let Some(wid) = line.split_whitespace().next() {
                        let r = tokio::process::Command::new("wmctrl")
                            .args(["-i", "-c", wid])
                            .output()
                            .await;
                        match r {
                            Ok(o) if o.status.success() => sent += 1,
                            _ => failed += 1,
                        }
                    }
                }
                ToolResult::ok(format!(
                    "Close request sent to {} windows ({} wmctrl calls failed); whether each app actually closed is not checked, confirm with app(action: \"list\")",
                    sent, failed
                ))
            }
            _ => ToolResult::error("Failed to list windows via wmctrl"),
        }
    } else {
        ToolResult::error(
            "quit_all requires wmctrl on Linux (ask the owner to install it; Nebo cannot run sudo. Package: wmctrl)",
        )
    }
}

#[cfg(target_os = "linux")]
async fn handle_activate(app: &str) -> ToolResult {
    if which("wmctrl") {
        let result = run_command("wmctrl", &["-a", app]).await;
        if result.is_error {
            return result;
        }
        ToolResult::ok(format!(
            "Activate request sent for '{}' via wmctrl; confirm with app(action: \"frontmost\")",
            app
        ))
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
                let result = run_command("xdotool", &["windowactivate", first]).await;
                if result.is_error {
                    return result;
                }
                ToolResult::ok(format!(
                    "Activate request sent to window {} ('{}') via xdotool; confirm with app(action: \"frontmost\")",
                    first, app
                ))
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
                let result = run_command("xdotool", &["windowminimize", first]).await;
                if result.is_error {
                    return result;
                }
                ToolResult::ok(format!(
                    "Minimize request sent to window {} ('{}') via xdotool",
                    first, app
                ))
            }
            _ => ToolResult::error(format!("No window found for '{}'", app)),
        }
    } else {
        ToolResult::error(
            "Window hiding requires xdotool on Linux (ask the owner to install it; Nebo cannot run sudo. Package: xdotool)",
        )
    }
}

#[cfg(target_os = "linux")]
async fn handle_info(app: &str) -> ToolResult {
    // Try to find .desktop file and read it
    let desktop_dirs = ["/usr/share/applications", "/usr/local/share/applications"];
    let home = std::env::var("HOME").unwrap_or_default();
    let user_desktop = format!("{}/.local/share/applications", home);

    let app_lower = app.to_lowercase();
    for dir in desktop_dirs
        .iter()
        .chain(std::iter::once(&user_desktop.as_str()))
    {
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
                ToolResult::error(format!(
                    "No {}.desktop in {}, {}, or {}, and no running process matching '{}'.",
                    app_lower, desktop_dirs[0], desktop_dirs[1], user_desktop, app
                ))
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
    let script = format!(
        "Start-Process '{app}' -ErrorAction Stop; 'Launch request sent to {app}; confirm with app(action: \"list\")'",
        app = escape_powershell(app)
    );
    run_powershell(&script).await
}

#[cfg(target_os = "windows")]
async fn handle_quit(app: &str) -> ToolResult {
    // Try graceful close first, then force stop
    let script = format!(
        "$procs = Get-Process -Name '{}' -ErrorAction SilentlyContinue; \
         if ($procs) {{ $n = 0; $refused = 0; $procs | ForEach-Object {{ if ($_.CloseMainWindow()) {{ $n++ }} else {{ $refused++ }} }}; \
         \"Close request sent to $n window(s) of {}; $refused had no main window to close; confirm with app(action: 'list')\" }} \
         else {{ 'Nothing done: no running process named {} (Get-Process -Name matched nothing)'; exit 1 }}",
        escape_powershell(app),
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
         if ($proc) {{ if ([WinAPI]::SetForegroundWindow($proc.MainWindowHandle)) {{ 'Activated {}: SetForegroundWindow returned true' }} else {{ 'SetForegroundWindow returned false for {}: Windows refused to bring it to the foreground; the window is unchanged'; exit 1 }} }}\n\
         else {{ 'Nothing done: no running process named {} (Get-Process -Name matched nothing)'; exit 1 }}",
        escape_powershell(app),
        escape_powershell(app),
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
         if ($proc) {{ if ([WinAPI]::ShowWindow($proc.MainWindowHandle, 0)) {{ 'Hidden {} (its window was visible)' }} else {{ 'Hide sent to {}: ShowWindow reported the window was already hidden' }} }}\n\
         else {{ 'Nothing done: no running process named {} (Get-Process -Name matched nothing)'; exit 1 }}",
        escape_powershell(app),
        escape_powershell(app),
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
         else {{ 'No running process named {} (Get-Process -Name matched nothing); confirm the name with app(action: \"list\")'; exit 1 }}",
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
// Fallback for unsupported platforms (Android, etc.)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
async fn handle_list() -> ToolResult {
    ToolResult::error("Listing desktop applications is not available on Android")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
async fn handle_launch(_app: &str) -> ToolResult {
    ToolResult::error("Launching desktop applications is not available on Android")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
async fn handle_quit(_app: &str) -> ToolResult {
    ToolResult::error("Quitting desktop applications is not available on Android")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
async fn handle_quit_all() -> ToolResult {
    ToolResult::error("Quitting desktop applications is not available on Android")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
async fn handle_activate(_app: &str) -> ToolResult {
    ToolResult::error("Activating desktop applications is not available on Android")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
async fn handle_hide(_app: &str) -> ToolResult {
    ToolResult::error("Hiding desktop applications is not available on Android")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
async fn handle_info(_app: &str) -> ToolResult {
    ToolResult::error("Desktop application info is not available on Android")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
async fn handle_frontmost() -> ToolResult {
    ToolResult::error("No frontmost desktop application on Android")
}

// ═══════════════════════════════════════════════════════════════════════
// Shell helpers
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "linux")]
async fn run_command(cmd: &str, args: &[&str]) -> ToolResult {
    match tokio::process::Command::new(cmd).args(args).output().await {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::ok(if text.is_empty() {
                "(exit 0, no output)".to_string()
            } else {
                text
            })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let code = output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into());
            let detail = [stdout, stderr].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
            ToolResult::error(format!(
                "'{} {}' exited {}{}",
                cmd,
                args.join(" "),
                code,
                if detail.is_empty() { " and printed nothing".to_string() } else { format!(": {detail}") }
            ))
        }
        Err(e) => ToolResult::error(format!("Command '{}' failed: {}", cmd, e)),
    }
}

/// Like `run_command` for a PowerShell script, but the error names the
/// script's own output rather than echoing the whole script text back.
#[cfg(target_os = "windows")]
async fn run_powershell(script: &str) -> ToolResult {
    match tokio::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", script])
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::ok(if text.is_empty() {
                "(exit 0, no output)".to_string()
            } else {
                text
            })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let code = output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into());
            let detail = [stdout, stderr].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
            ToolResult::error(format!(
                "PowerShell exited {}{}",
                code,
                if detail.is_empty() { " and printed nothing".to_string() } else { format!(": {detail}") }
            ))
        }
        Err(e) => ToolResult::error(format!("PowerShell could not be started: {}", e)),
    }
}

#[cfg(target_os = "windows")]
fn escape_powershell(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(target_os = "linux")]
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
        assert!(result.content.contains("Missing required parameter 'app'"));
    }

    #[tokio::test]
    async fn test_quit_missing_app() {
        let tool = AppTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "quit"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing required parameter 'app'"));
    }

    #[tokio::test]
    async fn test_activate_missing_app() {
        let tool = AppTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "activate"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing required parameter 'app'"));
    }

    #[tokio::test]
    async fn test_hide_missing_app() {
        let tool = AppTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "hide"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing required parameter 'app'"));
    }

    #[tokio::test]
    async fn test_info_missing_app() {
        let tool = AppTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "info"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing required parameter 'app'"));
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
        // Must not hard-error regardless of GUI state: a real frontmost app
        // name when a session is active, or a clean "no frontmost" message
        // when there isn't one (headless/locked/CI). Never a raw AppleScript
        // error surfaced to the model.
        let result = handle_frontmost().await;
        assert!(
            !result.is_error,
            "frontmost must degrade gracefully, not error: {}",
            result.content
        );
        assert!(!result.content.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_escape_applescript() {
        assert_eq!(escape_applescript("hello"), "hello");
        assert_eq!(escape_applescript("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(escape_applescript("path\\to"), "path\\\\to");
    }
}
