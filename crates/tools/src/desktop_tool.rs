#[cfg(target_os = "windows")]
use crate::desktop_daemon::DesktopDaemon;
use crate::errors;
use crate::ax_native;
use crate::desktop_snapshot::{
    self, Rect, Snapshot, SnapshotStore, UIElement, assign_element_ids, delta_line, drop_report,
    generate_snapshot_id, image_to_screen, screen_point_now, screen_to_image,
};
#[cfg(target_os = "macos")]
use crate::desktop_snapshot::parse_ax_output;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use std::collections::HashMap;
#[cfg(target_os = "windows")]
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Desktop automation — windows, input, clipboard, notifications, screen capture,
/// UI accessibility, menus, dialogs, virtual desktops, shortcuts, TTS, and dock.
/// Cross-platform: macOS (AppleScript/native), Linux (xdotool/wmctrl/AT-SPI), Windows (PowerShell).
/// Persistent PowerShell daemon — shared across all desktop tool invocations (Windows only).
/// Initialized on first use, one per process. Not mutable global state — the internal
/// Mutex serializes all access.
#[cfg(target_os = "windows")]
static PS_DAEMON: std::sync::OnceLock<Arc<DesktopDaemon>> = std::sync::OnceLock::new();

#[cfg(target_os = "windows")]
fn ps_daemon() -> &'static Arc<DesktopDaemon> {
    PS_DAEMON.get_or_init(|| Arc::new(DesktopDaemon::new()))
}

/// Cached accessibility walk per app key: what the walk observed, its
/// elements, and when it ran.
type AxCache = std::sync::Mutex<HashMap<String, (AxCapture, Vec<UIElement>, Instant)>>;

pub struct DesktopTool {
    /// Serializes mouse + keyboard operations (one physical input device).
    input_lock: tokio::sync::Mutex<()>,
    /// Serializes clipboard read/write (single system clipboard).
    clipboard_lock: tokio::sync::Mutex<()>,
    snapshot_store: tokio::sync::Mutex<SnapshotStore>,
    ax_cache: AxCache,
}

impl DesktopTool {
    pub fn new() -> Self {
        Self {
            input_lock: tokio::sync::Mutex::new(()),
            clipboard_lock: tokio::sync::Mutex::new(()),
            snapshot_store: tokio::sync::Mutex::new(SnapshotStore::new()),
            ax_cache: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

impl DynTool for DesktopTool {
    fn name(&self) -> &str {
        "desktop"
    }

    fn description(&self) -> String {
        "Desktop automation — windows, input, clipboard, notifications, screen capture, \
         UI accessibility, menus, dialogs, virtual desktops, shortcuts, TTS, and dock.\n\n\
         Resources:\n\
         - window: list, focus, minimize, maximize, resize, close, move\n\
         - input: click (click_count/button), type, press, move, scroll (direction/amount), drag, paste\n\
         - clipboard: read, write, clear\n\
         - notification: send, alert\n\
         - capture: screenshot, see\n\
         - ui: tree, find, click, get_value, set_value, list_apps\n\
         - menu: list, menus, click, status, click_status\n\
         - dialog: detect, list, click, fill, dismiss\n\
         - space: list, switch, move_window\n\
         - shortcut: list, run\n\
         - tts: speak\n\
         - dock: badges, recent, is_running (macOS only)\n\n\
         Workflow: capture(action: see, app) returns the window as an image plus the elements it found, \
         with refs and positions in that image's pixels. Every input action on that app then returns the \
         window as it looks AFTER the action (image + what changed) — read it before the next step. \
         Act by ref when the element is listed, by image pixel (coordinate: [x, y]) when it is not; a pixel \
         click on an app you have not captured is refused, capture first. Text the accessibility tree does not \
         expose is read from the image and listed as OCRText elements, clickable by ref. \
         scroll(until: \"text\") pages until that text is on screen; drag(ref, to_ref | coordinate) reports where \
         the dragged element ended up.\n\n\
         Examples:\n  \
         os(resource: \"capture\", action: \"see\", app: \"Safari\") — snapshot + element IDs\n  \
         os(resource: \"input\", action: \"click\", ref: \"B3\") — click element from snapshot\n  \
         os(resource: \"input\", action: \"type\", ref: \"T1\", text: \"hello\") — focus + type\n  \
         os(resource: \"clipboard\", action: \"read\")\n  \
         os(resource: \"window\", action: \"list\")\n  \
         os(resource: \"capture\", action: \"screenshot\", app: \"Safari\")\n  \
         os(resource: \"notification\", action: \"send\", title: \"Done\", message: \"Task complete\")\n  \
         os(resource: \"tts\", action: \"speak\", text: \"Hello world\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "Desktop resource",
                    "enum": ["window", "input", "clipboard", "notification", "capture",
                             "ui", "menu", "dialog", "space", "shortcut", "tts", "dock"]
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform on the resource"
                },
                "app": { "type": "string", "description": "Application name (for window/capture/ui)" },
                "title": { "type": "string", "description": "Window or notification title" },
                "message": { "type": "string", "description": "Notification message" },
                "text": { "type": "string", "description": "Text to type, write to clipboard, or speak" },
                "key": { "type": "string", "description": "Key or combo to press (e.g. 'return', 'tab', 'cmd+shift+s')" },
                "x": { "type": "integer", "description": "X coordinate (window move; or an input target, same meaning as coordinate[0])" },
                "y": { "type": "integer", "description": "Y coordinate (window move; or an input target, same meaning as coordinate[1])" },
                "wait_ms": { "type": "integer", "description": "For input actions: how long to wait after acting before capturing the after-state (default 800, max 10000)" },
                "coordinate": { "type": "array", "items": { "type": "integer" }, "description": "[x, y] target for input actions (alternative to ref). With `app`: a pixel of the last capture(see) image of that app. Without `app`: screen points." },
                "start_coordinate": { "type": "array", "items": { "type": "integer" }, "description": "[x, y] drag start point" },
                "click_count": { "type": "integer", "description": "For input click: 1=single, 2=double. Default 1." },
                "button": { "type": "string", "description": "For input click: mouse button. Default left.", "enum": ["left", "right"] },
                "direction": { "type": "string", "description": "For input scroll: up/down/left/right", "enum": ["up", "down", "left", "right"] },
                "amount": { "type": "integer", "description": "For input scroll: ticks (~100px each, default 3)" },
                "width": { "type": "integer", "description": "Width for resize/move" },
                "height": { "type": "integer", "description": "Height for resize/move" },
                "region": { "type": "string", "description": "Region for screenshot: 'x,y,w,h'" },
                "quality": { "type": "string", "description": "Screenshot quality: 'low' (800px, 50% JPEG), 'medium' (1280px, 65% JPEG, default), 'high' (full-res PNG)" },
                "name": { "type": "string", "description": "Name for shortcut/menu/dialog element" },
                "value": { "type": "string", "description": "Value for set_value/fill" },
                "role": { "type": "string", "description": "UI element role filter (e.g. 'AXButton')" },
                "label": { "type": "string", "description": "UI element label/identifier (ui find: substring matched against every element's label, role and value)" },
                "index": { "type": "integer", "description": "Index for space/menu item" },
                "voice": { "type": "string", "description": "TTS voice name" },
                "rate": { "type": "integer", "description": "TTS speaking rate (words per minute)" },
                "ref": { "type": "string", "description": "Element ref from capture(action: see) (e.g. B1, T2)" },
                "snapshot_id": { "type": "string", "description": "Snapshot ID from a previous see action" },
                "max_elements": { "type": "integer", "description": "Max elements returned by see (default: 60)" },
                "until": { "type": "string", "description": "For input scroll: keep scrolling a page at a time until an element whose label contains this text is on screen (case-insensitive), up to max_pages" },
                "max_pages": { "type": "integer", "description": "For input scroll with until: page budget (default 8, max 30)" },
                "to_ref": { "type": "string", "description": "For input drag: the element to drop onto (from the last capture); alternative to coordinate" },
                "physical": { "type": "boolean", "description": "For input click/type: use the mouse and clipboard instead of accessibility. Off by default; an accessibility action that fails is reported, never silently replaced" }
            },
            "required": ["resource", "action"]
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
            let resource = input["resource"].as_str().unwrap_or("");
            let action = input["action"].as_str().unwrap_or("");

            match resource {
                "window" => {
                    let _guard = self.input_lock.lock().await;
                    handle_window(action, &input).await
                }
                "input" => {
                    let _guard = self.input_lock.lock().await;
                    handle_input(action, &input, &self.snapshot_store, &self.ax_cache).await
                }
                "clipboard" => {
                    let _guard = self.clipboard_lock.lock().await;
                    handle_clipboard(action, &input).await
                }
                "notification" => handle_notification(action, &input).await,
                "capture" => {
                    handle_capture(action, &input, &self.snapshot_store, &self.ax_cache).await
                }
                "ui" => {
                    let _guard = self.input_lock.lock().await;
                    handle_ui(action, &input, &self.snapshot_store, &self.ax_cache).await
                }
                "menu" => {
                    let _guard = self.input_lock.lock().await;
                    handle_menu(action, &input).await
                }
                "dialog" => {
                    let _guard = self.input_lock.lock().await;
                    handle_dialog(action, &input).await
                }
                "space" => handle_space(action, &input).await,
                "shortcut" => handle_shortcut(action, &input).await,
                "tts" => handle_tts(action, &input).await,
                "dock" => handle_dock(action, &input).await,
                _ => ToolResult::error(format!(
                    "Unknown resource '{}'. Use: window, input, clipboard, notification, capture, \
                     ui, menu, dialog, space, shortcut, tts, dock",
                    resource
                )),
            }
        })
    }
}

// --- Window management ---

async fn handle_window(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "list" => handle_window_list().await,
        "focus" => {
            let app = input["app"].as_str().unwrap_or("");
            if app.is_empty() {
                return ToolResult::error(errors::missing_param("focus", "app", "os(resource: \"window\", action: \"focus\", app: \"Safari\")"));
            }
            handle_window_focus(app).await
        }
        "minimize" => {
            let app = input["app"].as_str().unwrap_or("");
            if app.is_empty() {
                return ToolResult::error(errors::missing_param("minimize", "app", "os(resource: \"window\", action: \"minimize\", app: \"Safari\")"));
            }
            handle_window_minimize(app).await
        }
        "maximize" => {
            let app = input["app"].as_str().unwrap_or("");
            if app.is_empty() {
                return ToolResult::error(errors::missing_param("maximize", "app", "os(resource: \"window\", action: \"maximize\", app: \"Safari\")"));
            }
            handle_window_maximize(app).await
        }
        "resize" => {
            let app = input["app"].as_str().unwrap_or("");
            let w = input["width"].as_i64().unwrap_or(800);
            let h = input["height"].as_i64().unwrap_or(600);
            if app.is_empty() {
                return ToolResult::error(errors::missing_param("resize", "app", "os(resource: \"window\", action: \"resize\", app: \"Safari\", width: 1024, height: 768)"));
            }
            handle_window_resize(app, w, h).await
        }
        "close" => {
            let app = input["app"].as_str().unwrap_or("");
            if app.is_empty() {
                return ToolResult::error(errors::missing_param("close", "app", "os(resource: \"window\", action: \"close\", app: \"Safari\")"));
            }
            handle_window_close(app).await
        }
        "move" => {
            let app = input["app"].as_str().unwrap_or("");
            let x = input["x"].as_i64().unwrap_or(0);
            let y = input["y"].as_i64().unwrap_or(0);
            if app.is_empty() {
                return ToolResult::error(errors::missing_param("move", "app", "os(resource: \"window\", action: \"move\", app: \"Safari\", x: 100, y: 100)"));
            }
            handle_window_move(app, x, y).await
        }
        _ => ToolResult::error(format!(
            "Unknown window action '{}'. Use: list, focus, minimize, maximize, resize, close, move",
            action
        )),
    }
}

async fn handle_window_list() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
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
        let result = run_osascript(script).await;
        // System Events silently yields zero windows when this process lacks
        // Accessibility/Automation permission — say so instead of "no windows".
        if !result.is_error && result.content.trim().is_empty() {
            return ToolResult::error(
                "Window list empty. Either no visible windows exist or Accessibility \
                 permission is missing; open System Settings > Privacy & Security > \
                 Accessibility and enable Nebo, then retry.",
            );
        }
        return result;
    }
    #[cfg(target_os = "linux")]
    {
        if which("wmctrl") {
            return run_command("wmctrl", &["-l", "-G"]).await;
        }
        if which("xdotool") {
            return run_command("xdotool", &["search", "--onlyvisible", "--name", ""]).await;
        }
        return ToolResult::error(
            "Window list requires wmctrl or xdotool (install with your package manager)",
        );
    }
    #[cfg(target_os = "windows")]
    {
        let script = r#"Get-Process | Where-Object { $_.MainWindowTitle -ne '' } | ForEach-Object { "$($_.ProcessName) | $($_.MainWindowTitle) | $($_.Id)" }"#;
        return run_powershell(script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Window management is not supported on this platform")
}

async fn handle_window_focus(app: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"{}\" to activate",
            escape_applescript(app)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("wmctrl") {
            return run_command("wmctrl", &["-a", app]).await;
        }
        if which("xdotool") {
            let search = run_command_raw("xdotool", &["search", "--name", app]).await;
            if let Ok(wid) = search {
                let wid = wid.lines().next().unwrap_or("").trim();
                if !wid.is_empty() {
                    return run_command("xdotool", &["windowactivate", wid]).await;
                }
            }
            return ToolResult::error(format!("Window '{}' not found", app));
        }
        return ToolResult::error("Window focus requires wmctrl or xdotool");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type @"
using System; using System.Runtime.InteropServices;
public class Win {{ [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd); }}
"@
$p = Get-Process | Where-Object {{ $_.MainWindowTitle -match '{}' }} | Select-Object -First 1
if ($p) {{ [Win]::SetForegroundWindow($p.MainWindowHandle) }} else {{ Write-Error "Window '{}' not found" }}"#,
            escape_powershell(app),
            escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = app;
        ToolResult::error("Window focus is not supported on this platform")
    }
}

async fn handle_window_minimize(app: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"System Events\" to set miniaturized of first window of process \"{}\" to true",
            escape_applescript(app)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            let search = run_command_raw("xdotool", &["search", "--name", app]).await;
            if let Ok(wid) = search {
                let wid = wid.lines().next().unwrap_or("").trim();
                if !wid.is_empty() {
                    return run_command("xdotool", &["windowminimize", wid]).await;
                }
            }
            return ToolResult::error(format!("Window '{}' not found", app));
        }
        return ToolResult::error("Window minimize requires xdotool");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type @"
using System; using System.Runtime.InteropServices;
public class Win {{ [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow); }}
"@
$p = Get-Process | Where-Object {{ $_.MainWindowTitle -match '{}' }} | Select-Object -First 1
if ($p) {{ [Win]::ShowWindow($p.MainWindowHandle, 6) }} else {{ Write-Error "Window '{}' not found" }}"#,
            escape_powershell(app),
            escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = app;
        ToolResult::error("Window minimize is not supported on this platform")
    }
}

async fn handle_window_maximize(app: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"System Events\"\n\
             set theWindow to first window of process \"{}\"\n\
             set position of theWindow to {{0, 25}}\n\
             set size of theWindow to {{1920, 1055}}\n\
             end tell",
            escape_applescript(app)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("wmctrl") {
            return run_command(
                "wmctrl",
                &["-r", app, "-b", "add,maximized_vert,maximized_horz"],
            )
            .await;
        }
        if which("xdotool") {
            let search = run_command_raw("xdotool", &["search", "--name", app]).await;
            if let Ok(wid) = search {
                let wid = wid.lines().next().unwrap_or("").trim();
                if !wid.is_empty() {
                    return run_command("xdotool", &["windowsize", wid, "100%", "100%"]).await;
                }
            }
            return ToolResult::error(format!("Window '{}' not found", app));
        }
        return ToolResult::error("Window maximize requires wmctrl or xdotool");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type @"
using System; using System.Runtime.InteropServices;
public class Win {{ [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow); }}
"@
$p = Get-Process | Where-Object {{ $_.MainWindowTitle -match '{}' }} | Select-Object -First 1
if ($p) {{ [Win]::ShowWindow($p.MainWindowHandle, 3) }} else {{ Write-Error "Window '{}' not found" }}"#,
            escape_powershell(app),
            escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = app;
        ToolResult::error("Window maximize is not supported on this platform")
    }
}

async fn handle_window_resize(app: &str, w: i64, h: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"System Events\" to set size of first window of process \"{}\" to {{{}, {}}}",
            escape_applescript(app),
            w,
            h
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            let search = run_command_raw("xdotool", &["search", "--name", app]).await;
            if let Ok(wid) = search {
                let wid = wid.lines().next().unwrap_or("").trim();
                if !wid.is_empty() {
                    let ws = w.to_string();
                    let hs = h.to_string();
                    return run_command("xdotool", &["windowsize", wid, &ws, &hs]).await;
                }
            }
            return ToolResult::error(format!("Window '{}' not found", app));
        }
        return ToolResult::error("Window resize requires xdotool");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type @"
using System; using System.Runtime.InteropServices;
public class Win {{ [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr hWnd, int X, int Y, int nWidth, int nHeight, bool bRepaint); }}
"@
$p = Get-Process | Where-Object {{ $_.MainWindowTitle -match '{}' }} | Select-Object -First 1
if ($p) {{ [Win]::MoveWindow($p.MainWindowHandle, 0, 0, {}, {}, $true) }} else {{ Write-Error "Window '{}' not found" }}"#,
            escape_powershell(app),
            w,
            h,
            escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (app, w, h);
        ToolResult::error("Window resize is not supported on this platform")
    }
}

async fn handle_window_close(app: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"System Events\" to click button 1 of first window of process \"{}\"",
            escape_applescript(app)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            let search = run_command_raw("xdotool", &["search", "--name", app]).await;
            if let Ok(wid) = search {
                let wid = wid.lines().next().unwrap_or("").trim();
                if !wid.is_empty() {
                    return run_command("xdotool", &["key", "--window", wid, "alt+F4"]).await;
                }
            }
            return ToolResult::error(format!("Window '{}' not found", app));
        }
        if which("wmctrl") {
            return run_command("wmctrl", &["-c", app]).await;
        }
        return ToolResult::error("Window close requires xdotool or wmctrl");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"$p = Get-Process | Where-Object {{ $_.MainWindowTitle -match '{}' }} | Select-Object -First 1
if ($p) {{ $p.CloseMainWindow() }} else {{ Write-Error "Window '{}' not found" }}"#,
            escape_powershell(app),
            escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = app;
        ToolResult::error("Window close is not supported on this platform")
    }
}

async fn handle_window_move(app: &str, x: i64, y: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"System Events\" to set position of first window of process \"{}\" to {{{}, {}}}",
            escape_applescript(app),
            x,
            y
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            let search = run_command_raw("xdotool", &["search", "--name", app]).await;
            if let Ok(wid) = search {
                let wid = wid.lines().next().unwrap_or("").trim();
                if !wid.is_empty() {
                    let xs = x.to_string();
                    let ys = y.to_string();
                    return run_command("xdotool", &["windowmove", wid, &xs, &ys]).await;
                }
            }
            return ToolResult::error(format!("Window '{}' not found", app));
        }
        if which("wmctrl") {
            let geom = format!("0,{},{},−1,−1", x, y);
            return run_command("wmctrl", &["-r", app, "-e", &geom]).await;
        }
        return ToolResult::error("Window move requires xdotool or wmctrl");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type @"
using System; using System.Runtime.InteropServices;
public class Win {{ [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr hWnd, int X, int Y, int nWidth, int nHeight, bool bRepaint);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
[StructLayout(LayoutKind.Sequential)] public struct RECT {{ public int Left, Top, Right, Bottom; }} }}
"@
$p = Get-Process | Where-Object {{ $_.MainWindowTitle -match '{}' }} | Select-Object -First 1
if ($p) {{ $r = New-Object Win+RECT; [Win]::GetWindowRect($p.MainWindowHandle, [ref]$r);
[Win]::MoveWindow($p.MainWindowHandle, {}, {}, ($r.Right - $r.Left), ($r.Bottom - $r.Top), $true) }} else {{ Write-Error "Window '{}' not found" }}"#,
            escape_powershell(app),
            x,
            y,
            escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (app, x, y);
        ToolResult::error("Window move is not supported on this platform")
    }
}

// --- Input simulation ---

/// Parse a `[x, y]` coordinate array param.
fn coord(input: &serde_json::Value, key: &str) -> Option<(i64, i64)> {
    let arr = input.get(key)?.as_array()?;
    if arr.len() >= 2 {
        Some((arr[0].as_i64()?, arr[1].as_i64()?))
    } else {
        None
    }
}

/// The target an input action names. `ref` (an element from capture see)
/// and `coordinate: [x, y]` are the primary names, the same shape as the web
/// browser tool. `element_id` and `element` are read as `ref`, and `x`/`y`
/// integers as the coordinate: the straps and the os description taught
/// those spellings, and a model that follows them must still hit the
/// element (2026-09-05 audit: no documented click shape reached one).
fn input_target(input: &serde_json::Value) -> (&str, Option<(i64, i64)>) {
    let element_ref = ["ref", "element_id", "element"]
        .iter()
        .filter_map(|k| input[*k].as_str())
        .find(|s| !s.is_empty())
        .unwrap_or("");
    let coordinate = coord(input, "coordinate").or_else(|| {
        match (input["x"].as_i64(), input["y"].as_i64()) {
            (Some(x), Some(y)) => Some((x, y)),
            _ => None,
        }
    });
    (element_ref, coordinate)
}

/// Jev (TypeSafe through Janus), when the server installed it: picks the
/// element an input action means from the elements that can take it.
static DECIDER: std::sync::OnceLock<std::sync::Arc<ai::DecideClient>> = std::sync::OnceLock::new();
/// Jev's misses per capture: two on one screen and the model drives until
/// the screen changes.
static JEV_MISSES: std::sync::Mutex<Vec<(String, u8)>> = std::sync::Mutex::new(Vec::new());

/// Recent acts and the screen each left behind: (app + action + target,
/// fingerprint of the after-state). A toggle loop — click the title, a
/// popover opens; click the text, it closes; again — never repeats a result
/// byte for byte (every capture has a new id), so the runner's repeat guard
/// cannot see it. The same act producing the same screen three times is
/// refused the fourth (Stadium, 2026-09-24: 20+ alternating clicks).
static RECENT_ACTS: std::sync::Mutex<Vec<(String, u64)>> = std::sync::Mutex::new(Vec::new());

