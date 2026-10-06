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
         os(resource: \"app\", action: \"list\")\n  \
         os(resource: \"app\", action: \"launch\", app: \"Safari\")\n  \
         os(resource: \"app\", action: \"quit\", app: \"Slack\")\n  \
         os(resource: \"app\", action: \"activate\", app: \"Terminal\")\n  \
         os(resource: \"app\", action: \"info\", app: \"Xcode\")\n  \
         os(resource: \"app\", action: \"frontmost\")"
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


    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let action = input["action"].as_str().unwrap_or("");
            let app = input["app"].as_str().unwrap_or("");

            match action {
                "list" => crate::result_shape::keep_lines(handle_list().await, input["filter"].as_str().unwrap_or(""), input["limit"].as_u64().map(|n| n as usize)),
                "launch" => {
                    if app.is_empty() {
                        return ToolResult::error(crate::errors::missing_param(
                            "launch",
                            "app",
                            "os(resource: \"app\", action: \"launch\", app: \"Safari\")",
                        ));
                    }
                    handle_launch(app).await
                }
                "quit" => {
                    if app.is_empty() {
                        return ToolResult::error(crate::errors::missing_param(
                            "quit",
                            "app",
                            "os(resource: \"app\", action: \"quit\", app: \"Safari\")",
                        ));
                    }
                    if is_protected_process(app) {
                        return ToolResult::error(format!(
                            "'{app}' is a protected system process (or Nebo itself) and is never quit by a tool: the session, the desktop or this agent would go with it."
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
                            "os(resource: \"app\", action: \"activate\", app: \"Safari\")",
                        ));
                    }
                    handle_activate(app).await
                }
                "hide" => {
                    if app.is_empty() {
                        return ToolResult::error(crate::errors::missing_param(
                            "hide",
                            "app",
                            "os(resource: \"app\", action: \"hide\", app: \"Safari\")",
                        ));
                    }
                    handle_hide(app).await
                }
                "info" => {
                    if app.is_empty() {
                        return ToolResult::error(crate::errors::missing_param(
                            "info",
                            "app",
                            "os(resource: \"app\", action: \"info\", app: \"Safari\")",
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
         return \"Launch request sent to {app}; confirm with os(resource: \\\"app\\\", action: \\\"list\\\")\"",
        app = escape_applescript(app),
    );
    run_osascript(&script).await
}

/// Processes a quit would take the session, the desktop or this agent down
/// with. Matched on the whole name or any dotted part of a bundle id, any case.
const PROTECTED_PROCESSES: &[&str] =
    &["loginwindow", "windowserver", "dock", "launchd", "finder", "systemuiserver", "controlcenter", "nebo",
      "explorer", "dwm", "winlogon", "csrss", "wininit", "lsass", "services", "smss", "svchost",
      "applicationframehost", "shellexperiencehost", "startmenuexperiencehost", "searchhost", "textinputhost",
      "sihost", "ctfmon", "runtimebroker", "lockapp"];

pub(crate) fn is_protected_process(app: &str) -> bool {
    let a = app.trim().trim_end_matches(".app").to_lowercase();
    PROTECTED_PROCESSES.iter().any(|p| a == *p || a.split('.').any(|part| part == *p))
}

#[cfg(target_os = "macos")]
async fn handle_quit(app: &str) -> ToolResult {
    let script = format!(
        "tell application \"{app}\" to quit\nreturn \"Quit request sent to {app}; confirm with os(resource: \\\"app\\\", action: \\\"list\\\")\"",
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
        if appName is not in {"Finder", "Nebo", "Dock", "SystemUIServer", "ControlCenter", "loginwindow"} then
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
        "tell application \"{app}\" to activate\nreturn \"Activate request sent to {app}; confirm with os(resource: \\\"app\\\", action: \\\"frontmost\\\")\"",
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
            "{} is not running (System Events has no process named '{}'); nothing to hide. Running apps: os(resource: \"app\", action: \"list\")",
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
            "No '{}.app' in {}. Confirm the exact name with os(resource: \"app\", action: \"list\") (running apps only).",
            app, searched
        ));
    };

    let output = command::new::<tokio::process::Command>("mdls", command::Console::Hidden)
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
    // The helper answers without AppleEvents; System Events is the fallback
    // (on a Mac without the Automation grant it hangs on a consent prompt).
    if let Ok(name) = crate::ax_native::frontmost().await {
        if crate::ax_native::is_lock_screen(&name) {
            return ToolResult::ok(crate::ax_native::LOCKED_SCREEN);
        }
        if !name.is_empty() {
            return ToolResult::ok(name);
        }
    }
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
    match command::new::<tokio::process::Command>("osascript", command::Console::Hidden)
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
                "Launch request sent to '{}' via gtk-launch; confirm with os(resource: \"app\", action: \"list\")",
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
            "Launch request sent to '{}' via xdg-open; confirm with os(resource: \"app\", action: \"list\")",
            app
        ))
    } else {
        // Try launching directly
        match command::new::<tokio::process::Command>(app, command::Console::Hidden).spawn() {
            Ok(_) => ToolResult::ok(format!("Launched '{}'", app)),
            Err(e) => ToolResult::error(format!("Failed to launch '{}': {}", app, e)),
        }
    }
}