fn screen_fingerprint(snap: &Snapshot) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut parts: Vec<String> = snap
        .elements
        .iter()
        .map(|e| format!("{}|{}|{}", e.role, e.label, e.value.as_deref().unwrap_or("")))
        .collect();
    parts.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    snap.frame.as_ref().map(|f| (f.x, f.y, f.width, f.height)).hash(&mut h);
    h.finish()
}

/// How many of the last acts were this act and left this same screen.
fn circling(act: &str, fingerprint: Option<u64>) -> usize {
    let Ok(acts) = RECENT_ACTS.lock() else { return 0 };
    acts.iter().rev().take(10).filter(|(a, f)| a == act && fingerprint.map_or(true, |fp| *f == fp)).count()
}

fn remember_act(act: String, fingerprint: u64) {
    if let Ok(mut acts) = RECENT_ACTS.lock() {
        acts.push((act, fingerprint));
        let n = acts.len();
        if n > 20 {
            acts.drain(..n - 20);
        }
    }
}

/// Installed once at server boot.
pub fn set_decider(client: std::sync::Arc<ai::DecideClient>) {
    let _ = DECIDER.set(client);
}

/// What an element offers, for the choice list Jev reads.
fn describe_for_pick(e: &UIElement) -> String {
    let mut d = format!("{} \"{}\"", e.role, e.label);
    if let Some(v) = &e.value {
        d.push_str(&format!(" = \"{}\"", v.chars().take(60).collect::<String>()));
    }
    if e.path.starts_with("m:") {
        d.push_str(" (menu item)");
    }
    d
}

/// The elements of `snap` that `action` can act on: pressable for a click,
/// editable for type, anything with a context menu for a right-click.
fn pick_candidates<'a>(action: &str, snap: &'a Snapshot) -> Vec<&'a UIElement> {
    snap.elements
        .iter()
        .filter(|e| e.actionable && (!e.label.is_empty() || e.value.is_some()))
        .filter(|e| match action {
            "type" => e.actions.iter().any(|a| a == "AXSetValue") || is_editable_role(&e.role),
            "right_click" => e.actions.iter().any(|a| a == "AXShowMenu" || a == "AXPress"),
            _ => e.actions.is_empty() || e.actions.iter().any(|a| a == "AXPress" || a == "AXShowMenu"),
        })
        .take(60)
        .collect()
}

/// Resolve `target` ("the Save button") to one element of `snap`. A single
/// element carrying exactly that label needs no model; otherwise Jev picks,
/// accepted at confidence 0.7 or more. Every other outcome falls through to
/// the model with the reason, and nothing moves.
async fn jev_pick(action: &str, target: &str, snap: &Snapshot) -> Result<(String, String), String> {
    let candidates = pick_candidates(action, snap);
    if candidates.is_empty() {
        return Err("this screen has no labelled elements for that action (read by vision); pick a pixel from the image".into());
    }
    let want = target.trim().trim_start_matches("the ").to_lowercase();
    let exact: Vec<&&UIElement> = candidates.iter().filter(|e| e.label.to_lowercase() == want).collect();
    if exact.len() == 1 {
        return Ok((exact[0].id.clone(), format!("{} is the one element labelled \"{}\"", exact[0].id, exact[0].label)));
    }
    let misses = JEV_MISSES.lock().ok().and_then(|m| m.iter().find(|(id, _)| *id == snap.id).map(|(_, n)| *n)).unwrap_or(0);
    if misses >= 2 {
        return Err("Jev missed twice on this screen; choose the ref yourself until the screen changes".into());
    }
    let Some(client) = DECIDER.get() else {
        return Err("Jev is not available here; choose the ref yourself".into());
    };
    let described: Vec<(String, String)> = candidates.iter().map(|e| (e.id.clone(), describe_for_pick(e))).collect();
    let mut criteria: Vec<(&str, &str)> = described.iter().map(|(id, d)| (id.as_str(), d.as_str())).collect();
    criteria.push(("none", "no listed element is the one meant"));
    let instructions = format!(
        "On the {} screen, which element should a {action} act on to do this: {target}? Answer none when no listed element is clearly it.",
        snap.app.as_deref().unwrap_or("current")
    );
    let questions: std::collections::BTreeMap<&str, ai::Question> =
        [("element", ai::Question::choice(&instructions, &criteria))].into_iter().collect();
    let state = serde_json::json!({ "app": snap.app, "action": action, "target": target });
    let trace = ai::RequestTrace::new("desktop_pick");
    let miss = |why: String| {
        if let Ok(mut m) = JEV_MISSES.lock() {
            match m.iter_mut().find(|(id, _)| *id == snap.id) {
                Some((_, n)) => *n += 1,
                None => m.push((snap.id.clone(), 1)),
            }
            let len = m.len();
            if len > 32 {
                m.drain(..len - 32);
            }
        }
        why
    };
    let decision = client.decide(&trace, &state, &questions).await.map_err(|e| miss(format!("Jev did not answer ({e}); choose the ref yourself")))?;
    let Some(ans) = decision.answer("element") else {
        return Err(miss("Jev returned no choice; choose the ref yourself".into()));
    };
    let picked = ans.picked().to_string();
    let conf = ans.confidence.unwrap_or(0.0);
    let mut top: Vec<(&String, &f64)> = ans.probabilities.iter().collect();
    top.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
    let top = top.iter().take(3).map(|(k, p)| format!("{k} {:.0}%", *p * 100.0)).collect::<Vec<_>>().join(", ");
    if picked == "none" || picked.is_empty() || conf < 0.7 || !candidates.iter().any(|e| e.id == picked) {
        return Err(miss(format!("Jev was not sure which element is \"{target}\" ({top}); choose the ref yourself")));
    }
    Ok((picked.clone(), format!("Jev picked {picked} for \"{target}\" ({:.0}% sure)", conf * 100.0)))
}

/// The capture an input action's coordinates refer to: the one named by
/// `snapshot_id`, else the latest of `app`, else the latest of anything.
async fn snapshot_for(
    store: &tokio::sync::Mutex<SnapshotStore>,
    snapshot_id: &str,
    app: &str,
) -> Option<Snapshot> {
    let store = store.lock().await;
    if !snapshot_id.is_empty() {
        store.get(snapshot_id).cloned()
    } else if !app.is_empty() {
        store.latest_for(app).cloned()
    } else {
        store.latest().cloned()
    }
}

/// One input action, then the window as it looks after it. The point is
/// resolved from what the model last SAW (a ref, or a pixel of that image)
/// against where the window IS now; the result carries the next image, so
/// acting blind is not a state this tool can be in.
async fn handle_input(
    action: &str,
    input: &serde_json::Value,
    snapshot_store: &tokio::sync::Mutex<SnapshotStore>,
    ax_cache: &AxCache,
) -> ToolResult {
    if action == "paste" {
        return input_paste().await;
    }
    // `target` in words instead of a ref: resolved to one element of the
    // last capture, or handed back to the model with the reason.
    let picked_input;
    let mut pick_note = String::new();
    let input = match input["target"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
        Some(target) if input_target(input).0.is_empty() && input_target(input).1.is_none() => {
            let app = input["app"].as_str().unwrap_or("");
            let Some(snap) = snapshot_for(snapshot_store, input["snapshot_id"].as_str().unwrap_or(""), app).await else {
                return ToolResult::error(format!("{action}: target needs a capture to pick from; capture {} first", if app.is_empty() { "the app" } else { app }));
            };
            match jev_pick(action, target, &snap).await {
                Ok((id, note)) => {
                    let mut v = input.clone();
                    v["ref"] = serde_json::json!(id);
                    picked_input = v;
                    pick_note = format!(" ({note})");
                    &picked_input
                }
                Err(why) => return ToolResult::error(format!("Not delivered (safe to retry): {why}. Nothing was touched.")),
            }
        }
        _ => input,
    };
    let (element_ref, coordinate) = input_target(input);
    // The act as a key, for the circling guard: the same thing done to the same target.
    // Everything that says what the act does (direction, amount, text, key…);
    // not what only says how to look afterwards.
    let act_key = {
        let mut v = input.clone();
        if let Some(o) = v.as_object_mut() {
            for k in ["snapshot_id", "wait_ms", "wait_for", "quality", "max_elements"] {
                o.remove(k);
            }
        }
        format!("{action}|{v}")
    };
    let repeats = circling(&act_key, None);
    if repeats >= 3 {
        let same_screen = RECENT_ACTS
            .lock()
            .ok()
            .and_then(|a| a.iter().rev().find(|(k, _)| *k == act_key).map(|(_, f)| *f))
            .map_or(0, |fp| circling(&act_key, Some(fp)));
        if same_screen >= 3 {
            return ToolResult::error(format!(
                "Not delivered: this {action} has left the same screen {same_screen} times in the last few acts — it is going in circles. Stop repeating it: read the last capture, and use a different way (a menu: os(resource: \"menu\", action: \"click\", name: \"Menu > Item\"), a key, or a different element), or tell the user what is in the way."
            ));
        }
    }
    let snapshot_id = input["snapshot_id"].as_str().unwrap_or("");
    let app_arg = input["app"].as_str().unwrap_or("").trim().to_string();
    let snap = snapshot_for(snapshot_store, snapshot_id, &app_arg).await;
    let app = if !app_arg.is_empty() {
        app_arg
    } else {
        snap.as_ref().and_then(|s| s.app.clone()).unwrap_or_default()
    };

    let element: Option<UIElement> = if element_ref.is_empty() {
        None
    } else {
        match snap.as_ref().and_then(|s| s.elements.iter().find(|e| e.id == element_ref)) {
            Some(e) => Some(e.clone()),
            None => {
                return ToolResult::error(format!(
                    "Element '{}' is not in the last capture{}. Call os(resource: \"capture\", action: \"see\"{}) and use a ref from that list.",
                    element_ref,
                    if app.is_empty() { String::new() } else { format!(" of {app}") },
                    if app.is_empty() { String::new() } else { format!(", app: \"{app}\"") },
                ))
            }
        }
    };

    // A pixel is only meaningful against an image of that app. Refused
    // before anything moves: the fix is one capture, and the message says so.
    if element.is_none() && coordinate.is_some() && !app.is_empty()
        && !snap.as_ref().map_or(false, |s| s.frame.is_some())
    {
        let (x, y) = coordinate.unwrap();
        return ToolResult::error(format!(
            "{action}: no capture of {app} to read ({x},{y}) against. Call os(resource: \"capture\", action: \"see\", app: \"{app}\") first, then act by ref or by a pixel of that image."
        ));
    }

    // Where the window is NOW, brought to the front. The native helper
    // answers from the accessibility API; System Events is the fallback, and
    // it needs an Automation grant the app may not have (Stadium, 2026-09-23:
    // every AppleEvent hung two minutes waiting for a consent nobody could
    // click). Acting must not depend on it.
    let now: Option<Rect> = if app.is_empty() {
        None
    } else {
        match ax_native::window(&app, 1).await {
            Ok(w) => {
                let _ = ax_native::raise(&app, 1).await;
                Some(Rect { x: w.frame[0], y: w.frame[1], width: w.frame[2], height: w.frame[3] })
            }
            // The helper answered: there is nothing to act on. System Events
            // would only say the same thing worse ("Can't set process to true").
            Err(e) if is_no_window(&e) => {
                return ToolResult::error(format!("{action}: {e}; capture again once a window is back"));
            }
            Err(_) => match window_frame(&app, true).await {
                Ok(r) => Some(r),
                Err(e) if cfg!(target_os = "macos") => {
                    return ToolResult::error(format!("{action}: {e}"));
                }
                Err(_) => snap.as_ref().and_then(|s| s.frame.clone()),
            },
        }
    };

    // The target in screen points, with the label the model used for it.
    let to_screen = |px: (i64, i64)| -> Result<(i64, i64), String> {
        if app.is_empty() {
            return Ok(px);
        }
        match (&snap, &now) {
            (Some(s), Some(now)) if s.frame.is_some() => {
                image_to_screen(px, s.frame.as_ref().unwrap(), now, s.scale)
            }
            _ => Err(format!("no capture of {app} to read ({},{}) against", px.0, px.1)),
        }
    };
    let target: Option<(i64, i64, String)> = if let Some(e) = &element {
        let c = e.bounds.center();
        let p = match (snap.as_ref().and_then(|s| s.frame.as_ref()), &now) {
            (Some(then), Some(now)) => match screen_point_now(c, then, now) {
                Ok(p) => p,
                Err(err) => return ToolResult::error(format!("{action}: {err}")),
            },
            _ => c,
        };
        Some((p.0, p.1, format!("{} \"{}\"", e.id, e.label)))
    } else if let Some(px) = coordinate {
        match to_screen(px) {
            Ok(p) => Some((p.0, p.1, format!("({},{})", px.0, px.1))),
            Err(err) => return ToolResult::error(format!("{action}: {err}")),
        }
    } else {
        None
    };

    // A drag remembers what it moved and where it dropped, for the after-state.
    let mut drop_check: Option<(String, (i64, i64))> = None;
    let mut hit_note = String::new();
    let performed: ToolResult = 'performed: { match action {
        "type" => {
            let text = input["text"].as_str().unwrap_or("");
            if text.is_empty() {
                return ToolResult::error(errors::missing_param("type", "text", "os(resource: \"input\", action: \"type\", text: \"hello\")"));
            }
            // A field that accepts AXSetValue gets the text set and read back;
            // anything else gets the text pasted (one keystroke per character
            // drops characters and loses capitals). The ref is re-identified
            // before either.
            let physical = input["physical"].as_bool().unwrap_or(false);
            if !physical {
                if let Some(e) = element.as_ref().filter(|e| !e.path.is_empty() && e.actions.iter().any(|a| a == "AXSetValue")) {
                    // Through to the after-state like every act: the result shows the field.
                    break 'performed match ax_native::set_value(&app, 1, &e.path, text, Some((&e.role, &e.label))).await {
                        Ok(()) => ToolResult::ok(format!("Set {} \"{}\" to {} chars via accessibility (read back)", e.id, e.label, text.chars().count())),
                        Err(err) => return ToolResult::error(format!(
                            "Not delivered (safe to retry): setting {} \"{}\" through accessibility failed: {err}. Nothing was typed. \
                             Capture again and use a current ref, or pass physical: true to click it and paste."
                        , e.id, e.label)),
                    };
                }
            }
            if let Some((x, y, label)) = &target {
                if element.is_some() && !app.is_empty() {
                    let _ = ax_native::raise(&app, 1).await;
                }
                let click_result = input_click(*x, *y).await;
                if click_result.is_error {
                    return click_result;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                let r = paste_text(text).await;
                if r.is_error {
                    return ToolResult::error(format!("Clicked {label} but pasting failed: {}", r.content));
                }
                ToolResult::ok(format!("Clicked {label} and pasted {} chars", text.chars().count()))
            } else {
                let r = paste_text(text).await;
                if r.is_error {
                    return r;
                }
                ToolResult::ok(format!("Pasted {} chars into the focused field", text.chars().count()))
            }
        }
        "press" | "hotkey" => {
            // `keys` as a combo string or a list (["cmd", "a"]) is the same key.
            let joined;
            let key = match input["key"].as_str().filter(|k| !k.is_empty()) {
                Some(k) => k,
                None => {
                    joined = combo_from(&input["keys"]);
                    joined.as_str()
                }
            };
            if key.is_empty() {
                return ToolResult::error(errors::missing_param(action, "key", "os(resource: \"input\", action: \"hotkey\", key: \"cmd+a\")"));
            }
            // Combos that end the session or force-quit are never sent by a tool.
            if blocked_combo(key) && !input["force"].as_bool().unwrap_or(false) {
                return ToolResult::error(format!(
                    "Not delivered: {key} logs out, locks the screen or force-quits, and a tool does not send it. If the owner asked for exactly that, pass force: true."
                ));
            }
            let r = if key.contains('+') { input_hotkey(key).await } else { input_press(key).await };
            if r.is_error {
                return r;
            }
            ToolResult::ok(format!("Pressed {key}"))
        }
        "click" | "double_click" | "right_click" => {
            let Some((x, y, label)) = &target else {
                return ToolResult::error(
                    "click requires `ref` (from capture see) or `coordinate: [x, y]`.",
                );
            };
            let click_count = if action == "double_click" {
                2
            } else {
                input["click_count"]
                    .as_u64()
                    .or_else(|| input["click_count"].as_str().and_then(|s| s.trim().parse().ok()))
                    .unwrap_or(1)
                    .clamp(1, 3)
            };
            let button = if action == "right_click" { "right" } else { input["button"].as_str().unwrap_or("left") };
            // A plain click on an element that accepts AXPress is pressed
            // through accessibility, after the element is re-identified in
            // the live tree: no pointer travel, and it works on an element
            // the pointer could not reach. A press that fails is reported,
            // not replaced by a mouse click on a point that may now hold
            // something else; the mouse is opt-in (`physical: true`).
            let physical = input["physical"].as_bool().unwrap_or(false);
            if click_count == 1 && button == "left" && !physical {
                if let Some(e) = element.as_ref().filter(|e| !e.path.is_empty() && e.actions.iter().any(|a| a == "AXPress")) {
                    break 'performed match ax_native::act(&app, 1, &e.path, "AXPress", Some((&e.role, &e.label))).await {
                        Ok(()) => ToolResult::ok(format!("Pressed {label} via accessibility")),
                        Err(err) => return ToolResult::error(format!(
                            "Not delivered (safe to retry): the press of {label} through accessibility failed: {err}. Nothing was clicked. \
                             Capture again and use a current ref, or pass physical: true to click its point with the mouse."
                        )),
                    };
                }
            }
            // A right-click on an element that offers a context menu opens it
            // through accessibility: proof is a menu that was not open before,
            // and its items come back listed as refs.
            if button == "right" && !physical {
                if let Some(e) = element.as_ref().filter(|e| !e.path.is_empty() && e.actions.iter().any(|a| a == "AXShowMenu")) {
                    let mut args = vec!["show-menu".to_string(), "--app".into(), app.clone(), "--path".into(), e.path.clone()];
                    if !e.role.is_empty() {
                        args.extend(["--role".into(), e.role.clone(), "--label".into(), e.label.clone()]);
                    }
                    match ax_native::run(&args, Duration::from_secs(8)).await {
                        Ok(_) => break 'performed ToolResult::ok(format!("Opened the context menu of {label} via accessibility; its items are listed first below")),
                        Err(err) if err.contains("helper unavailable") => {}
                        Err(err) => return ToolResult::error(format!("Right-click of {label}: {err}")),
                    }
                }
            }
            // Physical input lands on whatever window is on top at the point:
            // bring the element's own window forward first, and the element
            // itself into view — a click on a clipped element hits whatever is
            // drawn over it.
            let mut target_xy = (*x, *y);
            if let Some(e) = element.as_ref().filter(|e| !e.path.is_empty() && !app.is_empty()) {
                let _ = ax_native::raise(&app, 1).await;
                let mut args = vec!["scroll-to".to_string(), "--app".into(), app.clone(), "--path".into(), e.path.clone()];
                if !e.role.is_empty() {
                    args.extend(["--role".into(), e.role.clone(), "--label".into(), e.label.clone()]);
                }
                if let Ok(out) = ax_native::run(&args, Duration::from_secs(5)).await {
                    let moved = ax_native::json_lines(&out).first().map_or(false, |v| v["steps"].as_u64().unwrap_or(0) > 0);
                    if moved {
                        // It moved: aim at where it is now, not where it was.
                        if let Ok(t) = ax_native::tree(&app, &ax_native::WalkOpts::default()).await {
                            if let Some(n) = t.nodes.iter().find(|n| n.role == e.role && element_from_node(n).label == e.label) {
                                target_xy = (n.frame[0] + n.frame[2] / 2, n.frame[1] + n.frame[3] / 2);
                            }
                        }
                    }
                }
            } else if !app.is_empty() {
                // A pixel of the image: say what is under it, and refuse when
                // another app's window covers the point — the click would land there.
                if let Ok(out) = ax_native::run(&["hit".to_string(), "--x".into(), x.to_string(), "--y".into(), y.to_string()], Duration::from_secs(3)).await {
                    if let Some(v) = ax_native::json_lines(&out).first() {
                        let owner = v["app"].as_str().unwrap_or("");
                        if !owner.is_empty() && !owner.eq_ignore_ascii_case(&app) && owner != "Dock" && owner != "SystemUIServer" && !physical {
                            return ToolResult::error(format!(
                                "Not delivered (safe to retry): {label} is covered by {owner}'s {} \"{}\" — the click would land there. Capture {app} again, or pass physical: true to click that point anyway.",
                                v["role"].as_str().unwrap_or("window"), v["label"].as_str().unwrap_or("")
                            ));
                        }
                        let role = v["role"].as_str().unwrap_or("");
                        if !role.is_empty() {
                            hit_note = format!(" (on {role} \"{}\")", v["label"].as_str().unwrap_or(""));
                        }
                    }
                }
            }
            let (x, y) = (&target_xy.0, &target_xy.1);
            let (r, how) = match (click_count, button) {
                (_, "right") => (input_right_click(*x, *y).await, "Right-clicked"),
                (2, _) => (input_double_click(*x, *y).await, "Double-clicked"),
                _ => (input_click(*x, *y).await, "Clicked"),
            };
            if r.is_error {
                return r;
            }
            ToolResult::ok(format!("{how} {label}{hit_note} at screen ({x},{y})"))
        }
        "move" => {
            let Some((x, y, label)) = &target else {
                return ToolResult::error("move requires `ref` or `coordinate: [x, y]`.");
            };
            let r = input_move(*x, *y).await;
            if r.is_error {
                return r;
            }
            ToolResult::ok(format!("Moved the pointer to {label}"))
        }
        "scroll" => {
            // Web-parity shape: direction + amount (ticks, ~100px each).
            // A number or a numeric string ("30"): models send both.
            let amount = input["amount"]
                .as_i64()
                .or_else(|| input["amount"].as_str().and_then(|s| s.trim().parse().ok()))
                .unwrap_or(3)
                .clamp(1, 100);
            let step = 100;
            let direction = input["direction"].as_str().unwrap_or("down");
            let (dx, dy) = match direction {
                "up" => (0, -amount * step),
                "left" => (-amount * step, 0),
                "right" => (amount * step, 0),
                _ => (0, amount * step),
            };
            // The wheel turns under the pointer: over the target, or over the
            // app's window — not wherever the pointer was left (Stadium,
            // 2026-09-24: a scroll with no target moved nothing in TextEdit).
            if let Some((x, y, _)) = &target {
                let _ = input_move(*x, *y).await;
            } else if let Some(w) = &now {
                let _ = input_move(w.x + w.width / 2, w.y + w.height / 2).await;
            }
            let until = input["until"].as_str().unwrap_or("").trim().to_lowercase();
            if until.is_empty() {
                let r = input_scroll(dx, dy).await;
                if r.is_error {
                    return r;
                }
                ToolResult::ok(format!("Scrolled {direction} {amount} ticks"))
            } else {
                // Scroll a page at a time until an element whose label carries
                // the text is on screen, or the page budget runs out. Each page
                // is a fresh observe, so the match is what is visible NOW.
                let max_pages = input["max_pages"].as_u64().unwrap_or(8).clamp(1, 30) as usize;
                // On screen means visible: the helper reads each element's
                // visible text (a text area's visible character range, not its
                // whole value); labels of the capture are the fallback.
                let label_visible = |snap: &Snapshot| snap.elements.iter().any(|e| e.label.to_lowercase().contains(&until));
                let helper_says = |app: String, text: String| async move {
                    let args = vec!["wait".to_string(), "--app".into(), app, "--for".into(), "text".into(), "--text".into(), text, "--timeout-ms".into(), "0".into()];
                    match ax_native::run(&args, Duration::from_secs(4)).await {
                        Ok(_) => Some(true),
                        Err(e) if e.starts_with("wait_timeout") => Some(false),
                        Err(_) => None,
                    }
                };
                let mut pages = 0;
                let mut found = match helper_says(app.clone(), until.clone()).await {
                    Some(v) => v,
                    None => snap.as_ref().map_or(false, |s| label_visible(s)),
                };
                while !found && pages < max_pages {
                    let r = input_scroll(dx, dy).await;
                    if r.is_error {
                        return r;
                    }
                    pages += 1;
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    if let Ok(mut guard) = ax_cache.lock() {
                        guard.clear();
                    }
                    found = match helper_says(app.clone(), until.clone()).await {
                        Some(v) => v,
                        None => match observe(&app, &serde_json::json!({ "app": app, "quality": input["quality"] }), snapshot_store, ax_cache).await {
                            Ok(o) => label_visible(&o.snapshot),
                            Err(e) => return e,
                        },
                    };
                }
                if found {
                    ToolResult::ok(format!("Scrolled {direction} {pages} page(s); \"{}\" is on screen — find it in the list below", input["until"].as_str().unwrap_or("")))
                } else {
                    ToolResult::ok(format!("Scrolled {direction} {pages} page(s) and \"{}\" did not appear; it may be elsewhere or worded differently", input["until"].as_str().unwrap_or("")))
                }
            }
        }
        "drag" => {
            // What to drag: `ref` (the element) or `start_coordinate`; where
            // to: `to_ref` (an element) or `coordinate`.
            let start: Option<((i64, i64), String)> = if let Some(e) = &element {
                match (snap.as_ref().and_then(|s| s.frame.as_ref()), &now) {
                    (Some(then), Some(now)) => match screen_point_now(e.bounds.center(), then, now) {
                        Ok(p) => Some((p, e.label.clone())),
                        Err(err) => return ToolResult::error(format!("drag: {err}")),
                    },
                    _ => Some((e.bounds.center(), e.label.clone())),
                }
            } else if let Some(px) = coord(input, "start_coordinate") {
                match to_screen(px) {
                    Ok(p) => Some((p, String::new())),
                    Err(err) => return ToolResult::error(format!("drag: {err}")),
                }
            } else {
                None
            };
            let to_ref = input["to_ref"].as_str().unwrap_or("");
            let end: Option<((i64, i64), (i64, i64))> = if !to_ref.is_empty() {
                let Some(e) = snap.as_ref().and_then(|s| s.elements.iter().find(|e| e.id == to_ref)) else {
                    return ToolResult::error(format!("drag: to_ref '{to_ref}' is not in the last capture; capture again and use a ref from that list."));
                };
                let c = e.bounds.center();
                let p = match (snap.as_ref().and_then(|s| s.frame.as_ref()), &now) {
                    (Some(then), Some(now)) => match screen_point_now(c, then, now) {
                        Ok(p) => p,
                        Err(err) => return ToolResult::error(format!("drag: {err}")),
                    },
                    _ => c,
                };
                let px = match (snap.as_ref().and_then(|s| s.frame.as_ref()), snap.as_ref()) {
                    (Some(f), Some(s)) => screen_to_image(c, f, s.scale),
                    _ => c,
                };
                Some((p, px))
            } else if let Some(px) = coordinate {
                match to_screen(px) {
                    Ok(p) => Some((p, px)),
                    Err(err) => return ToolResult::error(format!("drag: {err}")),
                }
            } else {
                None
            };
            let (Some(((x, y), label)), Some(((x2, y2), end_px))) = (start, end) else {
                return ToolResult::error(
                    "drag needs what to drag (`ref`, or `start_coordinate: [x, y]`) and where to (`to_ref`, or `coordinate: [x, y]`).",
                );
            };
            let r = input_drag(x, y, x2, y2).await;
            if r.is_error {
                return r;
            }
            let what = if label.is_empty() { format!("({x},{y})") } else { format!("\"{label}\"") };
            drop_check = Some((label, end_px));
            ToolResult::ok(format!("Dragged {what} to ({},{})", end_px.0, end_px.1))
        }
        _ => {
            return ToolResult::error(format!(
                "Unknown input action '{}'. Use: click, double_click, right_click, type, press, hotkey, move, scroll, drag, paste",
                action
            ))
        }
    } };

    // The after-state: the same target, captured fresh — after the thing the
    // caller said to wait for, or after a fixed pause.
    let wait_note = match wait_for_args(&app, input) {
        Some(args) => match ax_native::run(&args, Duration::from_millis(wait_for_timeout(input) + 2_000)).await {
            Ok(out) => {
                let ms = ax_native::json_lines(&out).first().and_then(|v| v["elapsed_ms"].as_u64()).unwrap_or(0);
                format!(" Waited {ms} ms until {}.", wait_for_label(input))
            }
            Err(e) => format!(" {}", e.trim_start_matches("wait_timeout: ").replace("waited", "Waited")),
        },
        None => {
            let wait_ms = input["wait_ms"].as_u64().unwrap_or(800).min(10_000);
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
            String::new()
        }
    };
    if let Ok(mut guard) = ax_cache.lock() {
        guard.clear();
    }
    let after_input = serde_json::json!({ "app": app, "quality": input["quality"] });
    match observe(&app, &after_input, snapshot_store, ax_cache).await {
        Ok(after) => {
            remember_act(act_key.clone(), screen_fingerprint(&after.snapshot));
            let mut delta = match snap.as_ref().filter(|s| s.app.as_deref().map_or(app.is_empty(), |a| a.eq_ignore_ascii_case(&app))) {
                Some(before) => delta_line(before, &after.snapshot),
                None => String::new(),
            };
            if let Some((label, end_px)) = drop_check {
                let report = drop_report(&label, end_px, &after.snapshot);
                if !report.is_empty() {
                    delta = format!("{delta} {report}");
                }
            }
            // Delivery, said plainly: what changed is the verification.
            let verdict = if delta.is_empty() {
                "Delivered"
            } else if delta.starts_with("Window unchanged") {
                "Delivered; nothing visible changed yet (slow to react, or it had no effect)"
            } else {
                "Delivered and verified"
            };
            let mut r = after.result;
            r.content = format!("{}{pick_note}. {verdict}.{wait_note} {delta}\n\n{}", performed.content, r.content);
            r
        }
        Err(e) => ToolResult::ok(format!(
            "{}. Delivered, unverified: could not capture the after-state: {}",
            performed.content, e.content
        )),
    }
}

/// `wait_for` on an input action: wait for that instead of a fixed pause.
/// `{"text": "Saved"}`, `{"appears": "Export"}` / `{"gone": "Loading"}` (a
/// label), `{"menu": true}` / `{"menu": false}`, `{"window": true}` or
/// `{"window": "Untitled"}`; `timeout_ms` (default 5000, max 30000).
fn wait_for_args(app: &str, input: &serde_json::Value) -> Option<Vec<String>> {
    let w = input.get("wait_for").filter(|v| v.is_object())?;
    if app.is_empty() {
        return None;
    }
    let mut a = vec!["wait".to_string(), "--app".into(), app.to_string()];
    let s = |k: &str| w[k].as_str().map(str::to_string).filter(|v| !v.is_empty());
    if let Some(t) = s("text") {
        a.extend(["--for".into(), "text".into(), "--text".into(), t]);
    } else if let Some(l) = s("appears") {
        a.extend(["--for".into(), "appears".into(), "--label".into(), l]);
    } else if let Some(l) = s("gone") {
        a.extend(["--for".into(), "gone".into(), "--label".into(), l]);
    } else if let Some(open) = w["menu"].as_bool() {
        a.extend(["--for".into(), if open { "menu" } else { "menu-closed" }.into()]);
    } else if !w["window"].is_null() && w["window"] != serde_json::json!(false) {
        a.extend(["--for".into(), "window".into()]);
        if let Some(t) = s("window") {
            a.extend(["--title".into(), t]);
        }
    } else {
        return None;
    }
    a.extend(["--timeout-ms".into(), wait_for_timeout(input).to_string()]);
    Some(a)
}

fn wait_for_timeout(input: &serde_json::Value) -> u64 {
    input["wait_for"]["timeout_ms"].as_u64().unwrap_or(5_000).clamp(100, 30_000)
}

fn wait_for_label(input: &serde_json::Value) -> String {
    let w = &input["wait_for"];
    for k in ["text", "appears", "gone"] {
        if let Some(v) = w[k].as_str() {
            return format!("{k} \"{v}\"");
        }
    }
    if let Some(open) = w["menu"].as_bool() {
        return if open { "a menu opened".into() } else { "the menu closed".into() };
    }
    "the window changed".into()
}

/// Put `text` into the focused field through the clipboard, and put the
/// clipboard back afterwards. On Linux xdotool types directly, which does
/// not drop characters the way macOS keystrokes do.
async fn paste_text(text: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let before = tokio::process::Command::new("pbpaste").output().await.ok().map(|o| o.stdout);
        if let Err(e) = pbcopy(text.as_bytes()).await {
            return ToolResult::error(format!("could not stage the text on the clipboard: {e}"));
        }
        let r = input_paste().await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        if let Some(b) = before {
            let _ = pbcopy(&b).await;
        }
        return r;
    }
    #[allow(unreachable_code)]
    input_type(text).await
}

#[cfg(target_os = "macos")]
async fn pbcopy(bytes: &[u8]) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(bytes).await.map_err(|e| e.to_string())?;
    }
    child.wait().await.map_err(|e| e.to_string()).map(|_| ())
}

/// Physical input through the native helper (CGEvent): no cliclick, which a
/// stock Mac does not have, and no System Events, which needs an Automation
/// grant. `None` means the helper is unavailable and the caller falls back.
#[cfg(target_os = "macos")]
async fn helper_input(args: &[String]) -> Option<ToolResult> {
    match ax_native::run(args, Duration::from_secs(20)).await {
        Ok(out) => Some(ToolResult::ok(out)),
        Err(e) if e.contains("helper unavailable") => None,
        Err(e) => Some(ToolResult::error(e)),
    }
}

#[cfg(target_os = "macos")]
fn args_of(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// `keys` as the model sends it: "cmd+a", ["cmd", "a"], or that list
/// serialized into a string ("[\"cmd\", \"a\"]").
fn combo_from(v: &serde_json::Value) -> String {
    let list = match v {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::String(s) if s.trim_start().starts_with('[') => serde_json::from_str(s).unwrap_or_default(),
        serde_json::Value::String(s) => return s.trim().to_string(),
        _ => return String::new(),
    };
    list.iter().filter_map(|k| k.as_str()).map(str::trim).filter(|k| !k.is_empty()).collect::<Vec<_>>().join("+")
}

/// Log out, lock, force-quit: combos a tool never sends on its own.
fn blocked_combo(keys: &str) -> bool {
    let Ok((key, mods)) = split_combo(keys) else { return false };
    let mut m: Vec<&str> = mods.split(',').filter(|s| !s.is_empty()).collect();
    m.sort_unstable();
    let key = key.to_lowercase();
    matches!(
        (m.as_slice(), key.as_str()),
        (["cmd", "shift"], "q") | (["cmd", "opt", "shift"], "q") | (["cmd", "ctrl"], "q") | (["cmd", "opt"], "escape" | "esc")
            | (["cmd", "opt", "shift"], "escape" | "esc")
    )
}

/// "cmd+shift+s" → (key, "cmd,shift"). Unknown modifiers are refused.
fn split_combo(keys: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = keys.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    let Some((key, mods)) = parts.split_last() else { return Err("empty key combo".into()) };
    let mut out = Vec::new();
    for m in mods {
        out.push(match m.to_lowercase().as_str() {
            "cmd" | "command" | "meta" | "super" => "cmd",
            "shift" => "shift",
            "opt" | "option" | "alt" => "opt",
            "ctrl" | "control" => "ctrl",
            "fn" => "fn",
            other => return Err(format!("unknown modifier '{other}' in '{keys}'")),
        });
    }
    Ok((key.to_string(), out.join(",")))
}

async fn input_type(text: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        if let Some(r) = helper_input(&args_of(&["type", "--text", text])).await {
            return r;
        }
        let script = format!(
            "tell application \"System Events\" to keystroke \"{}\"",
            escape_applescript(text)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            return run_command("xdotool", &["type", "--clearmodifiers", text]).await;
        }
        return ToolResult::error("Input type requires xdotool");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            "$wsh = New-Object -ComObject WScript.Shell; $wsh.SendKeys('{}')",
            escape_powershell(text)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = text;
        ToolResult::error("Input type is not supported on this platform")
    }
}

async fn input_press(key: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        if let Ok(code) = key_name_to_code(key) {
            if let Some(r) = helper_input(&args_of(&["key", "--code", code])).await {
                return r;
            }
        }
        let key_code = match key_name_to_code(key) {
            Ok(code) => code,
            Err(e) => return ToolResult::error(e),
        };
        let script = format!(
            "tell application \"System Events\" to key code {}",
            key_code
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            return run_command("xdotool", &["key", key]).await;
        }
        return ToolResult::error("Input press requires xdotool");
    }
    #[cfg(target_os = "windows")]
    {
        let sendkey = key_name_to_sendkeys(key);
        let script = format!(
            "$wsh = New-Object -ComObject WScript.Shell; $wsh.SendKeys('{}')",
            sendkey
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = key;
        ToolResult::error("Input press is not supported on this platform")
    }
}


/// `x, y, w, h` as System Events prints a window's `{position, size}`.
/// The helper's answer when the app is running with no window to act on.
pub(crate) fn is_no_window(e: &str) -> bool {
    e.contains("has no open window") || e.contains("there is no window")
}

/// A window mid-resize (a launch animation, a mode switch) yields an image
/// that is not the frame's shape, and the elements would land on the wrong
/// pixels. Five percent covers rounding and a title bar's worth of shadow.
pub(crate) fn image_matches_frame(frame: Option<&Rect>, dims: Option<(i64, i64)>) -> bool {
    match (frame, dims) {
        (Some(f), Some((w, h))) if f.width > 0 && f.height > 0 && w > 0 && h > 0 => {
            let want = f.width as f64 / f.height as f64;
            let got = w as f64 / h as f64;
            ((want - got) / want).abs() <= 0.05
        }
        _ => true,
    }
}

pub(crate) fn parse_frame(s: &str) -> Option<(i64, i64, i64, i64)> {
    let mut it = s
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(|t| t.parse::<i64>().ok());
    let f = (it.next()??, it.next()??, it.next()??, it.next()??);
    (f.2 > 0 && f.3 > 0).then_some(f)
}

/// `app`'s first window frame in screen points — the units cliclick and
/// screencapture take. `raise` brings the app to the front first (needed
/// before acting, not before looking).
#[cfg(target_os = "macos")]
async fn window_frame(app: &str, raise: bool) -> Result<Rect, String> {
    let script = format!(
        "tell application \"System Events\" to tell process \"{}\"\n\
         {}get {{position, size}} of window 1\n\
         end tell",
        escape_applescript(app),
        if raise { "set frontmost to true\n" } else { "" }
    );
    let out = match run_osascript_raw(&script, Some(AX_CAPTURE_TIMEOUT)).await {
        Ok(out) => out,
        Err(e) if e.contains("-1719") => return Err(format!("{app} has no open window")),
        Err(e) => return Err(e),
    };
    parse_frame(&out)
        .map(|(x, y, w, h)| Rect { x, y, width: w, height: h })
        .ok_or_else(|| format!("could not read the window frame of {app} (got '{}')", out.trim()))
}

// ponytail: window frames are read on macOS only; elsewhere every capture is
// the whole screen and coordinates are screen pixels. Add xdotool
// getwindowgeometry / UIAutomation BoundingRectangle when a cloud task
// needs window-relative acting.
#[cfg(not(target_os = "macos"))]
async fn window_frame(app: &str, _raise: bool) -> Result<Rect, String> {
    Err(format!("window frames are not read on this platform ({app}); captures are the whole screen"))
}

/// The whole screen in screen points, for captures that are not of one window.
async fn screen_rect() -> Option<Rect> {
    #[cfg(target_os = "macos")]
    {
        let out = run_osascript_raw(
            "tell application \"Finder\" to get bounds of window of desktop",
            Some(AX_CAPTURE_TIMEOUT),
        )
        .await
        .ok()?;
        let (x, y, w, h) = parse_frame(&out)?;
        return Some(Rect { x, y, width: w, height: h });
    }
    #[cfg(target_os = "linux")]
    {
        let out = run_command_raw("xdotool", &["getdisplaygeometry"]).await.ok()?;
        let mut it = out.split_whitespace().filter_map(|t| t.parse::<i64>().ok());
        return Some(Rect { x: 0, y: 0, width: it.next()?, height: it.next()? });
    }
    #[cfg(target_os = "windows")]
    {
        let out = ps_daemon()
            .execute(
                "Add-Type -AssemblyName System.Windows.Forms; $b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds; \"$($b.Width) $($b.Height)\"",
                Duration::from_secs(5),
            )
            .await
            .ok()?;
        let mut it = out.split_whitespace().filter_map(|t| t.parse::<i64>().ok());
        return Some(Rect { x: 0, y: 0, width: it.next()?, height: it.next()? });
    }
    #[allow(unreachable_code)]
    None
}

async fn input_click(x: i64, y: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        if let Some(r) = helper_input(&args_of(&["click", "--x", &x.to_string(), "--y", &y.to_string()])).await {
            return r;
        }
        let arg = format!("c:{},{}", x, y);
        return run_command("cliclick", &[&arg]).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            let xs = x.to_string();
            let ys = y.to_string();
            let _ = run_command("xdotool", &["mousemove", &xs, &ys]).await;
            return run_command("xdotool", &["click", "1"]).await;
        }
        return ToolResult::error("Input click requires xdotool");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type @"
using System; using System.Runtime.InteropServices;
public class Mouse {{
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, int dwExtraInfo);
}}
"@
[Mouse]::SetCursorPos({}, {})
[Mouse]::mouse_event(0x0002, 0, 0, 0, 0)
[Mouse]::mouse_event(0x0004, 0, 0, 0, 0)"#,
            x, y
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (x, y);
        ToolResult::error("Input click is not supported on this platform")
    }
}

async fn input_double_click(x: i64, y: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        if let Some(r) = helper_input(&args_of(&["click", "--x", &x.to_string(), "--y", &y.to_string(), "--count", "2"])).await {
            return r;
        }
        let arg = format!("dc:{},{}", x, y);
        return run_command("cliclick", &[&arg]).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            let xs = x.to_string();
            let ys = y.to_string();
            let _ = run_command("xdotool", &["mousemove", &xs, &ys]).await;
            return run_command("xdotool", &["click", "--repeat", "2", "1"]).await;
        }
        return ToolResult::error("Double click requires xdotool");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type @"
using System; using System.Runtime.InteropServices;
public class Mouse {{
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, int dwExtraInfo);
}}
"@
[Mouse]::SetCursorPos({}, {})
[Mouse]::mouse_event(0x0002, 0, 0, 0, 0); [Mouse]::mouse_event(0x0004, 0, 0, 0, 0)
Start-Sleep -Milliseconds 50
[Mouse]::mouse_event(0x0002, 0, 0, 0, 0); [Mouse]::mouse_event(0x0004, 0, 0, 0, 0)"#,
            x, y
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (x, y);
        ToolResult::error("Double click is not supported on this platform")
    }
}

async fn input_right_click(x: i64, y: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        if let Some(r) = helper_input(&args_of(&["click", "--x", &x.to_string(), "--y", &y.to_string(), "--button", "right"])).await {
            return r;
        }
        let arg = format!("rc:{},{}", x, y);
        return run_command("cliclick", &[&arg]).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            let xs = x.to_string();
            let ys = y.to_string();
            let _ = run_command("xdotool", &["mousemove", &xs, &ys]).await;
            return run_command("xdotool", &["click", "3"]).await;
        }
        return ToolResult::error("Right click requires xdotool");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type @"
using System; using System.Runtime.InteropServices;
public class Mouse {{
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, int dwExtraInfo);
}}
"@
[Mouse]::SetCursorPos({}, {})
[Mouse]::mouse_event(0x0008, 0, 0, 0, 0)
[Mouse]::mouse_event(0x0010, 0, 0, 0, 0)"#,
            x, y
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (x, y);
        ToolResult::error("Right click is not supported on this platform")
    }
}

async fn input_hotkey(keys: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        match split_combo(keys) {
            Ok((key, mods)) => {
                if let Ok(code) = key_name_to_code(&key) {
                    if let Some(r) = helper_input(&args_of(&["key", "--code", code, "--mods", &mods])).await {
                        return r;
                    }
                }
            }
            Err(e) => return ToolResult::error(e),
        }
        // Parse "command+shift+s" into AppleScript key code with modifiers
        let parts: Vec<&str> = keys.split('+').map(|s| s.trim()).collect();
        let key = parts.last().unwrap_or(&"");
        let mut modifiers = Vec::new();
        for &part in &parts[..parts.len().saturating_sub(1)] {
            match part.to_lowercase().as_str() {
                "command" | "cmd" => modifiers.push("command down"),
                "shift" => modifiers.push("shift down"),
                "option" | "alt" => modifiers.push("option down"),
                "control" | "ctrl" => modifiers.push("control down"),
                _ => {}
            }
        }
        let modifier_str = if modifiers.is_empty() {
            String::new()
        } else {
            format!(" using {{{}}}", modifiers.join(", "))
        };
        let script = format!(
            "tell application \"System Events\" to keystroke \"{}\"{}",
            escape_applescript(key),
            modifier_str
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            // xdotool uses "+" for key combos natively
            return run_command("xdotool", &["key", keys]).await;
        }
        return ToolResult::error("Hotkey requires xdotool");
    }
    #[cfg(target_os = "windows")]
    {
        // Convert "ctrl+shift+s" to SendKeys format "^+s"
        let parts: Vec<&str> = keys.split('+').map(|s| s.trim()).collect();
        let key = parts.last().unwrap_or(&"");
        let mut prefix = String::new();
        for &part in &parts[..parts.len().saturating_sub(1)] {
            match part.to_lowercase().as_str() {
                "ctrl" | "control" => prefix.push('^'),
                "alt" | "option" => prefix.push('%'),
                "shift" => prefix.push('+'),
                _ => {}
            }
        }
        let script = format!(
            "$wsh = New-Object -ComObject WScript.Shell; $wsh.SendKeys('{}{}')",
            prefix, key
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = keys;
        ToolResult::error("Hotkey is not supported on this platform")
    }
}