#[cfg(target_os = "linux")]
async fn handle_quit(app: &str) -> ToolResult {
    // Find PID by name and send SIGTERM
    let output = command::new::<tokio::process::Command>("pgrep", command::Console::Hidden)
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
                "SIGTERM sent to pid {} ({}); confirm with os(resource: \"app\", action: \"list\")",
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
        let output = command::new::<tokio::process::Command>("wmctrl", command::Console::Hidden)
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
                        let r = command::new::<tokio::process::Command>("wmctrl", command::Console::Hidden)
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
                    "Close request sent to {} windows ({} wmctrl calls failed); whether each app actually closed is not checked, confirm with os(resource: \"app\", action: \"list\")",
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
            "Activate request sent for '{}' via wmctrl; confirm with os(resource: \"app\", action: \"frontmost\")",
            app
        ))
    } else if which("xdotool") {
        let output = command::new::<tokio::process::Command>("xdotool", command::Console::Hidden)
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
                    "Activate request sent to window {} ('{}') via xdotool; confirm with os(resource: \"app\", action: \"frontmost\")",
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
        let output = command::new::<tokio::process::Command>("xdotool", command::Console::Hidden)
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
    let output = command::new::<tokio::process::Command>("ps", command::Console::Hidden)
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

/// How every Windows app action finds the app the owner named, the way
/// macOS finds it by its name:
/// - `Find-NeboStartApp $q`: the Start menu app named `$q` (exactly, else
///   containing it), with its AppID.
/// - `Find-NeboAppProcess $q`: the app's processes, by process name; else
///   the Start menu app's, which run from its package folder ("Xbox" is
///   `XboxPcApp` and its helper); else the apps with a window whose name,
///   title or description contains `$q`. A shell host is never the app:
///   ApplicationFrameHost hosts the windows of many apps (Settings among
///   them), and ending it would close every one.
#[cfg(target_os = "windows")]
const APP_RESOLVER: &str = r#"
$NeboHosts = @('ApplicationFrameHost', 'explorer', 'ShellExperienceHost', 'StartMenuExperienceHost', 'SearchHost', 'TextInputHost', 'dwm', 'sihost', 'ctfmon', 'RuntimeBroker', 'LockApp', 'SystemSettingsBroker')
function Test-NeboContains([string]$s, [string]$q) { $s -and $s.IndexOf($q, [StringComparison]::OrdinalIgnoreCase) -ge 0 }
function Find-NeboStartApp([string]$q) {
  $apps = @(Get-StartApps)
  $hit = $apps | Where-Object { $_.Name -ieq $q } | Select-Object -First 1
  if (-not $hit) { $hit = $apps | Where-Object { Test-NeboContains $_.Name $q } | Select-Object -First 1 }
  $hit
}
function Find-NeboAppProcess([string]$q) {
  $procs = @(Get-Process -Name $q -ErrorAction SilentlyContinue)
  if ($procs) { return $procs }
  $app = @(Get-StartApps) | Where-Object { $_.Name -ieq $q } | Select-Object -First 1
  if ($app -and $app.AppID -match '^([^_!\\]+)_[a-z0-9]+!') {
    $fam = $Matches[1]
    $procs = @(Get-Process | Where-Object { $_.Path -like "*\WindowsApps\$($fam)_*" -and $NeboHosts -notcontains $_.Name })
    if ($procs) { return $procs }
  }
  $win = @(Get-Process | Where-Object { $_.MainWindowTitle -ne '' -and $NeboHosts -notcontains $_.Name -and ((Test-NeboContains $_.Name $q) -or (Test-NeboContains $_.MainWindowTitle $q) -or (Test-NeboContains $_.Description $q)) })
  if ($win) { return @(Get-Process -Name ($win | Select-Object -ExpandProperty Name -Unique) -ErrorAction SilentlyContinue) }
  @()
}
"#;

/// `script` with the app resolver before it and `{app}` bound, quoted.
#[cfg(target_os = "windows")]
fn app_script(script: &str, app: &str) -> String {
    format!("{APP_RESOLVER}{}", script.replace("{app}", &escape_powershell(app)))
}

/// Launch on Windows: a file or folder path opens; else the Start menu app
/// with that name; else a program on PATH. A bare name handed to
/// `Start-Process` as it is opens the "Pick an app" picker and starts
/// nothing, so nothing is launched that did not resolve.
#[cfg(target_os = "windows")]
const LAUNCH_SCRIPT: &str = r#"
$q = '{app}'
if (Test-Path -LiteralPath $q) { Start-Process -FilePath $q -ErrorAction Stop; "Opened $q"; return }
$app = @(Get-StartApps) | Where-Object { $_.Name -ieq $q } | Select-Object -First 1
if (-not $app) {
  $cmd = Get-Command -Name $q, "$q.exe" -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($cmd) { Start-Process -FilePath $cmd.Source -ErrorAction Stop; "Launched $($cmd.Name) ($($cmd.Source)); confirm with os(resource: 'app', action: 'list')"; return }
  $app = Find-NeboStartApp $q
}
if ($app) { Start-Process "shell:AppsFolder\$($app.AppID)" -ErrorAction Stop; "Launched $($app.Name); confirm with os(resource: 'app', action: 'list')"; return }
"Nothing launched: no Start menu app, program on PATH or file is named '$q'"; exit 1
"#;

#[cfg(target_os = "windows")]
async fn handle_launch(app: &str) -> ToolResult {
    run_powershell(&app_script(LAUNCH_SCRIPT, app)).await
}

/// Quit an app on Windows the way `quit` does on macOS: the app ends, not
/// just its window. The app is the resolver's (`Find-NeboAppProcess`). Each
/// process is asked to close; one that only hides to the tray while no
/// window of it is left is stopped, while one still showing a window (a
/// save prompt) is left for the owner and named. The reply says which ended.
#[cfg(target_os = "windows")]
const QUIT_SCRIPT: &str = r#"
$q = '{app}'
$procs = @(Find-NeboAppProcess $q)
if (-not $procs) { "Nothing done: no running app matches '$q' by process name, Start menu name, window title or description; os(resource: 'app', action: 'list') shows what is running"; exit 1 }
$names = (($procs | Select-Object -ExpandProperty Name -Unique) -join ', ')
$ids = @($procs | Select-Object -ExpandProperty Id)
$procs | ForEach-Object { [void]$_.CloseMainWindow() }
$deadline = (Get-Date).AddSeconds(5)
do { Start-Sleep -Milliseconds 250; $left = @(Get-Process -Id $ids -ErrorAction SilentlyContinue) } while ($left -and (Get-Date) -lt $deadline)
$prompting = @($left | Where-Object { $_.MainWindowHandle -ne 0 -and $_.MainWindowTitle -ne '' })
$tray = @($left | Where-Object { $_.MainWindowHandle -eq 0 -or $_.MainWindowTitle -eq '' })
if ($tray -and -not $prompting) { $tray | Stop-Process -Force -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 500 }
$still = @(Get-Process -Id $ids -ErrorAction SilentlyContinue)
if (-not $still) { "Quit ${names}: it is no longer running" }
elseif ($prompting) { "$names did not quit: its window '$(($prompting | Select-Object -First 1).MainWindowTitle)' is still open (it may be asking to save, or still starting up); nothing was forced"; exit 1 }
else { "$names is still running ($($still.Count) process(es)) after it was asked to close and then stopped; Windows refused to end it"; exit 1 }
"#;

#[cfg(target_os = "windows")]
async fn handle_quit(app: &str) -> ToolResult {
    run_powershell(&app_script(QUIT_SCRIPT, app)).await
}

#[cfg(target_os = "windows")]
async fn handle_quit_all() -> ToolResult {
    let script = "Get-Process | Where-Object { $_.MainWindowTitle -ne '' } | \
                  ForEach-Object { $_.CloseMainWindow() | Out-Null }; \
                  'All visible applications have been asked to quit'";
    run_powershell(script).await
}

/// Activate on Windows: the app's window to the front — its open dialog
/// when it has one (a window with a modal dialog takes no input itself).
#[cfg(target_os = "windows")]
const ACTIVATE_SCRIPT: &str = r#"
Add-Type @"
using System; using System.Runtime.InteropServices;
public class NeboActivate {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern IntPtr GetLastActivePopup(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
}
"@
$q = '{app}'
$proc = @(Find-NeboAppProcess $q) | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { "Nothing done: no running app named '$q' has a window; os(resource: 'app', action: 'list') shows what is running"; exit 1 }
$h = $proc.MainWindowHandle
if ([NeboActivate]::IsIconic($h)) { [void][NeboActivate]::ShowWindow($h, 9) }
$front = [NeboActivate]::GetLastActivePopup($h)
if ([NeboActivate]::SetForegroundWindow($front)) { "Activated $($proc.Name)" + $(if ($front -ne $h) { ' (its open dialog is in front)' } else { '' }) }
else { "Windows refused to bring $($proc.Name) to the foreground; the window is unchanged"; exit 1 }
"#;

#[cfg(target_os = "windows")]
async fn handle_activate(app: &str) -> ToolResult {
    run_powershell(&app_script(ACTIVATE_SCRIPT, app)).await
}

#[cfg(target_os = "windows")]
const HIDE_SCRIPT: &str = r#"
Add-Type @"
using System; using System.Runtime.InteropServices;
public class NeboHide { [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow); }
"@
$q = '{app}'
$proc = @(Find-NeboAppProcess $q) | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { "Nothing done: no running app named '$q' has a window; os(resource: 'app', action: 'list') shows what is running"; exit 1 }
if ([NeboHide]::ShowWindow($proc.MainWindowHandle, 0)) { "Hidden $($proc.Name) (its window was visible)" } else { "$($proc.Name)'s window was already hidden" }
"#;

#[cfg(target_os = "windows")]
async fn handle_hide(app: &str) -> ToolResult {
    run_powershell(&app_script(HIDE_SCRIPT, app)).await
}

#[cfg(target_os = "windows")]
const INFO_SCRIPT: &str = r#"
$q = '{app}'
$proc = @(Find-NeboAppProcess $q) | Select-Object -First 1
if ($proc) { $proc | Select-Object Name, Id, CPU, WorkingSet64, MainWindowTitle, Path, StartTime | Format-List }
else { "No running app matches '$q'; confirm the name with os(resource: 'app', action: 'list')"; exit 1 }
"#;

#[cfg(target_os = "windows")]
async fn handle_info(app: &str) -> ToolResult {
    run_powershell(&app_script(INFO_SCRIPT, app)).await
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
    match command::new::<tokio::process::Command>(cmd, command::Console::Hidden).args(args).output().await {
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
    match command::powershell::<tokio::process::Command>(&script)
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
    command::new::<std::process::Command>("which", command::Console::Hidden)
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

#[cfg(all(test, target_os = "windows"))]
mod windows_quit_tests {
    /// Every Windows app script parses, with the resolver before it.
    #[tokio::test]
    async fn every_app_script_parses() {
        for (name, script) in [
            ("launch", super::LAUNCH_SCRIPT),
            ("quit", super::QUIT_SCRIPT),
            ("activate", super::ACTIVATE_SCRIPT),
            ("hide", super::HIDE_SCRIPT),
            ("info", super::INFO_SCRIPT),
        ] {
            let full = super::app_script(script, "It's");
            let check = format!(
                "$e = $null; [void][System.Management.Automation.Language.Parser]::ParseInput('{}', [ref]$null, [ref]$e); \
                 if ($e) {{ $e[0].Message + ' at line ' + $e[0].Extent.StartLineNumber; exit 1 }}; 'parsed'",
                full.replace('\'', "''")
            );
            let r = super::run_powershell(&check).await;
            assert!(!r.is_error && r.content == "parsed", "the {name} script does not parse: {}", r.content);
        }
    }

    /// `quit` ends the app and says so, found by the name the owner uses.
    #[tokio::test]
    #[ignore = "launches and quits Character Map on the signed-in desktop"]
    async fn quit_ends_a_running_app() {
        let launched = super::handle_launch("charmap").await;
        assert!(!launched.is_error, "{}", launched.content);
        assert!(launched.content.starts_with("Launched charmap.exe"), "{}", launched.content);
        let nothing = super::handle_launch("no-such-app-xyz").await;
        assert!(nothing.is_error && nothing.content.contains("Nothing launched"), "{}", nothing.content);
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        // By a word of its window title, as the owner names it.
        let quit = super::handle_quit("Character Map").await;
        assert!(!quit.is_error, "{}", quit.content);
        assert!(quit.content.starts_with("Quit charmap"), "{}", quit.content);
        assert!(super::is_protected_process("ApplicationFrameHost"), "the host of many apps' windows is never quit");
        let missing = super::handle_quit("no-such-app-xyz").await;
        assert!(missing.is_error && missing.content.contains("Nothing done: no running app matches"), "{}", missing.content);
    }
}

#[cfg(test)]
mod protected_tests {
    #[test]
    fn protected_processes_are_matched_by_name_or_bundle_part() {
        for p in ["Finder", "finder", "com.apple.finder", "Dock", "loginwindow", "Nebo", "Nebo.app", "WindowServer", "explorer", "dwm"] {
            assert!(super::is_protected_process(p), "{p}");
        }
        for p in ["Safari", "Calculator", "com.apple.Safari", "Finder Helper"] {
            assert!(!super::is_protected_process(p), "{p}");
        }
    }
}