async fn input_move(x: i64, y: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        if let Some(r) = helper_input(&args_of(&["move", "--x", &x.to_string(), "--y", &y.to_string()])).await {
            return r;
        }
        let arg = format!("m:{},{}", x, y);
        return run_command("cliclick", &[&arg]).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            let xs = x.to_string();
            let ys = y.to_string();
            return run_command("xdotool", &["mousemove", &xs, &ys]).await;
        }
        return ToolResult::error("Mouse move requires xdotool");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type @"
using System; using System.Runtime.InteropServices;
public class Mouse {{ [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y); }}
"@
[Mouse]::SetCursorPos({}, {})"#,
            x, y
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (x, y);
        ToolResult::error("Mouse move is not supported on this platform")
    }
}

#[allow(unused_variables)]
async fn input_scroll(dx: i64, dy: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        // Real wheel events, both axes. (The cliclick path below sent `ku`/`kd`,
        // which are key up/down commands, not scrolling.)
        if let Some(r) = helper_input(&args_of(&["scroll", "--dx", &dx.to_string(), "--dy", &dy.to_string()])).await {
            return r;
        }
        // cliclick supports scroll: kd (scroll down) / ku (scroll up)
        if dx != 0 && dy == 0 {
            return ToolResult::error(
                "Horizontal scroll (left/right) is not supported on macOS. Nothing was scrolled; only up and down work here.",
            );
        }
        if dy == 0 {
            return ToolResult::error("Nothing scrolled: the scroll amount was zero.");
        }
        let dir = if dy > 0 { "ku" } else { "kd" };
        let count = dy.unsigned_abs() as usize;
        let args: Vec<String> = (0..count).map(|_| dir.to_string()).collect();
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        return run_command("cliclick", &arg_refs).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            // button 4=up, 5=down, 6=left, 7=right
            let mut results = Vec::new();
            if dy != 0 {
                let btn = if dy > 0 { "4" } else { "5" };
                let count = dy.unsigned_abs().to_string();
                results.push(run_command("xdotool", &["click", "--repeat", &count, btn]).await);
            }
            if dx != 0 {
                let btn = if dx > 0 { "7" } else { "6" };
                let count = dx.unsigned_abs().to_string();
                results.push(run_command("xdotool", &["click", "--repeat", &count, btn]).await);
            }
            if results.is_empty() {
                return ToolResult::ok("Nothing scrolled (zero delta).");
            }
            return results
                .into_iter()
                .last()
                .unwrap_or_else(|| ToolResult::ok("OK"));
        }
        return ToolResult::error("Scroll requires xdotool");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type @"
using System; using System.Runtime.InteropServices;
public class Mouse {{ [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, int dwExtraInfo); }}
"@
[Mouse]::mouse_event(0x0800, 0, 0, {}, 0)"#,
            dy * 120 // WHEEL_DELTA = 120
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (dx, dy);
        ToolResult::error("Scroll is not supported on this platform")
    }
}

async fn input_drag(x: i64, y: i64, x2: i64, y2: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        // Pick up, move in steps, dwell over the target so it activates, drop.
        if let Some(r) = helper_input(&args_of(&["drag", "--x", &x.to_string(), "--y", &y.to_string(), "--to-x", &x2.to_string(), "--to-y", &y2.to_string()])).await {
            return r;
        }
        let dd = format!("dd:{},{}", x, y);
        let du = format!("du:{},{}", x2, y2);
        return run_command("cliclick", &[&dd, &du]).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            let xs = x.to_string();
            let ys = y.to_string();
            let x2s = x2.to_string();
            let y2s = y2.to_string();
            let _ = run_command("xdotool", &["mousemove", &xs, &ys]).await;
            let _ = run_command("xdotool", &["mousedown", "1"]).await;
            let _ = run_command("xdotool", &["mousemove", &x2s, &y2s]).await;
            return run_command("xdotool", &["mouseup", "1"]).await;
        }
        return ToolResult::error("Drag requires xdotool");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type @"
using System; using System.Runtime.InteropServices;
public class Mouse {{
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, int dwExtraInfo);
}}
"@
[Mouse]::SetCursorPos({}, {})
[Mouse]::mouse_event(0x0002, 0, 0, 0, 0)
Start-Sleep -Milliseconds 50
[Mouse]::SetCursorPos({}, {})
Start-Sleep -Milliseconds 50
[Mouse]::mouse_event(0x0004, 0, 0, 0, 0)"#,
            x, y, x2, y2
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (x, y, x2, y2);
        ToolResult::error("Drag is not supported on this platform")
    }
}

async fn input_paste() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        if let Some(r) = helper_input(&args_of(&["key", "--code", "9", "--mods", "cmd"])).await {
            return r;
        }
        let script = "tell application \"System Events\" to keystroke \"v\" using command down";
        return run_osascript(script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            return run_command("xdotool", &["key", "ctrl+v"]).await;
        }
        return ToolResult::error("Paste requires xdotool");
    }
    #[cfg(target_os = "windows")]
    {
        let script = "$wsh = New-Object -ComObject WScript.Shell; $wsh.SendKeys('^v')";
        return run_powershell(script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Paste is not supported on this platform")
}

// --- Clipboard ---

async fn handle_clipboard(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "read" => clipboard_read().await,
        "write" => {
            let text = input["text"].as_str().unwrap_or("");
            if text.is_empty() {
                return ToolResult::error(errors::missing_param("write", "text", "os(resource: \"clipboard\", action: \"write\", text: \"hello\")"));
            }
            clipboard_write(text).await
        }
        "clear" => clipboard_clear().await,
        _ => ToolResult::error(format!(
            "Unknown clipboard action '{}'. Use: read, write, clear",
            action
        )),
    }
}

async fn clipboard_read() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        return match tokio::process::Command::new("pbpaste").output().await {
            Ok(output) => {
                let text = String::from_utf8_lossy(&output.stdout).to_string();
                ToolResult::ok(if text.is_empty() {
                    "Clipboard is empty (0 bytes).".to_string()
                } else {
                    text
                })
            }
            Err(e) => ToolResult::error(format!("Failed to read clipboard: {}", e)),
        };
    }
    #[cfg(target_os = "linux")]
    {
        // Try wayland first, then X11
        if which("wl-paste") {
            return run_command("wl-paste", &[]).await;
        }
        if which("xclip") {
            return run_command("xclip", &["-selection", "clipboard", "-o"]).await;
        }
        if which("xsel") {
            return run_command("xsel", &["-ob"]).await;
        }
        return ToolResult::error("Clipboard read requires wl-paste, xclip, or xsel");
    }
    #[cfg(target_os = "windows")]
    {
        return run_powershell("Get-Clipboard").await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Clipboard is not supported on this platform")
}

async fn clipboard_write(text: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let mut child = match tokio::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to write clipboard: {}", e)),
        };
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            if let Err(e) = stdin.write_all(text.as_bytes()).await {
                return ToolResult::error(format!("Failed to write clipboard: {}", e));
            }
        }
        match child.wait_with_output().await {
            Ok(output) if output.status.success() => {
                ToolResult::ok(clipboard_updated_message(text))
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                ToolResult::error(format!(
                    "pbcopy exited with {}{}; clipboard not updated",
                    output.status,
                    if stderr.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", stderr)
                    }
                ))
            }
            Err(e) => ToolResult::error(format!("Failed to write clipboard: {}", e)),
        }
    }
    #[cfg(target_os = "linux")]
    {
        let result = if which("wl-copy") {
            pipe_to_command("wl-copy", &[], text).await
        } else if which("xclip") {
            pipe_to_command("xclip", &["-selection", "clipboard"], text).await
        } else if which("xsel") {
            pipe_to_command("xsel", &["-ib"], text).await
        } else {
            return ToolResult::error("Clipboard write requires wl-copy, xclip, or xsel");
        };
        if result.is_error {
            return result;
        }
        return ToolResult::ok(clipboard_updated_message(text));
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!("Set-Clipboard -Value '{}'", escape_powershell(text));
        let result = run_powershell(&script).await;
        if result.is_error {
            return result;
        }
        return ToolResult::ok(clipboard_updated_message(text));
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = text;
        ToolResult::error("Clipboard is not supported on this platform")
    }
}

/// Success line for clipboard write: the char count is the evidence that the
/// text reached the clipboard, on every platform.
fn clipboard_updated_message(text: &str) -> String {
    format!("Clipboard updated ({} chars).", text.chars().count())
}

async fn clipboard_clear() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        return run_osascript("set the clipboard to \"\"").await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("wl-copy") {
            return pipe_to_command("wl-copy", &[], "").await;
        }
        if which("xclip") {
            return pipe_to_command("xclip", &["-selection", "clipboard"], "").await;
        }
        if which("xsel") {
            return run_command("xsel", &["-bc"]).await;
        }
        return ToolResult::error("Clipboard clear requires wl-copy, xclip, or xsel");
    }
    #[cfg(target_os = "windows")]
    {
        return run_powershell("Set-Clipboard -Value $null").await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Clipboard is not supported on this platform")
}

// --- Notifications ---

async fn handle_notification(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "send" => {
            let title = input["title"].as_str().unwrap_or("Nebo");
            let message = input["message"].as_str().unwrap_or("");
            if message.is_empty() {
                return ToolResult::error(errors::missing_param("send", "message", "os(resource: \"notification\", action: \"send\", title: \"Done\", message: \"Task complete\")"));
            }
            notification_send(title, message).await
        }
        "alert" => {
            let title = input["title"].as_str().unwrap_or("Nebo");
            let message = input["message"].as_str().unwrap_or("");
            if message.is_empty() {
                return ToolResult::error(errors::missing_param("alert", "message", "os(resource: \"notification\", action: \"alert\", title: \"Warning\", message: \"Something happened\")"));
            }
            notification_alert(title, message).await
        }
        _ => ToolResult::error(format!(
            "Unknown notification action '{}'. Use: send, alert",
            action
        )),
    }
}

async fn notification_send(title: &str, message: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            escape_applescript(message),
            escape_applescript(title)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("notify-send") {
            return run_command("notify-send", &[title, message]).await;
        }
        return ToolResult::error("Notifications require notify-send (install libnotify)");
    }
    #[cfg(target_os = "windows")]
    {
        // Try BurntToast module, fallback to .NET
        let script = format!(
            r#"if (Get-Module -ListAvailable -Name BurntToast) {{
    New-BurntToastNotification -Text '{}', '{}'
}} else {{
    Add-Type -AssemblyName System.Windows.Forms
    $n = New-Object System.Windows.Forms.NotifyIcon
    $n.Icon = [System.Drawing.SystemIcons]::Information
    $n.BalloonTipTitle = '{}'
    $n.BalloonTipText = '{}'
    $n.Visible = $true
    $n.ShowBalloonTip(5000)
}}"#,
            escape_powershell(title),
            escape_powershell(message),
            escape_powershell(title),
            escape_powershell(message)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (title, message);
        ToolResult::error("Notifications are not supported on this platform")
    }
}

async fn notification_alert(title: &str, message: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display alert \"{}\" message \"{}\"",
            escape_applescript(title),
            escape_applescript(message)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("zenity") {
            return run_command("zenity", &["--info", "--title", title, "--text", message]).await;
        }
        if which("kdialog") {
            return run_command("kdialog", &["--msgbox", message, "--title", title]).await;
        }
        if which("notify-send") {
            return run_command("notify-send", &["-u", "critical", title, message]).await;
        }
        return ToolResult::error("Alert requires zenity, kdialog, or notify-send");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type -AssemblyName PresentationFramework
[System.Windows.MessageBox]::Show('{}', '{}')"#,
            escape_powershell(message),
            escape_powershell(title)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (title, message);
        ToolResult::error("Alert is not supported on this platform")
    }
}

// --- Screen capture ---

/// The capture action a call means: the desktop straps say `capture` for a
/// screenshot, so that name is read as `screenshot`.
fn canonical_capture_action(action: &str) -> &str {
    if action == "capture" {
        "screenshot"
    } else {
        action
    }
}

async fn handle_capture(
    action: &str,
    input: &serde_json::Value,
    snapshot_store: &tokio::sync::Mutex<SnapshotStore>,
    ax_cache: &AxCache,
) -> ToolResult {
    match canonical_capture_action(action) {
        "screenshot" => capture_screenshot(input).await,
        "see" => capture_see(input, snapshot_store, ax_cache).await,
        // Wait for something to be on screen (or gone) instead of guessing a pause.
        "wait" => {
            let app = input["app"].as_str().unwrap_or("");
            let mut spec = input.clone();
            if spec.get("wait_for").is_none() {
                spec["wait_for"] = serde_json::json!({
                    "text": input["text"], "appears": input["label"], "gone": input["gone"],
                    "menu": input["menu"], "window": input["window"], "timeout_ms": input["timeout_ms"],
                });
            }
            match wait_for_args(app, &spec) {
                None => ToolResult::error(
                    "wait needs app and one of: text, label (appears), gone, menu (true|false), window — e.g. os(resource: \"capture\", action: \"wait\", app: \"Safari\", text: \"Saved\")",
                ),
                Some(args) => match ax_native::run(&args, Duration::from_millis(wait_for_timeout(&spec) + 2_000)).await {
                    Ok(out) => {
                        let ms = ax_native::json_lines(&out).first().and_then(|v| v["elapsed_ms"].as_u64()).unwrap_or(0);
                        ToolResult::ok(format!("Waited {ms} ms: {} in {app}. Capture to see it.", wait_for_label(&spec)))
                    }
                    Err(e) => ToolResult::error(e.trim_start_matches("wait_timeout: ").replace("waited", "Waited")),
                },
            }
        }
        _ => ToolResult::error(format!(
            "Unknown capture action '{}'. Use: screenshot, see",
            action
        )),
    }
}

/// One look at a target: image, frame, scale and elements, stored as a
/// snapshot the next act resolves against.
struct Observed {
    result: ToolResult,
    snapshot: Snapshot,
}

async fn capture_see(
    input: &serde_json::Value,
    snapshot_store: &tokio::sync::Mutex<SnapshotStore>,
    ax_cache: &AxCache,
) -> ToolResult {
    let app = input["app"].as_str().unwrap_or("").trim();
    match observe(app, input, snapshot_store, ax_cache).await {
        Ok(o) => o.result,
        Err(e) => e,
    }
}

/// Walk `app` natively; when that is unavailable, the AppleScript walk. The
/// layer used is named in the capture, never inferred by the caller.
/// Roles a person types into: their value is what they typed, never their name.
fn is_editable_role(role: &str) -> bool {
    matches!(role, "AXTextField" | "AXTextArea" | "AXComboBox" | "AXSearchField" | "AXSecureTextField")
}

/// A walked node as the model sees it. The label is what names the element
/// across captures (title, description, placeholder); a field's contents are
/// its value, listed beside it. A secure field's contents are never read.
fn element_from_node(n: &ax_native::AxNode) -> UIElement {
    let secure = n.role == "AXSecureTextField";
    let editable = is_editable_role(&n.role);
    let label = if !n.title.is_empty() {
        n.title.clone()
    } else if let Some(d) = n.desc.clone().filter(|d| !d.is_empty()) {
        d
    } else if let Some(p) = n.placeholder.clone().filter(|p| !p.is_empty()) {
        p
    } else if secure {
        "[secure field]".to_string()
    } else if editable {
        String::new()
    } else {
        n.value.clone().unwrap_or_default()
    };
    UIElement { more: n.more,
        id: String::new(),
        role: n.role.clone(),
        label,
        bounds: Rect { x: n.frame[0], y: n.frame[1], width: n.frame[2], height: n.frame[3] },
        actionable: !n.actions.is_empty(),
        keyboard_shortcut: n.shortcut.clone(),
        actions: n.actions.clone(),
        path: n.path.clone(),
        focused: n.focused,
        value: if secure || !editable { None } else { n.value.clone().filter(|v| !v.is_empty()) },
    }
}

async fn walk_elements(app: &str, root: Option<String>) -> (AxCapture, Vec<UIElement>) {
    // A drill walks one element's subtree deeper than the whole-window walk.
    let opts = match root {
        Some(r) => ax_native::WalkOpts { depth: 40, max: 500, root: Some(r), ..Default::default() },
        None => ax_native::WalkOpts::default(),
    };
    match ax_native::tree(app, &opts).await {
        Ok(tree) => {
            let elements: Vec<UIElement> = tree
                .nodes
                .iter()
                .map(element_from_node)
                .collect();
            let menu_open = tree.nodes.iter().any(|n| n.menu);
            let actionable = elements.iter().filter(|e| e.actionable).count();
            (
                AxCapture {
                    app: if tree.app.is_empty() { app.to_string() } else { tree.app },
                    windows: Some(tree.windows),
                    walk_error: None,
                    via: if actionable >= 3 && elements.iter().any(|e| !e.label.is_empty()) { "ax" } else { "vision" }.to_string(),
                    truncated: tree.truncated,
                    fallback: None,
                    cut_by: tree.cut_by.clone(),
                    menu_open,
                },
                elements,
            )
        }
        Err(reason) => {
            let (mut cap, elements) = capture_ax_elements(app).await;
            let actionable = elements.iter().filter(|e| e.actionable).count();
            cap.via = if cap.walk_error.is_none() && actionable >= 3 { "ax-shallow" } else { "vision" }.to_string();
            cap.fallback = Some(reason);
            (cap, elements)
        }
    }
}

/// Add recognized text lines to the element list as `OCRText` elements in
/// screen points, skipping lines whose centre falls inside an element that
/// already carries a label. Returns (added, total lines).
fn merge_text_lines(
    elements: &mut Vec<UIElement>,
    lines: &[ax_native::TextLine],
    frame: Option<&Rect>,
    scale: f64,
) -> (usize, usize) {
    let (ox, oy) = frame.map(|f| (f.x, f.y)).unwrap_or((0, 0));
    let mut added = 0;
    for l in lines {
        let to_screen = |px: i64, py: i64| (ox + (px as f64 * scale).round() as i64, oy + (py as f64 * scale).round() as i64);
        let (x, y) = to_screen(l.frame[0], l.frame[1]);
        let (w, h) = (((l.frame[2] as f64) * scale).round() as i64, ((l.frame[3] as f64) * scale).round() as i64);
        let (cx, cy) = (x + w / 2, y + h / 2);
        let known = elements.iter().any(|e| {
            !e.label.is_empty()
                && cx >= e.bounds.x
                && cx < e.bounds.x + e.bounds.width
                && cy >= e.bounds.y
                && cy < e.bounds.y + e.bounds.height
        });
        if known {
            continue;
        }
        elements.push(UIElement { more: 0,
            id: String::new(),
            role: "OCRText".into(),
            label: l.text.clone(),
            bounds: Rect { x, y, width: w.max(1), height: h.max(1) },
            actionable: true,
            keyboard_shortcut: None,
            actions: Vec::new(),
            path: String::new(),
            focused: false,
            value: None,
        });
        added += 1;
    }
    (added, lines.len())
}

async fn observe(
    app: &str,
    input: &serde_json::Value,
    snapshot_store: &tokio::sync::Mutex<SnapshotStore>,
    ax_cache: &AxCache,
) -> Result<Observed, ToolResult> {
    let quality = input["quality"].as_str().unwrap_or("medium");
    let max_elements = input["max_elements"].as_u64().unwrap_or(60).min(500) as usize;

    // No app named means the window in front — what "look" means to a person
    // — never the whole screen while a window is up. The model on Stadium
    // (2026-09-23) captured the screen, then every click on it was refused
    // against the window it had actually meant. The whole screen is what is
    // left when nothing is in front, or when Nebo itself is.
    let front;
    let app: &str = if app.is_empty() {
        match ax_native::frontmost().await {
            Ok(a) if ax_native::is_lock_screen(&a) => return Err(ToolResult::error(ax_native::LOCKED_SCREEN)),
            Ok(a) if !a.is_empty() && a != "Nebo" => {
                front = a;
                front.as_str()
            }
            _ => app,
        }
    } else {
        app
    };

    // 1. What the image will cover. The window's own pixels by id when the
    //    platform gives one (they are right even under other windows); by
    //    screen region otherwise, which shows whatever is on top there.
    let mut window_id: Option<u64>;
    let mut attempt = 0;
    let (frame, window_image, frame_note, shot, dims) = loop {
        window_id = None;
        let (frame, window_image, frame_note) = if app.is_empty() {
            (screen_rect().await, false, String::new())
        } else {
            match ax_native::window(app, 1).await {
                Ok(w) => {
                    window_id = w.window_id;
                    let f = Rect { x: w.frame[0], y: w.frame[1], width: w.frame[2], height: w.frame[3] };
                    let note = if w.window_id.is_some() { String::new() } else { " (captured by screen region; windows on top of it show through)".to_string() };
                    (Some(f), true, note)
                }
                Err(_) => match window_frame(app, false).await {
                    Ok(r) => (Some(r), true, " (captured by screen region; windows on top of it show through)".to_string()),
                    Err(e) => (screen_rect().await, false, format!(" ({e}; captured the whole screen instead)")),
                },
            }
        };

        // 2. The image.
        let shot_input = match (&frame, window_image, window_id) {
            (_, true, Some(id)) => serde_json::json!({ "window_id": id, "quality": quality }),
            (Some(f), true, None) => serde_json::json!({ "region": format!("{},{},{},{}", f.x, f.y, f.width, f.height), "quality": quality }),
            _ => serde_json::json!({ "quality": quality }),
        };
        let shot = capture_screenshot(&shot_input).await;
        if shot.is_error {
            return Err(shot);
        }
        let dims = shot
            .payload
            .as_ref()
            .and_then(|p| Some((p["width"].as_u64()? as i64, p["height"].as_u64()? as i64)))
            .filter(|(w, h)| *w > 0 && *h > 0);
        // One retake after a beat when the image is not the frame's shape.
        if attempt == 0 && window_image && !image_matches_frame(frame.as_ref(), dims) {
            attempt += 1;
            tokio::time::sleep(Duration::from_millis(350)).await;
            continue;
        }
        break (frame, window_image, frame_note, shot, dims);
    };
    let scale = match (&frame, dims) {
        (Some(f), Some((w, _))) => f.width as f64 / w as f64,
        _ => 1.0,
    };

    // 3. The elements (a walk done moments ago is reused; an act clears it).
    //    `ref` drills into that element of the last capture: its subtree only.
    let drill_ref = input["ref"].as_str().unwrap_or("").trim().to_string();
    let drill: Option<(String, String)> = if drill_ref.is_empty() {
        None
    } else {
        let store = snapshot_store.lock().await;
        match store.latest_for(app).and_then(|s| s.elements.iter().find(|e| e.id == drill_ref)) {
            Some(e) if !e.path.is_empty() && !e.path.starts_with("m:") => Some((e.path.clone(), format!("{} \"{}\"", e.id, e.label))),
            Some(e) => return Err(ToolResult::error(format!("{} \"{}\" cannot be drilled into (it has no tree path); capture the whole window instead", e.id, e.label))),
            None => return Err(ToolResult::error(format!("see: ref '{drill_ref}' is not in the last capture of {app}; capture again and use a ref from that list"))),
        }
    };
    let cache_key = app.to_string();
    let cached = if drill.is_some() { None } else { ax_cache.lock().ok().and_then(|guard| {
        guard
            .get(&cache_key)
            .filter(|(_, _, ts)| ts.elapsed() < Duration::from_secs(2))
            .map(|(cap, elems, ts)| (cap.clone(), elems.clone(), ts.elapsed().as_millis() as u64))
    }) };
    let mut ax_reused_from_ms_ago: Option<u64> = None;
    let (observed, mut elements) = if let Some((cap, elems, ms)) = cached {
        ax_reused_from_ms_ago = Some(ms);
        (cap, elems)
    } else {
        let (cap, elems) = walk_elements(app, drill.as_ref().map(|d| d.0.clone())).await;
        if drill.is_none() {
            if let Ok(mut guard) = ax_cache.lock() {
                guard.insert(cache_key, (cap.clone(), elems.clone(), Instant::now()));
            }
        }
        (cap, elems)
    };
    // 3b. The text in the image. The accessibility tree misses whatever an
    //     app draws itself (Simulator content, Electron without the switch,
    //     games, canvases); the recognizer reads it off the same pixels the
    //     model sees, so its boxes are already in image coordinates. Lines
    //     that sit inside a labelled AX element are that element, not news.
    let mut ocr_note = String::new();
    let mut ocr_added = 0usize;
    let image_path = shot.payload.as_ref().and_then(|p| p["path"].as_str()).map(std::path::PathBuf::from);
    if let Some(path) = image_path {
        match ax_native::text(&path).await {
            Ok(lines) => {
                let (added, total) = merge_text_lines(&mut elements, &lines, frame.as_ref(), scale);
                ocr_added = added;
                if total > 0 {
                    ocr_note = format!(" +ocr ({added} of {total} text lines not already in the tree)");
                }
            }
            Err(e) => ocr_note = format!(" (text recognition unavailable: {e})"),
        }
    }
    let elements_total = elements.len();
    elements.truncate(max_elements.max(1));
    assign_element_ids(&mut elements, snapshot_store.lock().await.book(app));

    // 4. The snapshot the next act resolves against.
    let snapshot = Snapshot {
        id: generate_snapshot_id(),
        // A screen capture belongs to no app: an act that names one must
        // capture that app's window first, or its pixels would be read
        // against the wrong frame.
        app: if observed.app.is_empty() || !window_image { None } else { Some(observed.app.clone()) },
        created_at: Instant::now(),
        elements: elements.clone(),
        frame: frame.clone(),
        scale,
        via: if observed.via == "vision" && ocr_added > 0 { "ocr".to_string() } else { observed.via.clone() },
    };
    snapshot_store.lock().await.insert(snapshot.clone());

    // 5. What the model reads: one header that says what it is looking at
    //    and how it was read, then the elements in the image's own pixels.
    let name = if observed.app.is_empty() { "the screen".to_string() } else { observed.app.clone() };
    let covers = match &frame {
        Some(f) if window_image => format!("window at {},{} size {}×{} pt", f.x, f.y, f.width, f.height),
        Some(f) => format!("whole screen {}×{} pt", f.width, f.height),
        None => "whole screen".to_string(),
    };
    let image_line = match dims {
        Some((w, h)) if (scale - 1.0).abs() < 0.01 => format!("image {w}×{h} px (1 px = 1 pt)"),
        Some((w, h)) => format!("image {w}×{h} px (1 px = {scale:.2} pt)"),
        None => "image".to_string(),
    };
    let mut via = format!("via {}", snapshot.via);
    if snapshot.via == "vision" {
        let actionable = elements.iter().filter(|e| e.actionable).count();
        via.push_str(&format!(" (accessibility tree had {actionable} actionable elements and no text was read; act by image pixel)"));
    } else if snapshot.via == "ocr" {
        via.push_str(" (accessibility tree was empty; the elements are lines of text read from the image, clickable by ref)");
    }
    via.push_str(&ocr_note);
    if let Some(reason) = &observed.fallback {
        via.push_str(&format!(" (native walk unavailable: {reason})"));
    }
    if let Some(err) = &observed.walk_error {
        via.push_str(&format!(" (walk failed: {err})"));
    }
    if observed.truncated {
        let by = if observed.cut_by.is_empty() { "its budget".to_string() } else { format!("its {}", observed.cut_by) };
        via.push_str(&format!(" (walk cut short by {by}; elements may be missing — capture with ref: <a container> to drill into one part)"));
    }
    if let Some((_, what)) = &drill {
        via.push_str(&format!(" (drilled into {what}: only its contents are listed)"));
    }
    if observed.menu_open {
        via.push_str(" (a menu is open: its items are listed first as menu items; click one by ref, or press escape to close it)");
    }
    if observed.windows == Some(0) {
        via.push_str(" (the app is running but has no open window)");
    }
    if let Some(ms) = ax_reused_from_ms_ago {
        via.push_str(&format!(" (elements from a walk {ms} ms ago)"));
    }
    let filter = input["filter"].as_str().unwrap_or("").to_lowercase();
    let role_filter = input["role"].as_str().unwrap_or("").to_lowercase();
    let listed: Vec<&UIElement> = elements
        .iter()
        .filter(|e| e.actionable || !e.label.is_empty() || e.more > 0)
        .filter(|e| {
            (filter.is_empty()
                || e.label.to_lowercase().contains(&filter)
                || e.role.to_lowercase().contains(&filter)
                || e.actions.iter().any(|a| a.to_lowercase().contains(&filter)))
                && (role_filter.is_empty() || e.role.to_lowercase().contains(&role_filter))
        })
        .collect();
    let mut text = format!(
        "{name} — {covers}{frame_note}; {image_line}; {via}; {} of {elements_total} elements listed; snapshot {}\n",
        listed.len(),
        snapshot.id
    );
    for e in &listed {
        let (x, y, w, h) = match &frame {
            Some(f) => {
                let (x, y) = screen_to_image((e.bounds.x, e.bounds.y), f, scale);
                (x, y, (e.bounds.width as f64 / scale).round() as i64, (e.bounds.height as f64 / scale).round() as i64)
            }
            None => (e.bounds.x, e.bounds.y, e.bounds.width, e.bounds.height),
        };
        let mut tags: Vec<String> = Vec::new();
        if e.path.starts_with("m:") { tags.push("menu item".into()); }
        if let Some(k) = &e.keyboard_shortcut { tags.push(k.clone()); }
        if e.more > 0 { tags.push(format!("+{} inside: capture with ref: \"{}\" to drill in", e.more, e.id)); }
        if e.role == "OCRText" { tags.push("text read from the image".into()); }
        if e.actions.iter().any(|a| a == "AXPress") { tags.push("press".into()); }
        if e.actions.iter().any(|a| a == "AXShowMenu") { tags.push("menu".into()); }
        if e.actions.iter().any(|a| a == "AXSetValue") { tags.push("editable".into()); }
        if e.focused { tags.push("focused".into()); }
        let tags = if tags.is_empty() { String::new() } else { format!("  [{}]", tags.join(", ")) };
        let value = e.value.as_deref().map(|v| format!("  = \"{v}\"")).unwrap_or_default();
        text.push_str(&format!("{}  {}  \"{}\"{value}  at {x},{y} {w}×{h}{tags}\n", e.id, e.role, e.label));
    }
    if !filter.is_empty() || !role_filter.is_empty() {
        text.push_str(&format!(
            "({} of {elements_total} elements match \"{}\"{}.)\n",
            listed.len(),
            input["filter"].as_str().unwrap_or(""),
            if role_filter.is_empty() { String::new() } else { format!(" with role {role_filter}") }
        ));
    } else if listed.len() < elements_total {
        text.push_str(&format!(
            "({} unlabelled or inert elements not listed; raise max_elements or use ui find.)\n",
            elements_total - listed.len()
        ));
    }
    text.push_str(&format!(
        "Coordinates are pixels of the image below. Act by ref (input click, ref: \"B1\") or by pixel (input click, app: \"{}\", coordinate: [x, y]); every input action returns this view again, after the action.",
        if observed.app.is_empty() { "App" } else { &observed.app }
    ));

    Ok(Observed {
        result: ToolResult { payload: None, content: text, is_error: false, image_url: shot.image_url, http_status: None, terminal: false },
        snapshot,
    })
}

/// Hard ceiling on elements the AX script will walk. The cost is one Apple
/// Event round-trip per property, so this bounds wall-clock, not just payload —
/// truncating in Rust afterwards would pay for every element anyway.
#[cfg(target_os = "macos")]
const AX_SCRIPT_MAX_ELEMENTS: usize = 500;

/// A full accessibility walk is far slower than a flat one. Past this the
/// snapshot is not worth the wait, and returning partial beats hanging `see`.
const AX_CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);

/// What the accessibility walk observed, beyond the elements themselves.
#[derive(Clone, Default)]
struct AxCapture {
    /// The application actually inspected. With no `app` argument the target is
    /// whatever is frontmost, which the caller cannot otherwise identify.
    app: String,
    /// Windows the process has open, when known.
    windows: Option<usize>,
    /// Set when the accessibility walk itself failed or timed out. An empty
    /// element list then says nothing about the window; the caller must be
    /// told which of the two it is looking at.
    walk_error: Option<String>,
    /// Which layer the elements came from: `ax`, `ax-shallow`, or `vision`.
    via: String,
    /// The walk stopped at its element or time budget.
    truncated: bool,
    /// Why the native walk was not used, when it was not.
    fallback: Option<String>,
    /// Which budget cut a truncated walk, from the helper ("node budget (400)").
    cut_by: String,
    /// A menu was open when the walk ran; its items were listed first.
    menu_open: bool,
}

/// Capture AX elements with position information from the accessibility tree.
#[allow(unused_variables)]
async fn capture_ax_elements(app: &str) -> (AxCapture, Vec<desktop_snapshot::UIElement>) {
    #[cfg(target_os = "macos")]
    {
        let target = if app.is_empty() {
            "first application process whose frontmost is true".to_string()
        } else {
            format!("process \"{}\"", escape_applescript(app))
        };
        // `every UI element of window 1` alone returns only the window's direct
        // children — for most apps the title bar buttons plus one opaque content
        // group, which is what made `see` report a close button and nothing else.
        // Descending one level reaches the toolbar and tab-bar controls that are
        // actually clickable (measured on Finder: 7 elements -> 20, ~0.3s).
        //
        // Two levels, walked explicitly, rather than anything cleverer:
        // `entire contents` returns nothing at all through System Events, and a
        // nested `every UI element of every UI element of ...` specifier returns
        // lists grouped by parent that flatten into corrupt rows.
        let script = format!(
            r#"tell application "System Events"
    set theProc to {target}
    set output to "APP||" & (name of theProc) & linefeed
    set winCount to 0
    try
        set winCount to count of (windows of theProc)
    end try
    set output to output & "WINDOWS||" & (winCount as text) & linefeed
    set elemCount to 0
    tell theProc
        try
            repeat with elem in (every UI element of window 1)
                if elemCount > {max} then exit repeat
                try
                    set eRole to role of elem
                    set eName to name of elem
                    if eName is missing value then set eName to description of elem
                    if eName is missing value then set eName to value of elem
                    if eName is missing value then set eName to ""
                    set ePos to position of elem
                    set eSize to size of elem
                    set output to output & eRole & "||" & (eName as text) & "||" & (item 1 of ePos as text) & "," & (item 2 of ePos as text) & "," & (item 1 of eSize as text) & "," & (item 2 of eSize as text) & linefeed
                    set elemCount to elemCount + 1
                end try
                try
                    repeat with child in (every UI element of elem)
                        if elemCount > {max} then exit repeat
                        try
                            set cRole to role of child
                            set cName to name of child
                            if cName is missing value then set cName to description of child
                            if cName is missing value then set cName to value of child
                            if cName is missing value then set cName to ""
                            set cPos to position of child
                            set cSize to size of child
                            set output to output & cRole & "||" & (cName as text) & "||" & (item 1 of cPos as text) & "," & (item 2 of cPos as text) & "," & (item 1 of cSize as text) & "," & (item 2 of cSize as text) & linefeed
                            set elemCount to elemCount + 1
                        end try
                    end repeat
                end try
            end repeat
        end try
    end tell
    return output
end tell"#,
            target = target,
            max = AX_SCRIPT_MAX_ELEMENTS
        );
        match run_osascript_raw(&script, Some(AX_CAPTURE_TIMEOUT)).await {
            Ok(output) => {
                let (header, body) = split_ax_app_header(&output);
                let mut elements = parse_ax_output(body);
                assign_element_ids(&mut elements, &mut desktop_snapshot::RefBook::default());
                // Fall back to the requested app when the script produced no
                // header — never silently report an empty app name.
                let name = if header.app.is_empty() {
                    app.to_string()
                } else {
                    header.app
                };
                (
                    AxCapture {
                        app: name,
                        windows: header.windows,
                        walk_error: None,
                        ..Default::default()
                    },
                    elements,
                )
            }
            Err(e) => (
                AxCapture {
                    app: app.to_string(),
                    windows: None,
                    walk_error: Some(e),
                    ..Default::default()
                },
                Vec::new(),
            ),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Linux/Windows: return empty for now — element detection requires platform-specific work
        (
            AxCapture {
                app: app.to_string(),
                windows: None,
                walk_error: None,
                ..Default::default()
            },
            Vec::new(),
        )
    }
}

/// The header the AX script emits ahead of its element lines.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct AxHeader {
    app: String,
    /// Number of windows the process has open. `None` when the script did not
    /// report it. Zero windows is why an app that is genuinely running can
    /// still yield no elements — worth telling the caller apart from a failed
    /// walk.
    windows: Option<usize>,
}

/// Split the `APP||<name>` / `WINDOWS||<n>` header from the element lines.
#[cfg(target_os = "macos")]
fn split_ax_app_header(output: &str) -> (AxHeader, &str) {
    let mut header = AxHeader::default();
    let mut rest = output;

    if let Some(after) = rest.strip_prefix("APP||") {
        let (value, tail) = after.split_once('\n').unwrap_or((after, ""));
        header.app = value.trim().to_string();
        rest = tail;
    }
    if let Some(after) = rest.strip_prefix("WINDOWS||") {
        let (value, tail) = after.split_once('\n').unwrap_or((after, ""));
        header.windows = value.trim().parse().ok();
        rest = tail;
    }
    (header, rest)
}

async fn capture_screenshot(input: &serde_json::Value) -> ToolResult {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    let quality = input["quality"].as_str().unwrap_or("medium");
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let app = input["app"].as_str().unwrap_or("");
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let region = input["region"].as_str();

    // Use JPEG capture on macOS for low/medium to skip PNG decode overhead
    #[cfg(target_os = "macos")]
    let use_jpeg = quality != "high";
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let use_jpeg = false;

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    let ext = if use_jpeg { "jpg" } else { "png" };
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let tmp_path = format!("/tmp/nebo-capture-{}.{}", std::process::id(), ext);
    // Windows has no /tmp — write to the real temp dir or the PowerShell
    // capture saves to a nonexistent C:\tmp and the readback fails.
    #[cfg(windows)]
    let tmp_path = std::env::temp_dir()
        .join(format!("nebo-capture-{}.{}", std::process::id(), ext))
        .to_string_lossy()
        .into_owned();

    #[cfg(target_os = "macos")]
    let result = {
        let mut base_args: Vec<String> = vec!["-x".to_string()];
        if use_jpeg {
            base_args.extend_from_slice(&["-t".to_string(), "jpg".to_string()]);
        }

        // A window is captured by its frame. System Events cannot return a
        // window id for `-l` (error -1728 on every app), so that path only
        // ever produced the whole screen while claiming the window.
        let window_id = input["window_id"].as_u64();
        let region: Option<String> = if let Some(id) = window_id {
            let mut args = base_args.clone();
            args.extend_from_slice(&["-l".to_string(), id.to_string(), "-o".to_string()]);
            args.push(tmp_path.clone());
            let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let out = tokio::process::Command::new("screencapture").args(&arg_refs).output().await;
            // The rest of this function reads `result`; a window-id capture
            // produces it directly.
            match out {
                Ok(o) if o.status.success() => {
                    return match tokio::fs::read(&tmp_path).await {
                        Ok(bytes) => {
                            let _ = tokio::fs::remove_file(&tmp_path).await;
                            compress_and_encode(&bytes, quality)
                        }
                        Err(e) => ToolResult::error(format!("Failed to read screenshot: {}", e)),
                    };
                }
                Ok(o) => {
                    return ToolResult::error(format!(
                        "Screenshot of window {id} not taken: {}",
                        String::from_utf8_lossy(&o.stderr).trim()
                    ));
                }
                Err(e) => return ToolResult::error(format!("Failed to run screenshot tool: {e}")),
            }
        } else if !app.is_empty() {
            match window_frame(app, false).await {
                Ok(f) => Some(format!("{},{},{},{}", f.x, f.y, f.width, f.height)),
                Err(e) => {
                    return ToolResult::error(format!("Screenshot of {app} not taken: {e}. Capture without `app` for the whole screen."));
                }
            }
        } else {
            region.map(str::to_string)
        };
        let region = region.as_deref();
        if let Some(region) = region {
            let parts: Vec<&str> = region.split(',').collect();
            if parts.len() == 4 {
                let mut args = base_args;
                args.extend_from_slice(&["-R".to_string(), region.to_string()]);
                args.push(tmp_path.clone());
                let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                tokio::process::Command::new("screencapture")
                    .args(&arg_refs)
                    .output()
                    .await
            } else {
                return ToolResult::error(format!(
                    "Screenshot not taken: region must be 'x,y,w,h' (got '{}').",
                    region
                ));
            }
        } else {
            base_args.push(tmp_path.clone());
            let arg_refs: Vec<&str> = base_args.iter().map(|s| s.as_str()).collect();
            tokio::process::Command::new("screencapture")
                .args(&arg_refs)
                .output()
                .await
        }
    };

    #[cfg(target_os = "linux")]
    let result = {
        if which("gnome-screenshot") {
            if !app.is_empty() {
                // Focus the window first, then capture
                if which("xdotool") {
                    let _ =
                        run_command("xdotool", &["search", "--name", app, "windowactivate"]).await;
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
                x11_command("gnome-screenshot")
                    .args(["-w", "-f", &tmp_path])
                    .output()
                    .await
            } else {
                x11_command("gnome-screenshot")
                    .args(["-f", &tmp_path])
                    .output()
                    .await
            }
        } else if which("scrot") {
            if !app.is_empty() {
                x11_command("scrot")
                    .args(["-u", &tmp_path])
                    .output()
                    .await
            } else {
                x11_command("scrot")
                    .args([&tmp_path])
                    .output()
                    .await
            }
        } else if which("grim") {
            // Wayland
            if let Some(region) = region {
                // Use slurp format
                x11_command("grim")
                    .args(["-g", region, &tmp_path])
                    .output()
                    .await
            } else {
                x11_command("grim")
                    .args([&tmp_path])
                    .output()
                    .await
            }
        } else {
            return ToolResult::error("Screenshot requires gnome-screenshot, scrot, or grim");
        }
    };

    #[cfg(target_os = "windows")]
    let result = {
        let ps_script = format!(
            r#"Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap($screen.Width, $screen.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($screen.Location, [System.Drawing.Point]::Empty, $screen.Size)
$bmp.Save('{}')
$g.Dispose()
$bmp.Dispose()"#,
            escape_powershell(&tmp_path)
        );
        tokio::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps_script])
            .output()
            .await
    };

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = input;
        return ToolResult::error("Screenshot is not supported on this platform");
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    match result {
        Ok(output) if output.status.success() => match tokio::fs::read(&tmp_path).await {
            Ok(bytes) => {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                compress_and_encode(&bytes, quality)
            }
            Err(e) => ToolResult::error(format!("Failed to read screenshot: {}", e)),
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let code = output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            let mut msg = format!("Screenshot failed (exit {})", code);
            if !stderr.is_empty() {
                msg.push_str(": ");
                msg.push_str(&stderr);
            }
            if cfg!(target_os = "macos") {
                msg.push_str(
                    ". On macOS this usually means Screen Recording permission is missing for Nebo.",
                );
            }
            ToolResult::error(msg)
        }
        Err(e) => ToolResult::error(format!("Failed to run screenshot tool: {}", e)),
    }
}

/// Persist captured image bytes to `<data_dir>/files/<uuid>.<ext>` so the agent can
/// reference the file (e.g. share it to a channel) and the app can render it via the
/// `GET /api/v1/files/<name>` route. Returns (filename, absolute_path) on success.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn persist_capture(bytes: &[u8], ext: &str) -> Option<(String, String)> {
    let dir = config::data_dir().ok()?.join("files");
    std::fs::create_dir_all(&dir).ok()?;
    let filename = format!("capture-{}.{}", uuid::Uuid::new_v4(), ext);
    let path = dir.join(&filename);
    std::fs::write(&path, bytes).ok()?;
    Some((filename, path.to_string_lossy().into_owned()))
}

/// Build the screenshot ToolResult: persist the final bytes to `<data_dir>/files`,
/// surface the saved path + `/api/v1/files/<name>` URL in the content (so the agent
/// can share/display it), and keep the `data:` URI in `image_url` so the vision
/// sidecar still works for vision-incapable providers.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn image_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

fn finalize_capture(bytes: &[u8], mime: &str, dims: Option<(u32, u32)>, summary: &str) -> ToolResult {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let data_uri = format!("data:{};base64,{}", mime, b64);
    let ext = if mime.contains("png") {
        "png"
    } else if mime.contains("webp") {
        "webp"
    } else {
        "jpg"
    };
    let mut persist_path: Option<String> = None;
    let content = match persist_capture(bytes, ext) {
        Some((filename, abs)) => {
            persist_path = Some(abs.clone());
            format!(
                "{summary}\nSaved to: {abs}\nTo share or display this image, use the path {abs} (served at /api/v1/files/{filename})."
            )
        }
        None => summary.to_string(),
    };
    let saved = persist_path.clone();
    ToolResult {
        payload: dims.map(|(w, h)| serde_json::json!({ "kind": "capture", "width": w, "height": h, "path": saved })),
        content,
        is_error: false,
        image_url: Some(data_uri),
        http_status: None,
        terminal: false,
    }
}

/// Compress a screenshot to JPEG at the given quality level, resize, and base64-encode.
/// Accepts both PNG and JPEG input (auto-detected via magic bytes).
///
/// Quality levels:
/// - "low":    800px max width, 50% JPEG
/// - "medium": 1280px max width, 65% JPEG (default)
/// - "high":   original format, no compression
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn compress_and_encode(img_bytes: &[u8], quality: &str) -> ToolResult {
    use image::ImageReader;
    use std::io::Cursor;

    let is_jpeg = img_bytes.len() >= 2 && img_bytes[0] == 0xFF && img_bytes[1] == 0xD8;

    if quality == "high" {
        let mime = if is_jpeg { "image/jpeg" } else { "image/png" };
        return finalize_capture(
            img_bytes,
            mime,
            image_dims(img_bytes),
            &format!("Screenshot captured (high quality, {} bytes)", img_bytes.len()),
        );
    }

    let (max_width, jpeg_quality) = match quality {
        "low" => (800u32, 50u8),
        _ => (1280u32, 65u8), // "medium" or any other value
    };

    let img = match ImageReader::new(Cursor::new(img_bytes))
        .with_guessed_format()
        .and_then(|r| {
            r.decode()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        }) {
        Ok(img) => img,
        Err(e) => {
            // Fall back to raw bytes if decode fails
            let mime = if is_jpeg { "image/jpeg" } else { "image/png" };
            tracing::warn!(error = %e, "failed to decode screenshot for compression, returning raw");
            return finalize_capture(
                img_bytes,
                mime,
                image_dims(img_bytes),
                &format!(
                    "Screenshot captured (requested {} quality but compression failed; returning original {} bytes)",
                    quality,
                    img_bytes.len()
                ),
            );
        }
    };

    // Resize if wider than max_width
    let img = if img.width() > max_width {
        img.resize(max_width, u32::MAX, image::imageops::FilterType::Triangle)
    } else {
        img
    };

    // Encode to JPEG
    let mut jpeg_buf = Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_buf, jpeg_quality);
    match img.write_with_encoder(encoder) {
        Ok(()) => {
            let jpeg_bytes = jpeg_buf.into_inner();
            let original_kib = img_bytes.len() / 1024;
            let compressed_kib = jpeg_bytes.len() / 1024;
            finalize_capture(
                &jpeg_bytes,
                "image/jpeg",
                Some((img.width(), img.height())),
                &format!(
                    "Screenshot captured: {}x{}, JPEG q{}, {} KiB (source {} KiB)",
                    img.width(),
                    img.height(),
                    jpeg_quality,
                    compressed_kib,
                    original_kib
                ),
            )
        }
        Err(e) => {
            let mime = if is_jpeg { "image/jpeg" } else { "image/png" };
            tracing::warn!(error = %e, "JPEG encode failed, returning raw image");
            finalize_capture(
                img_bytes,
                mime,
                image_dims(img_bytes),
                &format!(
                    "Screenshot captured (requested {} quality but compression failed; returning original {} bytes)",
                    quality,
                    img_bytes.len()
                ),
            )
        }
    }
}

// --- UI / Accessibility ---

#[allow(unused_variables)]
async fn handle_ui(
    action: &str,
    input: &serde_json::Value,
    snapshot_store: &tokio::sync::Mutex<SnapshotStore>,
    ax_cache: &AxCache,
) -> ToolResult {
    let app = input["app"].as_str().unwrap_or("");
    let role = input["role"].as_str().unwrap_or("");
    let label = input["label"].as_str().unwrap_or("");
    let value = input["value"].as_str().unwrap_or("");

    match action {
        "tree" => {
            if app.is_empty() {
                return ToolResult::error(errors::missing_param("tree", "app", "os(resource: \"ui\", action: \"tree\", app: \"Safari\")"));
            }
            ui_tree(app, role).await
        }
        // find is an observe with a filter: same walk, same refs, same image,
        // so a ref it returns is one the next act can use.
        "find" => {
            if label.is_empty() && role.is_empty() {
                return ToolResult::error(errors::missing_param("find", "label", "os(resource: \"ui\", action: \"find\", app: \"Safari\", label: \"Search\")"));
            }
            let query = serde_json::json!({ "app": app, "max_elements": 500, "filter": label, "role": role, "quality": input["quality"] });
            match observe(app, &query, snapshot_store, ax_cache).await {
                Ok(o) => o.result,
                Err(e) => e,
            }
        }
        "click" => {
            if label.is_empty() {
                return ToolResult::error(errors::missing_param("click", "label", "os(resource: \"ui\", action: \"click\", app: \"Safari\", label: \"Submit\")"));
            }
            ui_click(app, label).await
        }
        "get_value" => {
            if label.is_empty() {
                return ToolResult::error(errors::missing_param("get_value", "label", "os(resource: \"ui\", action: \"get_value\", app: \"Safari\", label: \"URL\")"));
            }
            ui_get_value(app, label).await
        }
        "set_value" => {
            if label.is_empty() || value.is_empty() {
                return ToolResult::error(errors::missing_param("set_value", "label/value", "os(resource: \"ui\", action: \"set_value\", app: \"Safari\", label: \"URL\", value: \"https://example.com\")"));
            }
            ui_set_value(app, label, value).await
        }
        "list_apps" => ui_list_apps().await,
        _ => ToolResult::error(format!(
            "Unknown ui action '{}'. Use: tree, find, click, get_value, set_value, list_apps",
            action
        )),
    }
}

#[allow(unused_variables)]
async fn ui_tree(app: &str, role: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let role_filter = if role.is_empty() {
            String::new()
        } else {
            format!(" whose role is \"{}\"", escape_applescript(role))
        };
        let script = format!(
            r#"tell application "System Events"
    tell process "{}"
        set uiTree to ""
        repeat with elem in (every UI element of window 1{})
            set uiTree to uiTree & (role of elem) & " | " & (description of elem) & " | " & (name of elem) & linefeed
        end repeat
        return uiTree
    end tell
end tell"#,
            escape_applescript(app),
            role_filter
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        // AT-SPI2 via python3 for accessibility tree
        if which("python3") {
            let py_script = format!(
                r#"import subprocess, json
try:
    r = subprocess.run(['busctl', 'call', 'org.a11y.Bus', '/org/a11y/bus', 'org.a11y.Bus', 'GetAddress'], capture_output=True, text=True)
    print('AT-SPI bus:', r.stdout.strip())
    # Fallback: use gdbus for basic tree
    r2 = subprocess.run(['gdbus', 'call', '--session', '--dest', 'org.a11y.atspi.Registry', '--object-path', '/org/a11y/atspi/accessible/root', '--method', 'org.a11y.atspi.Accessible.GetChildren'], capture_output=True, text=True)
    print(r2.stdout[:2000])
except Exception as e:
    print(f'AT-SPI error: {{e}}')"#
            );
            return run_command("python3", &["-c", &py_script]).await;
        }
        return ToolResult::error("UI accessibility requires python3 with AT-SPI2 support");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, '{}')
$app = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $cond)
if ($app) {{
    $all = $app.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($e in $all) {{
        "$($e.Current.ControlType.ProgrammaticName) | $($e.Current.Name) | $($e.Current.AutomationId)"
    }}
}} else {{ "Application '{}' not found" }}"#,
            escape_powershell(app),
            escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("UI accessibility is not supported on this platform")
}

async fn ui_click(app: &str, label: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let target = if app.is_empty() {
            "first application process whose frontmost is true".to_string()
        } else {
            format!("process \"{}\"", escape_applescript(app))
        };
        let script = format!(
            r#"tell application "System Events"
    tell {}
        click (first UI element of window 1 whose name is "{}")
    end tell
end tell"#,
            target,
            escape_applescript(label)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error("UI click is not available on Linux. Use capture(action: see) for a screenshot and input(action: click, coordinate: [x,y]) instead.");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, '{}')
$el = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
if ($el) {{
    $pattern = $el.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
    $pattern.Invoke()
    "Clicked: {}"
}} else {{ "Element '{}' not found" }}"#,
            escape_powershell(label),
            escape_powershell(label),
            escape_powershell(label)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("UI click is not supported on this platform")
}

#[allow(unused_variables)]
async fn ui_get_value(app: &str, label: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let target = if app.is_empty() {
            "first application process whose frontmost is true".to_string()
        } else {
            format!("process \"{}\"", escape_applescript(app))
        };
        let script = format!(
            r#"tell application "System Events"
    tell {}
        return value of (first UI element of window 1 whose name is "{}")
    end tell
end tell"#,
            target,
            escape_applescript(label)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error(
            "UI get_value is not available on Linux. Use capture(action: see) for a screenshot and read the value from it.",
        );
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, '{}')
$el = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
if ($el) {{
    try {{ $p = $el.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern); $p.Current.Value }}
    catch {{ $el.Current.Name }}
}} else {{ "Element '{}' not found" }}"#,
            escape_powershell(label),
            escape_powershell(label)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("UI get_value is not supported on this platform")
}

#[allow(unused_variables)]
async fn ui_set_value(app: &str, label: &str, value: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let target = if app.is_empty() {
            "first application process whose frontmost is true".to_string()
        } else {
            format!("process \"{}\"", escape_applescript(app))
        };
        let script = format!(
            r#"tell application "System Events"
    tell {}
        set value of (first UI element of window 1 whose name is "{}") to "{}"
    end tell
end tell"#,
            target,
            escape_applescript(label),
            escape_applescript(value)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error(
            "UI set_value is not available on Linux. Use input(action: click, coordinate: [x,y]) then input(action: type, text: ...) instead.",
        );
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, '{}')
$el = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
if ($el) {{
    $p = $el.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
    $p.SetValue('{}')
    "Value set"
}} else {{ "Element '{}' not found" }}"#,
            escape_powershell(label),
            escape_powershell(value),
            escape_powershell(label)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("UI set_value is not supported on this platform")
}

async fn ui_list_apps() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = r#"tell application "System Events"
    set appList to ""
    repeat with proc in (every process whose background only is false)
        set appList to appList & name of proc & linefeed
    end repeat
    return appList
end tell"#;
        return run_osascript(script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("wmctrl") {
            return run_command("wmctrl", &["-l"]).await;
        }
        // Fallback to listing desktop files
        return run_command("ls", &["/usr/share/applications/"]).await;
    }
    #[cfg(target_os = "windows")]
    {
        let script = "Get-Process | Where-Object { $_.MainWindowTitle -ne '' } | Select-Object ProcessName, MainWindowTitle | Format-Table -AutoSize";
        return run_powershell(script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("UI list_apps is not supported on this platform")
}

// --- Menu ---

#[allow(unused_variables)]
async fn handle_menu(action: &str, input: &serde_json::Value) -> ToolResult {
    let app = input["app"].as_str().unwrap_or("");
    let name = input["name"].as_str().unwrap_or("");

    match action {
        "list" => {
            if app.is_empty() {
                return ToolResult::error(errors::missing_param("list", "app", "os(resource: \"menu\", action: \"list\", app: \"Safari\")"));
            }
            match menu_via_helper(app, name, false).await {
                Some(r) => r,
                None => menu_list(app).await,
            }
        }
        "menus" => {
            if app.is_empty() {
                return ToolResult::error(errors::missing_param("menus", "app", "os(resource: \"menu\", action: \"menus\", app: \"Safari\")"));
            }
            match menu_via_helper(app, "", true).await {
                Some(r) => r,
                None => menu_menus(app).await,
            }
        }
        "click" | "menu" | "choose" => {
            // No app: the app in front owns the menu bar.
            let front;
            let app = if app.is_empty() {
                front = ax_native::frontmost().await.unwrap_or_default();
                front.as_str()
            } else {
                app
            };
            if app.is_empty() || name.is_empty() {
                return ToolResult::error(errors::missing_param("click", "app/name", "os(resource: \"menu\", action: \"click\", app: \"Safari\", name: \"File > New Window\")"));
            }
            let args = vec!["menu".to_string(), "--app".into(), app.to_string(), "--path".into(), name.to_string()];
            match ax_native::run(&args, Duration::from_secs(8)).await {
                Ok(out) => {
                    let v = ax_native::json_lines(&out).into_iter().next().unwrap_or_default();
                    let pressed = v["pressed"].as_str().unwrap_or(name);
                    if v["opened"].as_bool() == Some(true) {
                        ToolResult::ok(format!("Opened the {pressed} menu via accessibility. Capture {app} to see its items as refs, or click one with name: \"{pressed} > <item>\"."))
                    } else {
                        ToolResult::ok(format!("Chose {pressed} via accessibility; delivered{}.", if v["menu_closed"].as_bool() == Some(true) { " (the menu closed)" } else { "" }))
                    }
                }
                Err(e) if e.contains("helper unavailable") || e.contains("macOS-only") => menu_click(app, name).await,
                Err(e) => ToolResult::error(format!("Menu {name}: {e}")),
            }
        }
        "status" => menu_status_list().await,
        "click_status" => {
            if name.is_empty() {
                return ToolResult::error(errors::missing_param("click_status", "name", "os(resource: \"menu\", action: \"click_status\", name: \"Wi-Fi\")"));
            }
            menu_click_status(name).await
        }
        _ => ToolResult::error(format!(
            "Unknown menu action '{}'. Use: list, menus, click, status, click_status",
            action
        )),
    }
}

#[allow(unused_variables)]
/// The menu bar (or one menu, `path`), read through accessibility. `all`
/// lists every top-level menu with its items. `None`: the helper is not
/// available here, use the AppleScript reading.
async fn menu_via_helper(app: &str, path: &str, all: bool) -> Option<ToolResult> {
    let list = |p: &str| {
        let mut a = vec!["menu-list".to_string(), "--app".into(), app.to_string()];
        if !p.is_empty() {
            a.extend(["--path".into(), p.to_string()]);
        }
        a
    };
    let line = |v: &serde_json::Value| {
        let mut l = v["title"].as_str().unwrap_or("").to_string();
        if let Some(k) = v["shortcut"].as_str() {
            l.push_str(&format!("  {k}"));
        }
        if v["submenu"].as_bool() == Some(true) {
            l.push_str("  ▸");
        }
        if v["enabled"].as_bool() == Some(false) {
            l.push_str("  (disabled)");
        }
        l
    };
    let top = match ax_native::run(&list(path), Duration::from_secs(6)).await {
        Ok(out) => ax_native::json_lines(&out),
        Err(e) if e.contains("helper unavailable") || e.contains("macOS-only") => return None,
        Err(e) => return Some(ToolResult::error(format!("Menu of {app}: {e}"))),
    };
    let mut text = String::new();
    for v in &top {
        text.push_str(&line(v));
        text.push('\n');
        if all && v["title"].as_str().is_some_and(|t| t != "Apple") {
            if let Ok(out) = ax_native::run(&list(v["title"].as_str().unwrap_or("")), Duration::from_secs(6)).await {
                for item in ax_native::json_lines(&out) {
                    text.push_str(&format!("  {}\n", line(&item)));
                }
            }
        }
    }
    text.push_str("Choose one with os(resource: \"menu\", action: \"click\", app, name: \"Menu > Item\").");
    Some(ToolResult::ok(text))
}

async fn menu_list(app: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            r#"tell application "System Events"
    tell process "{}"
        set menuItems to ""
        repeat with menuBar in menu bars
            repeat with menuBarItem in menu bar items of menuBar
                set menuItems to menuItems & name of menuBarItem & linefeed
            end repeat
        end repeat
        return menuItems
    end tell
end tell"#,
            escape_applescript(app)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "windows")]
    {
        // The old PowerShell script only proved the process existed and then
        // answered "Menu bar found" with no menus in it.
        return ToolResult::error(
            "Menu enumeration is not implemented on Windows; use ui(action: tree) to read the window's controls.",
        );
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error(
            "Menu bar access is not supported on Linux (most apps use client-side decorations)",
        );
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Menu list is not supported on this platform")
}

#[allow(unused_variables)]
async fn menu_menus(app: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            r#"tell application "System Events"
    tell process "{}"
        set allMenus to ""
        repeat with menuBarItem in menu bar items of menu bar 1
            set menuName to name of menuBarItem
            set allMenus to allMenus & menuName & ":" & linefeed
            try
                repeat with menuItem in menu items of menu 1 of menuBarItem
                    set allMenus to allMenus & "  " & name of menuItem & linefeed
                end repeat
            end try
        end repeat
        return allMenus
    end tell
end tell"#,
            escape_applescript(app)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "windows")]
    {
        return ToolResult::error(
            "Menu enumeration on Windows requires UI Automation (use ui tree instead)",
        );
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error("Menu enumeration is not supported on Linux");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Menu menus is not supported on this platform")
}

#[allow(unused_variables)]
async fn menu_click(app: &str, name: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        // Try to click a menu item by walking menu bar items
        let parts: Vec<&str> = name.split('>').map(|s| s.trim()).collect();
        let script = if parts.len() == 2 {
            format!(
                r#"tell application "System Events"
    tell process "{}"
        click menu item "{}" of menu 1 of menu bar item "{}" of menu bar 1
    end tell
end tell"#,
                escape_applescript(app),
                escape_applescript(parts[1]),
                escape_applescript(parts[0])
            )
        } else {
            format!(
                r#"tell application "System Events"
    tell process "{}"
        set found to false
        repeat with menuBarItem in menu bar items of menu bar 1
            try
                click menu item "{}" of menu 1 of menuBarItem
                set found to true
                exit repeat
            end try
        end repeat
        if not found then error "Menu item '{}' not found"
    end tell
end tell"#,
                escape_applescript(app),
                escape_applescript(name),
                escape_applescript(name)
            )
        };
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "windows")]
    {
        return ToolResult::error(
            "Menu click on Windows requires UI Automation (use ui click instead)",
        );
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error("Menu click is not supported on Linux");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Menu click is not supported on this platform")
}

async fn menu_status_list() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = r#"tell application "System Events"
    set statusItems to ""
    repeat with menuExtra in menu bar items of menu bar 2 of application process "SystemUIServer"
        set statusItems to statusItems & name of menuExtra & linefeed
    end repeat
    return statusItems
end tell"#;
        return run_osascript(script).await;
    }
    #[cfg(target_os = "windows")]
    {
        let script = r#"$tray = New-Object -ComObject Shell.Application
$tray.NameSpace('shell:::{05d7b0f4-2121-4eff-bf6b-ed3f69b894d9}').Items() | ForEach-Object { $_.Name }"#;
        return run_powershell(script).await;
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error("Status menu listing is not supported on Linux");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Status menu listing is not supported on this platform")
}

#[allow(unused_variables)]
async fn menu_click_status(name: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            r#"tell application "System Events"
    tell menu bar 2 of application process "SystemUIServer"
        click menu bar item "{}"
    end tell
end tell"#,
            escape_applescript(name)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "windows")]
    {
        return ToolResult::error("Status menu click on Windows is not supported");
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error("Status menu click is not supported on Linux");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Status menu click is not supported on this platform")
}

// --- Dialog ---

#[allow(unused_variables)]
async fn handle_dialog(action: &str, input: &serde_json::Value) -> ToolResult {
    let app = input["app"].as_str().unwrap_or("");
    let name = input["name"].as_str().unwrap_or("");
    let value = input["value"].as_str().unwrap_or("");

    match action {
        "detect" => dialog_detect(app).await,
        "list" => dialog_list(app).await,
        "click" => {
            if name.is_empty() {
                return ToolResult::error(errors::missing_param("click", "name", "os(resource: \"dialog\", action: \"click\", name: \"OK\")"));
            }
            dialog_click(app, name).await
        }
        "fill" => {
            if value.is_empty() {
                return ToolResult::error(errors::missing_param("fill", "value", "os(resource: \"dialog\", action: \"fill\", value: \"my input\")"));
            }
            dialog_fill(app, name, value).await
        }
        "dismiss" => dialog_dismiss(app).await,
        _ => ToolResult::error(format!(
            "Unknown dialog action '{}'. Use: detect, list, click, fill, dismiss",
            action
        )),
    }
}

#[allow(unused_variables)]
async fn dialog_detect(app: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let target = if app.is_empty() {
            "first application process whose frontmost is true".to_string()
        } else {
            format!("process \"{}\"", escape_applescript(app))
        };
        let script = format!(
            r#"tell application "System Events"
    tell {}
        set sheetCount to count of sheets of window 1
        set dialogCount to count of windows whose subrole is "AXDialog" or subrole is "AXSheet"
        if sheetCount > 0 or dialogCount > 0 then
            return "Dialog detected: " & dialogCount & " dialog windows, " & sheetCount & " sheets on the front window."
        else
            return "No dialog detected"
        end if
    end tell
end tell"#,
            target
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "windows")]
    {
        let script = r#"Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Window)
$windows = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $cond)
$dialogs = @()
foreach ($w in $windows) { if ($w.Current.ClassName -match 'Dialog|#32770') { $dialogs += $w.Current.Name } }
if ($dialogs.Count -gt 0) { "Dialogs: $($dialogs -join ', ')" } else { "No dialog detected" }"#;
        return run_powershell(script).await;
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error("Dialog detection is not supported on Linux");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Dialog detection is not supported on this platform")
}

#[allow(unused_variables)]
async fn dialog_list(app: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let target = if app.is_empty() {
            "first application process whose frontmost is true".to_string()
        } else {
            format!("process \"{}\"", escape_applescript(app))
        };
        let script = format!(
            r#"tell application "System Events"
    tell {}
        set elems to ""
        try
            repeat with elem in (every UI element of sheet 1 of window 1)
                set elems to elems & (role of elem) & " | " & (name of elem) & linefeed
            end repeat
        on error
            repeat with elem in (every UI element of window 1)
                set elems to elems & (role of elem) & " | " & (name of elem) & linefeed
            end repeat
        end try
        return elems
    end tell
end tell"#,
            target
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "windows")]
    {
        let script = r#"Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Window)
$windows = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $cond)
foreach ($w in $windows) {
    if ($w.Current.ClassName -match 'Dialog|#32770') {
        $children = $w.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
        foreach ($c in $children) { "$($c.Current.ControlType.ProgrammaticName) | $($c.Current.Name)" }
    }
}"#;
        return run_powershell(script).await;
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error("Dialog listing is not supported on Linux");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Dialog listing is not supported on this platform")
}

#[allow(unused_variables)]
async fn dialog_click(app: &str, name: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let target = if app.is_empty() {
            "first application process whose frontmost is true".to_string()
        } else {
            format!("process \"{}\"", escape_applescript(app))
        };
        let script = format!(
            r#"tell application "System Events"
    tell {}
        try
            click button "{}" of sheet 1 of window 1
        on error
            click button "{}" of window 1
        end try
    end tell
end tell"#,
            target,
            escape_applescript(name),
            escape_applescript(name)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, '{}')
$el = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
if ($el) {{ $el.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke(); "Clicked: {}" }}
else {{ "Button '{}' not found" }}"#,
            escape_powershell(name),
            escape_powershell(name),
            escape_powershell(name)
        );
        return run_powershell(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error("Dialog click is not supported on Linux");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Dialog click is not supported on this platform")
}

#[allow(unused_variables)]
async fn dialog_fill(app: &str, name: &str, value: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let target = if app.is_empty() {
            "first application process whose frontmost is true".to_string()
        } else {
            format!("process \"{}\"", escape_applescript(app))
        };
        let field_target = if name.is_empty() {
            "first text field".to_string()
        } else {
            format!("text field \"{}\"", escape_applescript(name))
        };
        let script = format!(
            r#"tell application "System Events"
    tell {}
        try
            set value of {} of sheet 1 of window 1 to "{}"
        on error
            set value of {} of window 1 to "{}"
        end try
    end tell
end tell"#,
            target,
            field_target,
            escape_applescript(value),
            field_target,
            escape_applescript(value)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Edit)
$el = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
if ($el) {{ $p = $el.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern); $p.SetValue('{}'); "Field filled" }}
else {{ "Text field not found" }}"#,
            escape_powershell(value)
        );
        return run_powershell(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error("Dialog fill is not supported on Linux");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Dialog fill is not supported on this platform")
}

#[allow(unused_variables)]
async fn dialog_dismiss(app: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let target = if app.is_empty() {
            "first application process whose frontmost is true".to_string()
        } else {
            format!("process \"{}\"", escape_applescript(app))
        };
        let script = format!(
            r#"tell application "System Events"
    tell {}
        key code 53 -- Escape
    end tell
end tell"#,
            target
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "windows")]
    {
        let script = "$wsh = New-Object -ComObject WScript.Shell; $wsh.SendKeys('{ESC}')";
        return run_powershell(script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("xdotool") {
            return run_command("xdotool", &["key", "Escape"]).await;
        }
        return ToolResult::error("Dialog dismiss requires xdotool");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Dialog dismiss is not supported on this platform")
}

// --- Space / Virtual Desktop ---

#[allow(unused_variables)]
async fn handle_space(action: &str, input: &serde_json::Value) -> ToolResult {
    let index = input["index"].as_i64().unwrap_or(0);
    let app = input["app"].as_str().unwrap_or("");

    match action {
        "list" => space_list().await,
        "switch" => {
            if index == 0 {
                return ToolResult::error(errors::missing_param("switch", "index", "os(resource: \"space\", action: \"switch\", index: 2)"));
            }
            space_switch(index).await
        }
        "move_window" => {
            if index == 0 {
                return ToolResult::error(errors::missing_param("move_window", "index", "os(resource: \"space\", action: \"move_window\", index: 2)"));
            }
            space_move_window(app, index).await
        }
        _ => ToolResult::error(format!(
            "Unknown space action '{}'. Use: list, switch, move_window",
            action
        )),
    }
}

async fn space_list() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        // There is no supported way to enumerate Mission Control spaces. The
        // old `defaults read` script printed two unrelated integers that read
        // as space numbers.
        ToolResult::error(
            "Listing Mission Control spaces is not available on macOS. Switch by number with space(action: \"switch\", index: N), 1 to 9.",
        )
    }
    #[cfg(target_os = "linux")]
    {
        if which("wmctrl") {
            return run_command("wmctrl", &["-d"]).await;
        }
        return ToolResult::error("Virtual desktop listing requires wmctrl");
    }
    #[cfg(target_os = "windows")]
    {
        // PowerShell has no supported API for enumerating virtual desktops.
        return ToolResult::error(
            "Virtual desktop enumeration is not available on Windows; space(action: switch, index: N) moves N desktops to the right.",
        );
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Virtual desktops are not supported on this platform")
}

#[allow(unused_variables)]
async fn space_switch(index: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        // Switch to space via keyboard shortcut (Ctrl+number)
        let key_code = match index {
            1 => "18",
            2 => "19",
            3 => "20",
            4 => "21",
            5 => "23",
            6 => "22",
            7 => "26",
            8 => "28",
            9 => "25",
            _ => return ToolResult::error("Space index must be 1-9"),
        };
        let script = format!(
            "tell application \"System Events\" to key code {} using control down",
            key_code
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("wmctrl") {
            let idx = (index - 1).to_string();
            return run_command("wmctrl", &["-s", &idx]).await;
        }
        return ToolResult::error("Space switching requires wmctrl");
    }
    #[cfg(target_os = "windows")]
    {
        // Win+Ctrl+Left/Right to switch desktops
        let script = if index > 0 {
            let mut cmds = String::new();
            for _ in 0..index.unsigned_abs() {
                cmds.push_str("$wsh.SendKeys('^#{RIGHT}'); Start-Sleep -Milliseconds 200; ");
            }
            format!("$wsh = New-Object -ComObject WScript.Shell; {}", cmds)
        } else {
            return ToolResult::error("Space index must be positive");
        };
        let result = run_powershell(&script).await;
        if result.is_error {
            return result;
        }
        return ToolResult::ok(format!(
            "Pressed Ctrl+Win+Right {} times (relative move; Windows has no absolute desktop switch).",
            index
        ));
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Space switching is not supported on this platform")
}

#[allow(unused_variables)]
async fn space_move_window(app: &str, index: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        // macOS doesn't have a clean scripting API for moving windows between spaces
        return ToolResult::error(
            "Moving windows between spaces on macOS requires third-party tools (e.g., yabai)",
        );
    }
    #[cfg(target_os = "linux")]
    {
        if which("wmctrl") {
            let desktop = (index - 1).to_string();
            if app.is_empty() {
                return run_command("wmctrl", &["-r", ":ACTIVE:", "-t", &desktop]).await;
            }
            return run_command("wmctrl", &["-r", app, "-t", &desktop]).await;
        }
        return ToolResult::error("Moving windows requires wmctrl");
    }
    #[cfg(target_os = "windows")]
    {
        return ToolResult::error(
            "Moving windows between virtual desktops requires Windows API (not available via PowerShell)",
        );
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Moving windows between spaces is not supported on this platform")
}

// --- Shortcut ---

#[allow(unused_variables)]
async fn handle_shortcut(action: &str, input: &serde_json::Value) -> ToolResult {
    let name = input["name"].as_str().unwrap_or("");

    match action {
        "list" => shortcut_list().await,
        "run" => {
            if name.is_empty() {
                return ToolResult::error(errors::missing_param("run", "name", "os(resource: \"shortcut\", action: \"run\", name: \"My Shortcut\")"));
            }
            shortcut_run(name).await
        }
        _ => ToolResult::error(format!(
            "Unknown shortcut action '{}'. Use: list, run",
            action
        )),
    }
}

async fn shortcut_list() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        return run_command("shortcuts", &["list"]).await;
    }
    #[cfg(target_os = "linux")]
    {
        // List scripts in common locations
        let mut result = String::new();
        for dir in &["~/.local/bin", "~/bin", "/usr/local/bin"] {
            let expanded = dir.replace('~', &std::env::var("HOME").unwrap_or_default());
            if let Ok(entries) = std::fs::read_dir(&expanded) {
                result.push_str(&format!("Executables in {}:\n", dir));
                for entry in entries.flatten() {
                    result.push_str(&format!("  {}\n", entry.file_name().to_string_lossy()));
                }
            }
        }
        if result.is_empty() {
            return ToolResult::ok("No shortcuts found in ~/.local/bin, ~/bin, or /usr/local/bin");
        }
        return ToolResult::ok(result);
    }
    #[cfg(target_os = "windows")]
    {
        return run_powershell(
            "$all = @(Get-Command -CommandType Application,Script); \"First 50 of $($all.Count) commands:\"; $all | Select-Object -First 50 Name, Source | Format-Table -AutoSize",
        )
        .await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Shortcut listing is not supported on this platform")
}

#[allow(unused_variables)]
async fn shortcut_run(name: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        return run_command("shortcuts", &["run", name]).await;
    }
    #[cfg(target_os = "linux")]
    {
        // Try to run from common paths
        let home = std::env::var("HOME").unwrap_or_default();
        let paths = vec![
            format!("{}/.local/bin/{}", home, name),
            format!("{}/bin/{}", home, name),
            name.to_string(),
        ];
        for path in &paths {
            if std::path::Path::new(path).exists() {
                return run_command(path, &[]).await;
            }
        }
        // Fallback: run it as a PATH command, but only when it exists there;
        // otherwise the caller gets the three places that were searched.
        if !which(name) {
            return ToolResult::error(format!(
                "Shortcut '{}' not found in ~/.local/bin, ~/bin, or PATH",
                name
            ));
        }
        return run_command(name, &[]).await;
    }
    #[cfg(target_os = "windows")]
    {
        return run_powershell(&format!("& '{}'", escape_powershell(name))).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("Shortcut run is not supported on this platform")
}

// --- TTS ---

#[allow(unused_variables)]
async fn handle_tts(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "speak" => {
            let text = input["text"].as_str().unwrap_or("");
            if text.is_empty() {
                return ToolResult::error(errors::missing_param("speak", "text", "os(resource: \"tts\", action: \"speak\", text: \"Hello world\")"));
            }
            let voice = input["voice"].as_str().unwrap_or("");
            let rate = input["rate"].as_i64().unwrap_or(0);
            tts_speak(text, voice, rate).await
        }
        _ => ToolResult::error(format!("Unknown tts action '{}'. Use: speak", action)),
    }
}

#[allow(unused_variables)]
async fn tts_speak(text: &str, voice: &str, rate: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let mut args = vec![text.to_string()];
        if !voice.is_empty() {
            args.push("-v".to_string());
            args.push(voice.to_string());
        }
        if rate > 0 {
            args.push("-r".to_string());
            args.push(rate.to_string());
        }
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        return run_command("say", &args_ref).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("espeak") {
            let mut args = vec![text.to_string()];
            if rate > 0 {
                args.push("-s".to_string());
                args.push(rate.to_string());
            }
            if !voice.is_empty() {
                args.push("-v".to_string());
                args.push(voice.to_string());
            }
            let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            return run_command("espeak", &args_ref).await;
        }
        if which("spd-say") {
            let mut args = vec![text.to_string()];
            if rate > 0 {
                args.push("-r".to_string());
                args.push(rate.to_string());
            }
            let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            return run_command("spd-say", &args_ref).await;
        }
        return ToolResult::error(
            "TTS requires espeak or spd-say (install espeak or speech-dispatcher)",
        );
    }
    #[cfg(target_os = "windows")]
    {
        let rate_str = if rate > 0 {
            format!(
                "$synth.Rate = {}\n",
                (rate as f64 / 30.0).clamp(-10.0, 10.0) as i64
            )
        } else {
            String::new()
        };
        let voice_str = if !voice.is_empty() {
            format!("$synth.SelectVoice('{}')\n", escape_powershell(voice))
        } else {
            String::new()
        };
        let script = format!(
            r#"Add-Type -AssemblyName System.Speech
$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
{}{}$synth.Speak('{}')"#,
            voice_str,
            rate_str,
            escape_powershell(text)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("TTS is not supported on this platform")
}

// --- Dock (macOS only) ---

#[allow(unused_variables)]
async fn handle_dock(action: &str, input: &serde_json::Value) -> ToolResult {
    let app = input["app"].as_str().unwrap_or("");

    match action {
        "badges" => dock_badges().await,
        "recent" => dock_recent().await,
        "is_running" => {
            if app.is_empty() {
                return ToolResult::error(errors::missing_param("is_running", "app", "os(resource: \"dock\", action: \"is_running\", app: \"Safari\")"));
            }
            dock_is_running(app).await
        }
        _ => ToolResult::error(format!(
            "Unknown dock action '{}'. Use: badges, recent, is_running",
            action
        )),
    }
}

#[allow(unused_variables)]
async fn dock_badges() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = r#"tell application "System Events"
    set badgeInfo to ""
    repeat with proc in (every process whose background only is false)
        try
            set procName to name of proc
            set badgeInfo to badgeInfo & procName & linefeed
        end try
    end repeat
    return badgeInfo
end tell"#;
        // System Events cannot read Dock badge counts; the script above lists
        // running foreground apps, and the header must say exactly that.
        let result = run_osascript(script).await;
        if result.is_error {
            return result;
        }
        ToolResult::ok(format!(
            "Running apps (badge counts not available):\n{}",
            result.content
        ))
    }
    #[cfg(not(target_os = "macos"))]
    ToolResult::error("Dock badges are only available on macOS")
}

#[allow(unused_variables)]
async fn dock_recent() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let result = run_command("defaults", &["read", "com.apple.dock", "recent-apps"]).await;
        if result.is_error {
            return result;
        }
        ToolResult::ok(format!(
            "Raw plist from com.apple.dock recent-apps:\n{}",
            result.content
        ))
    }
    #[cfg(not(target_os = "macos"))]
    ToolResult::error("Dock recent items are only available on macOS")
}

#[allow(unused_variables)]
async fn dock_is_running(app: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            r#"tell application "System Events"
    if (exists process "{}") then
        return "{} is running"
    else
        return "{} is not running"
    end if
end tell"#,
            escape_applescript(app),
            escape_applescript(app),
            escape_applescript(app)
        );
        return run_osascript(&script).await;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        ToolResult::error("Dock is_running is only available on macOS")
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

// --- macOS helpers ---

#[cfg(target_os = "macos")]
async fn run_osascript(script: &str) -> ToolResult {
    // No deadline: this path includes `display alert`, which is meant to block
    // until the user dismisses it.
    match run_osascript_raw(script, None).await {
        // Empty stdout is a fact, not a verdict: a read that found nothing and
        // a write that succeeded must not both say "OK".
        Ok(output) => ToolResult::ok(if output.is_empty() {
            "(exit 0, no output)".to_string()
        } else {
            output
        }),
        Err(e) => ToolResult::error(e),
    }
}

/// Run an AppleScript and return its stdout.
///
/// `deadline` bounds scripts that walk the accessibility tree — those cost one
/// Apple Event round-trip per property and can stall indefinitely on a busy or
/// unresponsive app. `None` means wait forever, which is only correct for
/// scripts that are deliberately blocking on the user.
#[cfg(target_os = "macos")]
async fn run_osascript_raw(script: &str, deadline: Option<Duration>) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new("osascript");
    cmd.arg("-e").arg(script);
    // Without this a timed-out osascript keeps running, holding its Apple Event
    // session open and slowing every later capture.
    cmd.kill_on_drop(true);

    let output = match deadline {
        Some(d) => tokio::time::timeout(d, cmd.output())
            .await
            .map_err(|_| format!("AppleScript timed out after {}s", d.as_secs()))?,
        None => cmd.output().await,
    }
    .map_err(|e| format!("Failed to run osascript: {}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(describe_applescript_error(&stderr))
    }
}

/// Turn osascript's stderr into a statement of what happened. The numeric
/// codes are what System Events emits; the text after each is what the
/// caller can act on.
#[cfg(target_os = "macos")]
fn describe_applescript_error(stderr: &str) -> String {
    if stderr.contains("-25211") {
        return format!(
            "AppleScript error {}: Nebo is not allowed to control this app. Open System Settings > Privacy & Security > Accessibility, enable Nebo, then retry.",
            stderr
        );
    }
    if stderr.contains("-1728") {
        return format!(
            "AppleScript error {}: the named window/element does not exist; use capture see to list what exists.",
            stderr
        );
    }
    if stderr.contains("-600") || stderr.contains("-609") {
        return format!("AppleScript error {}: the app is not running.", stderr);
    }
    if stderr.is_empty() {
        return "AppleScript error: osascript exited non-zero and printed nothing".to_string();
    }
    format!("AppleScript error: {}", stderr)
}

#[cfg(target_os = "macos")]
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "macos")]
fn key_name_to_code(key: &str) -> Result<&'static str, String> {
    Ok(match key.to_lowercase().as_str() {
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
        "f6" => "97",
        "f7" => "98",
        "f8" => "100",
        "f9" => "101",
        "f10" => "109",
        "f11" => "103",
        "f12" => "111",
        "forwarddelete" | "del" => "117",
        "home" => "115",
        "end" => "119",
        "pageup" | "page_up" => "116",
        "pagedown" | "page_down" => "121",
        // US ANSI letters, digits and punctuation, so combos like cmd+s go
        // through key codes instead of System Events keystrokes.
        "a" => "0", "s" => "1", "d" => "2", "f" => "3", "h" => "4", "g" => "5", "z" => "6",
        "x" => "7", "c" => "8", "v" => "9", "b" => "11", "q" => "12", "w" => "13", "e" => "14",
        "r" => "15", "y" => "16", "t" => "17", "1" => "18", "2" => "19", "3" => "20", "4" => "21",
        "6" => "22", "5" => "23", "=" => "24", "9" => "25", "7" => "26", "-" => "27", "8" => "28",
        "0" => "29", "]" => "30", "o" => "31", "u" => "32", "[" => "33", "i" => "34", "p" => "35",
        "l" => "37", "j" => "38", "'" => "39", "k" => "40", ";" => "41", "\\" => "42", "," => "43",
        "/" => "44", "n" => "45", "m" => "46", "." => "47", "`" => "50",
        // Falling back to Return here used to press Enter for any typo, which
        // submits forms the caller never meant to submit.
        _ => {
            return Err(format!(
                "Key '{}' is not in the macOS key map (return, tab, space, delete, escape, arrows, home/end/pageup/pagedown, f1-f12, letters, digits, punctuation). Use type for text.",
                key
            ))
        }
    })
}

// --- Cross-platform helpers ---

/// Build the subprocess for an X11 helper. On a headless server with the
/// on-demand desktop session live, inject its DISPLAY so xdotool/wmctrl/etc.
/// target the session; on a real desktop the inherited environment wins.
/// Never sets DISPLAY process-wide.
fn x11_command(cmd: &str) -> tokio::process::Command {
    let c = tokio::process::Command::new(cmd);
    #[cfg(target_os = "linux")]
    {
        let mut c = c;
        if let Some(display) = crate::desktop_session::display() {
            crate::desktop_session::touch();
            c.env("DISPLAY", display);
        }
        return c;
    }
    #[allow(unreachable_code)]
    c
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
async fn run_command(cmd: &str, args: &[&str]) -> ToolResult {
    match x11_command(cmd).args(args).output().await {
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
                "'{}' exited {}{}",
                cmd,
                code,
                if detail.is_empty() { " and printed nothing".to_string() } else { format!(": {detail}") }
            ))
        }
        Err(e) => ToolResult::error(format!("Command '{}' failed: {}", cmd, e)),
    }
}

#[allow(dead_code)]
async fn run_command_raw(cmd: &str, args: &[&str]) -> Result<String, String> {
    let output = x11_command(cmd)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("Command '{}' failed: {}", cmd, e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());
        Err(format!(
            "'{}' exited {}{}",
            cmd,
            code,
            if stderr.is_empty() {
                " and printed nothing".to_string()
            } else {
                format!(": {}", stderr)
            }
        ))
    }
}

#[cfg(target_os = "windows")]
async fn run_powershell(script: &str) -> ToolResult {
    #[cfg(target_os = "windows")]
    {
        let daemon = ps_daemon();
        match daemon.execute(script, Duration::from_secs(30)).await {
            Ok(out) => {
                return ToolResult::ok(if out.is_empty() {
                    "(exit 0, no output)".to_string()
                } else {
                    out
                });
            }
            Err(e) => {
                tracing::debug!(error = %e, "persistent PowerShell failed, falling back to subprocess");
            }
        }
    }
    // Fallback: a fresh powershell subprocess. Anything the caller had set in
    // the persistent session is gone, and the result says so.
    let mut result = run_command("powershell", &["-NoProfile", "-Command", script]).await;
    result.content = format!(
        "(PowerShell session restarted; prior $variables are gone) {}",
        result.content
    );
    result
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

#[cfg(target_os = "linux")]
async fn pipe_to_command(cmd: &str, args: &[&str], text: &str) -> ToolResult {
    let mut child = match tokio::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("Failed to run {}: {}", cmd, e)),
    };
    if let Some(stdin) = child.stdin.as_mut() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(text.as_bytes()).await;
    }
    match child.wait().await {
        Ok(status) if status.success() => ToolResult::ok("(exit 0, no output)"),
        Ok(status) => ToolResult::error(format!("{} exited with {}", cmd, status)),
        Err(e) => ToolResult::error(format!("Failed to wait for {}: {}", cmd, e)),
    }
}

#[cfg(target_os = "windows")]
fn escape_powershell(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(target_os = "windows")]
fn key_name_to_sendkeys(key: &str) -> &str {
    match key.to_lowercase().as_str() {
        "return" | "enter" => "{ENTER}",
        "tab" => "{TAB}",
        "space" => " ",
        "delete" | "backspace" => "{BACKSPACE}",
        "escape" | "esc" => "{ESC}",
        "left" => "{LEFT}",
        "right" => "{RIGHT}",
        "down" => "{DOWN}",
        "up" => "{UP}",
        "f1" => "{F1}",
        "f2" => "{F2}",
        "f3" => "{F3}",
        "f4" => "{F4}",
        "f5" => "{F5}",
        "home" => "{HOME}",
        "end" => "{END}",
        "pageup" => "{PGUP}",
        "pagedown" => "{PGDN}",
        "insert" => "{INSERT}",
        "del" => "{DELETE}",
        _ => key,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    #[test]
    fn a_capture_mid_resize_is_not_the_frames_shape() {
        let f = super::Rect { x: 703, y: 824, width: 230, height: 408 };
        assert!(!super::image_matches_frame(Some(&f), Some((468, 501))), "Calculator mid-launch on Stadium");
        assert!(super::image_matches_frame(Some(&f), Some((230, 408))));
        assert!(super::image_matches_frame(Some(&f), Some((460, 816))), "retina");
        assert!(super::image_matches_frame(None, Some((10, 10))), "the whole screen has no frame to disagree with");
        assert!(super::is_no_window("Calculator has no open window"));
        assert!(!super::is_no_window("ax helper produced no output"));
    }

    #[test]
    fn parse_frame_reads_system_events_output() {
        assert_eq!(super::parse_frame("868, 60, 447, 950\n"), Some((868, 60, 447, 950)));
        assert_eq!(super::parse_frame("1319,209,447,950"), Some((1319, 209, 447, 950)));
        assert_eq!(super::parse_frame("menu bar 1 of application process Simulator"), None);
        assert_eq!(super::parse_frame("0, 0, 0, 950"), None, "a zero-size window is not a frame");
    }

    use super::*;

    // `see` reported an empty app on every no-argument call because it echoed
    // the input instead of the app it inspected. The header the AX script now
    // emits is what carries that back.
    #[cfg(target_os = "macos")]
    #[test]
    fn ax_app_header_is_split_from_elements() {
        let output = "APP||Brave Browser\nWINDOWS||2\nAXButton||close button||0,0,10,10\n";
        let (header, body) = split_ax_app_header(output);
        assert_eq!(header.app, "Brave Browser");
        assert_eq!(header.windows, Some(2));
        assert_eq!(body, "AXButton||close button||0,0,10,10\n");

        let elements = parse_ax_output(body);
        assert_eq!(elements.len(), 1, "header must not be parsed as an element");
        assert_eq!(elements[0].label, "close button");
    }

    // An app running with zero windows yields no elements for a reason the
    // caller can act on — it must not look like a failed walk.
    #[cfg(target_os = "macos")]
    #[test]
    fn ax_header_reports_zero_windows() {
        let (header, body) = split_ax_app_header("APP||Brave Browser\nWINDOWS||0\n");
        assert_eq!(header.app, "Brave Browser");
        assert_eq!(header.windows, Some(0));
        assert!(parse_ax_output(body).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ax_output_without_header_still_parses() {
        let (header, body) = split_ax_app_header("AXButton||ok||1,2,3,4\n");
        assert!(header.app.is_empty());
        assert_eq!(header.windows, None);
        assert_eq!(parse_ax_output(body).len(), 1);
    }

    #[test]
    fn test_tool_metadata() {
        let tool = DesktopTool::new();
        assert_eq!(tool.name(), "desktop");
        assert!(tool.description().contains("window"));
        assert!(tool.description().contains("clipboard"));
        assert!(tool.description().contains("ui"));
        assert!(tool.description().contains("menu"));
        assert!(tool.description().contains("tts"));
        assert!(tool.description().contains("dock"));
        assert!(tool.requires_approval());
        let schema = tool.schema();
        assert!(schema["properties"]["resource"].is_object());
        assert!(schema["properties"]["action"].is_object());
        // Verify all resources in schema enum
        let resources = schema["properties"]["resource"]["enum"].as_array().unwrap();
        let resource_names: Vec<&str> = resources.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(resource_names.contains(&"window"));
        assert!(resource_names.contains(&"input"));
        assert!(resource_names.contains(&"clipboard"));
        assert!(resource_names.contains(&"notification"));
        assert!(resource_names.contains(&"capture"));
        assert!(resource_names.contains(&"ui"));
        assert!(resource_names.contains(&"menu"));
        assert!(resource_names.contains(&"dialog"));
        assert!(resource_names.contains(&"space"));
        assert!(resource_names.contains(&"shortcut"));
        assert!(resource_names.contains(&"tts"));
        assert!(resource_names.contains(&"dock"));
    }

    #[test]
    fn test_schema_params() {
        let tool = DesktopTool::new();
        let schema = tool.schema();
        // Unified input params (web-parity shape)
        assert!(schema["properties"]["coordinate"].is_object());
        assert!(schema["properties"]["start_coordinate"].is_object());
        assert!(schema["properties"]["click_count"].is_object());
        assert!(schema["properties"]["button"].is_object());
        assert!(schema["properties"]["direction"].is_object());
        assert!(schema["properties"]["amount"].is_object());
        assert!(schema["properties"]["ref"].is_object());
        assert!(schema["properties"]["name"].is_object());
        assert!(schema["properties"]["value"].is_object());
        assert!(schema["properties"]["voice"].is_object());
        assert!(schema["properties"]["rate"].is_object());
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
        assert_eq!(key_name_to_code("return"), Ok("36"));
        assert_eq!(key_name_to_code("tab"), Ok("48"));
        assert_eq!(key_name_to_code("escape"), Ok("53"));
        assert_eq!(key_name_to_code("Enter"), Ok("36")); // case insensitive
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_key_name_to_code_rejects_unmapped_key() {
        // An unmapped key must not fall back to Return.
        let err = key_name_to_code("f13").unwrap_err();
        assert!(err.contains("Key 'f13' is not in the macOS key map"), "{err}");
        assert!(err.contains("f1-f12"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_describe_applescript_error_maps_codes() {
        let e = describe_applescript_error("execution error: Not authorized (-25211)");
        assert!(e.contains("Accessibility"), "{e}");
        let e = describe_applescript_error("Can't get window 1 (-1728)");
        assert!(e.contains("does not exist"), "{e}");
        let e = describe_applescript_error("Application isn't running (-600)");
        assert!(e.contains("not running"), "{e}");
        let e = describe_applescript_error("");
        assert!(e.contains("printed nothing"), "{e}");
    }

    #[test]
    fn test_clipboard_updated_message_counts_chars() {
        assert_eq!(clipboard_updated_message("héllo"), "Clipboard updated (5 chars).");
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

    #[tokio::test]
    async fn test_unknown_actions_for_new_resources() {
        // Ensure new resources return proper error for unknown actions
        for resource in &["ui", "menu", "dialog", "space", "shortcut", "tts", "dock"] {
            let tool = DesktopTool::new();
            let ctx = ToolContext::default();
            let input = serde_json::json!({"resource": resource, "action": "nonexistent"});
            let result = tool.execute_dyn(&ctx, input).await;
            assert!(
                result.is_error,
                "Expected error for {}/nonexistent",
                resource
            );
        }
    }

    #[tokio::test]
    async fn test_input_missing_params() {
        let store = tokio::sync::Mutex::new(SnapshotStore::new());
        let cache = AxCache::default();
        // type without text
        let result = handle_input("type", &serde_json::json!({}), &store, &cache).await;
        assert!(result.is_error);
        assert!(result.content.contains("text"));

        // press without key
        let result = handle_input("press", &serde_json::json!({}), &store, &cache).await;
        assert!(result.is_error);
        assert!(result.content.contains("key"));

        // click with neither ref nor coordinate → explicit error (no silent click at 0,0)
        let result = handle_input("click", &serde_json::json!({}), &store, &cache).await;
        assert!(result.is_error);
        assert!(result.content.contains("coordinate"));
    }

    /// A pixel means a pixel of an image the model was given. With no capture
    /// of the app there is no such image, so the click is refused before
    /// anything moves, and the message names the one call that fixes it.
    #[tokio::test]
    async fn pixel_click_without_a_capture_is_refused_with_the_capture_hint() {
        let store = tokio::sync::Mutex::new(SnapshotStore::new());
        let cache = AxCache::default();
        let r = handle_input(
            "click",
            &serde_json::json!({"app": "Simulator", "coordinate": [223, 900]}),
            &store,
            &cache,
        )
        .await;
        assert!(r.is_error);
        assert!(r.content.contains("no capture of Simulator"), "{}", r.content);
        assert!(r.content.contains("action: \"see\", app: \"Simulator\""), "{}", r.content);
    }

    /// A ref is looked up in the capture of the app it names, not in whatever
    /// capture happened to be last.
    #[tokio::test]
    async fn refs_resolve_against_the_named_apps_capture() {
        let store = tokio::sync::Mutex::new(SnapshotStore::new());
        let cache = AxCache::default();
        let mk = |id: &str, app: &str| Snapshot {
            id: id.into(),
            app: Some(app.into()),
            created_at: Instant::now(),
            elements: vec![UIElement { more: 0,
                id: "B1".into(),
                role: "AXButton".into(),
                label: format!("{app} button"),
                bounds: desktop_snapshot::Rect { x: 0, y: 0, width: 10, height: 10 },
                actionable: true,
                keyboard_shortcut: None,
                actions: vec![],
                path: String::new(),
                focused: false,
                value: None,
            }],
            frame: Some(desktop_snapshot::Rect { x: 0, y: 0, width: 100, height: 100 }),
            scale: 1.0,
            via: "ax".into(),
        };
        store.lock().await.insert(mk("s1", "Finder"));
        store.lock().await.insert(mk("s2", "Notes"));
        let latest = snapshot_for(&store, "", "Finder").await.unwrap();
        assert_eq!(latest.id, "s1");
        assert_eq!(snapshot_for(&store, "", "").await.unwrap().id, "s2");
        assert_eq!(snapshot_for(&store, "s1", "Notes").await.unwrap().id, "s1", "snapshot_id wins");
        let r = handle_input("click", &serde_json::json!({"app": "Mail", "ref": "B1"}), &store, &cache).await;
        assert!(r.is_error);
        assert!(r.content.contains("not in the last capture of Mail"), "{}", r.content);
    }

    /// Every spelling the docs taught for an input target lands on the
    /// handler's two names: element_id and element are the ref, x and y are
    /// the coordinate, and the primary names win when both are present.
    #[test]
    fn input_target_alias_table() {
        let cases: &[(serde_json::Value, (&str, Option<(i64, i64)>))] = &[
            (serde_json::json!({"ref": "B3"}), ("B3", None)),
            (serde_json::json!({"element_id": "B3"}), ("B3", None)),
            (serde_json::json!({"element": "T1"}), ("T1", None)),
            (serde_json::json!({"ref": "B1", "element_id": "B2"}), ("B1", None)),
            (serde_json::json!({"ref": "", "element_id": "B2"}), ("B2", None)),
            (serde_json::json!({"coordinate": [10, 20]}), ("", Some((10, 20)))),
            (serde_json::json!({"x": 100, "y": 200}), ("", Some((100, 200)))),
            (serde_json::json!({"coordinate": [1, 2], "x": 9, "y": 9}), ("", Some((1, 2)))),
            (serde_json::json!({"x": 100}), ("", None)),
            (serde_json::json!({"element_id": "B3", "x": 5, "y": 6}), ("B3", Some((5, 6)))),
            (serde_json::json!({}), ("", None)),
        ];
        for (input, want) in cases {
            assert_eq!(input_target(input), *want, "{input}");
        }
    }

    /// An `element_id` reaches the element lookup: the error names the
    /// element it could not find, not a missing `ref`.
    #[tokio::test]
    async fn element_id_is_looked_up_as_a_ref() {
        let store = tokio::sync::Mutex::new(SnapshotStore::new());
        let cache = AxCache::default();
        let result = handle_input("click", &serde_json::json!({"element_id": "Z9"}), &store, &cache).await;
        assert!(result.is_error);
        assert!(result.content.contains("Element 'Z9' is not in the last capture"), "{}", result.content);
        let result = handle_input("type", &serde_json::json!({"element": "Z9", "text": "hi"}), &store, &cache).await;
        assert!(result.content.contains("Element 'Z9' is not in the last capture"), "{}", result.content);
    }

    /// Live, read-only: one observe of Finder. Nothing is clicked or typed.
    /// Run with `cargo test -p nebo-tools --lib live_observe -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore]
    async fn live_observe_finder_returns_frame_scale_and_layer() {
        let store = tokio::sync::Mutex::new(SnapshotStore::new());
        let cache = AxCache::default();
        // NEBO_LIVE_APP picks another running app (Simulator, zed, …) to read.
        let app = std::env::var("NEBO_LIVE_APP").unwrap_or_else(|_| "Finder".into());
        let started = Instant::now();
        let o = observe(&app, &serde_json::json!({}), &store, &cache).await.map_err(|e| e.content).unwrap();
        println!("{} ms\n{}", started.elapsed().as_millis(), o.result.content);
        assert!(o.result.image_url.is_some(), "an observe carries the image");
        assert!(o.snapshot.frame.is_some(), "an observe always knows what the image covers");
        assert!(o.snapshot.scale > 0.0);
        // The app may have no window open, in which case the screen is what was captured — and the header says so.
        assert!(
            o.result.content.starts_with(&format!("{app} — window at ")) || o.result.content.contains("has no open window"),
            "{}", o.result.content
        );
        assert!(o.result.content.contains("via ax"), "{}", o.result.content);
    }

    /// Live: drive Calculator through observe → act. Buttons carry AXPress,
    /// so this presses through accessibility and moves no pointer. Launches
    /// Calculator and quits it afterwards.
    /// Run with `cargo test -p nebo-tools --lib live_act -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore]
    async fn live_act_drives_calculator_and_reads_the_result() {
        let store = tokio::sync::Mutex::new(SnapshotStore::new());
        let cache = AxCache::default();
        let _ = run_osascript_raw("tell application \"Calculator\" to activate", Some(AX_CAPTURE_TIMEOUT)).await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let _ = handle_input("press", &serde_json::json!({"app": "Calculator", "key": "escape", "wait_ms": 200}), &store, &cache).await;
        let o = observe("Calculator", &serde_json::json!({}), &store, &cache).await.map_err(|e| e.content).unwrap();
        println!("{}", o.result.content);
        let find = |label: &str| {
            o.snapshot.elements.iter().find(|e| e.label.eq_ignore_ascii_case(label)).map(|e| e.id.clone())
                .unwrap_or_else(|| panic!("no element labelled {label:?}"))
        };
        let mut last = String::new();
        for key in ["7", "+", "8", "="] {
            let alias = match key { "+" => "add", "=" => "equals", k => k };
            let id = o.snapshot.elements.iter()
                .find(|e| e.label.eq_ignore_ascii_case(key) || e.label.eq_ignore_ascii_case(alias))
                .map(|e| e.id.clone())
                .unwrap_or_else(|| find(key));
            let r = handle_input("click", &serde_json::json!({"app": "Calculator", "ref": id, "snapshot_id": o.snapshot.id, "wait_ms": 300}), &store, &cache).await;
            assert!(!r.is_error, "{}", r.content);
            assert!(r.image_url.is_some(), "an act returns the after-image");
            assert!(r.content.contains("via accessibility"), "{}", r.content.lines().next().unwrap_or(""));
            last = r.content;
        }
        println!("{}", last.lines().take(3).collect::<Vec<_>>().join("\n"));
        assert!(last.contains("15"), "the after-state shows the result: {last}");

        // The vision path: click "9" by a pixel of the LAST image (no ref),
        // which exercises image px → window pt → screen pt and a real click.
        let after = snapshot_for(&store, "", "Calculator").await.unwrap();
        let nine = after.elements.iter().find(|e| e.label == "9").expect("a 9 button");
        let frame = after.frame.as_ref().unwrap();
        let (cx, cy) = nine.bounds.center();
        let px = screen_to_image((cx, cy), frame, after.scale);
        let r = handle_input("click", &serde_json::json!({"app": "Calculator", "coordinate": [px.0, px.1], "wait_ms": 300}), &store, &cache).await;
        assert!(!r.is_error, "{}", r.content);
        let first = r.content.lines().next().unwrap_or("").to_string();
        println!("{first}");
        assert!(first.starts_with("Clicked (") && first.contains("at screen ("), "{first}");
        let shown = r.content.lines().find(|l| l.contains("AXStaticText") && !l.contains("\"\"")).unwrap_or("");
        assert!(shown.contains("\"9\"") || r.content.contains("\"9\"  at"), "after a pixel click on 9 the display shows 9: {}", r.content);
        let _ = run_osascript_raw("tell application \"Calculator\" to quit", Some(AX_CAPTURE_TIMEOUT)).await;
    }

    /// Every combo goes through key codes (no System Events), and the ones
    /// that end the session are refused unless forced.
    #[test]
    fn combos_split_into_a_key_code_and_modifiers_and_session_enders_are_blocked() {
        assert_eq!(super::split_combo("command+shift+s").unwrap(), ("s".to_string(), "cmd,shift".to_string()));
        assert_eq!(super::split_combo("ctrl+alt+Delete").unwrap(), ("Delete".to_string(), "ctrl,opt".to_string()));
        assert!(super::split_combo("hyper+s").is_err());
        #[cfg(target_os = "macos")]
        {
            assert_eq!(super::key_name_to_code("s").unwrap(), "1");
            assert_eq!(super::key_name_to_code("v").unwrap(), "9");
            assert_eq!(super::key_name_to_code("pagedown").unwrap(), "121");
            assert!(super::key_name_to_code("nonsense").is_err());
        }
        for k in ["cmd+shift+q", "shift+cmd+q", "cmd+option+escape", "ctrl+cmd+q"] {
            assert!(super::blocked_combo(k), "{k}");
        }
        for k in ["cmd+q", "cmd+s", "cmd+shift+s", "escape"] {
            assert!(!super::blocked_combo(k), "{k}");
        }
    }

    /// The same act leaving the same screen is counted; a different screen
    /// (the act did something new) is not.
    #[test]
    fn an_act_that_keeps_leaving_the_same_screen_is_counted_as_circling() {
        let key = "click|{\"app\":\"TextEdit\",\"coordinate\":[130,8],\"circling-test\":1}".to_string();
        for _ in 0..3 {
            super::remember_act(key.clone(), 42);
        }
        assert_eq!(super::circling(&key, Some(42)), 3);
        assert_eq!(super::circling(&key, Some(7)), 0, "a different screen is progress");
        assert_eq!(super::circling("TextEdit|click||Some((1, 1))||other", None), 0);
    }

    /// wait_for picks exactly one helper predicate; an empty or unknown one
    /// falls back to the fixed pause.
    #[test]
    fn wait_for_becomes_one_helper_wait() {
        let a = |v: serde_json::Value| super::wait_for_args("Mail", &serde_json::json!({ "wait_for": v }));
        assert_eq!(a(serde_json::json!({"text": "Sent"})).unwrap()[3..6], ["--for", "text", "--text"].map(String::from));
        assert!(a(serde_json::json!({"gone": "Loading"})).unwrap().contains(&"gone".to_string()));
        assert!(a(serde_json::json!({"menu": false})).unwrap().contains(&"menu-closed".to_string()));
        assert!(a(serde_json::json!({"window": "Untitled"})).unwrap().contains(&"--title".to_string()));
        assert!(a(serde_json::json!({"text": null, "window": null})).is_none());
        assert!(super::wait_for_args("", &serde_json::json!({"wait_for": {"text": "x"}})).is_none());
        let t = serde_json::json!({"wait_for": {"text": "x", "timeout_ms": 999_999}});
        assert_eq!(super::wait_for_timeout(&t), 30_000);
    }

    /// A field is named by its placeholder, not its contents; its contents are
    /// its value; a secure field has neither read.
    #[test]
    fn a_fields_name_is_not_its_contents_and_a_secure_field_has_none() {
        let node = |role: &str, title: &str, value: Option<&str>, placeholder: Option<&str>| ax_native::AxNode {
            path: "0".into(), role: role.into(), title: title.into(), value: value.map(String::from),
            desc: None, placeholder: placeholder.map(String::from), frame: [0, 0, 10, 10], actions: vec![], enabled: true, focused: false,
            ..Default::default()
        };
        let typed = element_from_node(&node("AXTextField", "", Some("alma@x.com"), Some("you@company.com")));
        assert_eq!((typed.label.as_str(), typed.value.as_deref()), ("you@company.com", Some("alma@x.com")));
        let empty = element_from_node(&node("AXTextField", "", Some(""), Some("you@company.com")));
        assert_eq!(empty.value, None);
        let secure = element_from_node(&node("AXSecureTextField", "", Some("hunter2"), None));
        assert_eq!((secure.label.as_str(), secure.value), ("[secure field]", None));
        let text = element_from_node(&node("AXStaticText", "", Some("Welcome back"), None));
        assert_eq!((text.label.as_str(), text.value), ("Welcome back", None));
        let titled = element_from_node(&node("AXButton", "Continue", None, None));
        assert_eq!(titled.label, "Continue");
    }

    /// A text line inside a labelled element is that element; one outside
    /// becomes an OCRText element in screen points (image px × scale + origin).
    #[test]
    fn text_lines_join_the_elements_unless_the_tree_already_names_them() {
        let frame = desktop_snapshot::Rect { x: 100, y: 50, width: 2000, height: 1000 };
        let mut elements = vec![UIElement { more: 0,
            id: String::new(),
            role: "AXButton".into(),
            label: "Save".into(),
            bounds: desktop_snapshot::Rect { x: 120, y: 90, width: 100, height: 40 },
            actionable: true,
            keyboard_shortcut: None,
            actions: vec!["AXPress".into()],
            path: "0".into(),
            focused: false,
            value: None,
        }];
        let lines = vec![
            ax_native::TextLine { text: "Save".into(), frame: [12, 22, 40, 12], confidence: 0.9 }, // centre (32,28) px → (164,106) pt: inside Save
            ax_native::TextLine { text: "Total: $41".into(), frame: [300, 400, 120, 14], confidence: 0.8 },
        ];
        let (added, total) = merge_text_lines(&mut elements, &lines, Some(&frame), 2.0);
        assert_eq!((added, total), (1, 2));
        let t = &elements[1];
        assert_eq!((t.role.as_str(), t.label.as_str()), ("OCRText", "Total: $41"));
        assert_eq!((t.bounds.x, t.bounds.y, t.bounds.width, t.bounds.height), (700, 850, 240, 28));
        assert!(t.actionable && t.actions.is_empty() && t.path.is_empty());
    }

    #[test]
    fn capture_is_read_as_screenshot() {
        assert_eq!(canonical_capture_action("capture"), "screenshot");
        assert_eq!(canonical_capture_action("see"), "see");
        assert_eq!(canonical_capture_action("screenshot"), "screenshot");
    }

    #[tokio::test]
    async fn test_tts_missing_text() {
        let result = handle_tts("speak", &serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("text"));
    }

    #[tokio::test]
    async fn test_dock_missing_app() {
        let result = handle_dock("is_running", &serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("app"));
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
        // Reading window geometry via System Events requires the Accessibility
        // permission, which test runners often lack (CI, sandboxed shells).
        // Missing permission (-25211) is a skip, not a failure — any other
        // error is a real regression.
        if result.is_error && result.content.contains("-25211") {
            eprintln!("skipping test_window_list: osascript lacks assistive access");
            return;
        }
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_window_focus_missing_app() {
        let result = handle_window("focus", &serde_json::json!({})).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_window_move_missing_app() {
        let result = handle_window("move", &serde_json::json!({"x": 100, "y": 100})).await;
        assert!(result.is_error);
        assert!(result.content.contains("app"));
    }

    #[tokio::test]
    async fn test_clipboard_clear() {
        // Just test the action routing, not the actual clipboard
        let result = handle_clipboard("invalid", &serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("clear"));
    }

    #[tokio::test]
    async fn test_notification_alert_missing_message() {
        let result = handle_notification("alert", &serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("message"));
    }
}
