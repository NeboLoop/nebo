#[cfg(target_os = "windows")]
use crate::desktop_daemon::DesktopDaemon;
use crate::desktop_snapshot::{
    self, assign_element_ids, generate_snapshot_id, parse_ax_output, Snapshot, SnapshotStore,
    UIElement,
};
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

pub struct DesktopTool {
    /// Serializes mouse + keyboard operations (one physical input device).
    input_lock: tokio::sync::Mutex<()>,
    /// Serializes clipboard read/write (single system clipboard).
    clipboard_lock: tokio::sync::Mutex<()>,
    snapshot_store: tokio::sync::Mutex<SnapshotStore>,
    ax_cache: std::sync::Mutex<HashMap<String, (Vec<UIElement>, Instant)>>,
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
         - input: click, double_click, right_click, type, press, hotkey, move, scroll, drag, paste\n\
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
         Workflow: Use capture(action: see) to get a snapshot with element IDs, then reference them in input actions.\n\n\
         Examples:\n  \
         desktop(resource: \"capture\", action: \"see\", app: \"Safari\") — snapshot + element IDs\n  \
         desktop(resource: \"input\", action: \"click\", element_id: \"B3\") — click element from snapshot\n  \
         desktop(resource: \"input\", action: \"type\", element_id: \"T1\", text: \"hello\") — focus + type\n  \
         desktop(resource: \"clipboard\", action: \"read\")\n  \
         desktop(resource: \"window\", action: \"list\")\n  \
         desktop(resource: \"capture\", action: \"screenshot\", app: \"Safari\")\n  \
         desktop(resource: \"notification\", action: \"send\", title: \"Done\", message: \"Task complete\")\n  \
         desktop(resource: \"tts\", action: \"speak\", text: \"Hello world\")"
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
                "key": { "type": "string", "description": "Key to press (e.g. 'return', 'tab')" },
                "keys": { "type": "string", "description": "Key combination for hotkey (e.g. 'command+shift+s')" },
                "x": { "type": "integer", "description": "X coordinate" },
                "y": { "type": "integer", "description": "Y coordinate" },
                "x2": { "type": "integer", "description": "End X coordinate (for drag)" },
                "y2": { "type": "integer", "description": "End Y coordinate (for drag)" },
                "dx": { "type": "integer", "description": "Scroll delta X" },
                "dy": { "type": "integer", "description": "Scroll delta Y" },
                "width": { "type": "integer", "description": "Width for resize/move" },
                "height": { "type": "integer", "description": "Height for resize/move" },
                "region": { "type": "string", "description": "Region for screenshot: 'x,y,w,h'" },
                "quality": { "type": "string", "description": "Screenshot quality: 'low' (800px, 50% JPEG), 'medium' (1280px, 65% JPEG, default), 'high' (full-res PNG)" },
                "name": { "type": "string", "description": "Name for shortcut/menu/dialog element" },
                "value": { "type": "string", "description": "Value for set_value/fill" },
                "role": { "type": "string", "description": "UI element role filter (e.g. 'AXButton')" },
                "label": { "type": "string", "description": "UI element label/identifier" },
                "index": { "type": "integer", "description": "Index for space/menu item" },
                "voice": { "type": "string", "description": "TTS voice name" },
                "rate": { "type": "integer", "description": "TTS speaking rate (words per minute)" },
                "element_id": { "type": "string", "description": "Element ID from a snapshot (e.g. B1, T2). Use capture(action: see) first" },
                "snapshot_id": { "type": "string", "description": "Snapshot ID from a previous see action" },
                "max_elements": { "type": "integer", "description": "Max elements returned by see (default: 100)" }
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
                    let result = handle_input(action, &input, &self.snapshot_store).await;
                    if !result.is_error {
                        // Invalidate AX cache on successful input actions
                        if let Ok(mut guard) = self.ax_cache.lock() {
                            guard.clear();
                        }
                    }
                    result
                }
                "clipboard" => {
                    let _guard = self.clipboard_lock.lock().await;
                    handle_clipboard(action, &input).await
                }
                "notification" => handle_notification(action, &input).await,
                "capture" => handle_capture(action, &input, &self.snapshot_store, &self.ax_cache).await,
                "ui" => {
                    let _guard = self.input_lock.lock().await;
                    handle_ui(action, &input).await
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
                return ToolResult::error("'app' parameter required for focus");
            }
            handle_window_focus(app).await
        }
        "minimize" => {
            let app = input["app"].as_str().unwrap_or("");
            if app.is_empty() {
                return ToolResult::error("'app' parameter required for minimize");
            }
            handle_window_minimize(app).await
        }
        "maximize" => {
            let app = input["app"].as_str().unwrap_or("");
            if app.is_empty() {
                return ToolResult::error("'app' parameter required for maximize");
            }
            handle_window_maximize(app).await
        }
        "resize" => {
            let app = input["app"].as_str().unwrap_or("");
            let w = input["width"].as_i64().unwrap_or(800);
            let h = input["height"].as_i64().unwrap_or(600);
            if app.is_empty() {
                return ToolResult::error("'app' parameter required for resize");
            }
            handle_window_resize(app, w, h).await
        }
        "close" => {
            let app = input["app"].as_str().unwrap_or("");
            if app.is_empty() {
                return ToolResult::error("'app' parameter required for close");
            }
            handle_window_close(app).await
        }
        "move" => {
            let app = input["app"].as_str().unwrap_or("");
            let x = input["x"].as_i64().unwrap_or(0);
            let y = input["y"].as_i64().unwrap_or(0);
            if app.is_empty() {
                return ToolResult::error("'app' parameter required for move");
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
        return run_osascript(script).await;
    }
    #[cfg(target_os = "linux")]
    {
        if which("wmctrl") {
            return run_command("wmctrl", &["-l", "-G"]).await;
        }
        if which("xdotool") {
            return run_command("xdotool", &["search", "--onlyvisible", "--name", ""]).await;
        }
        return ToolResult::error("Window list requires wmctrl or xdotool (install with your package manager)");
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
            escape_powershell(app), escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    { let _ = app; ToolResult::error("Window focus is not supported on this platform") }
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
            escape_powershell(app), escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    { let _ = app; ToolResult::error("Window minimize is not supported on this platform") }
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
            return run_command("wmctrl", &["-r", app, "-b", "add,maximized_vert,maximized_horz"]).await;
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
            escape_powershell(app), escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    { let _ = app; ToolResult::error("Window maximize is not supported on this platform") }
}

async fn handle_window_resize(app: &str, w: i64, h: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"System Events\" to set size of first window of process \"{}\" to {{{}, {}}}",
            escape_applescript(app), w, h
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
            escape_powershell(app), w, h, escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    { let _ = (app, w, h); ToolResult::error("Window resize is not supported on this platform") }
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
            escape_powershell(app), escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    { let _ = app; ToolResult::error("Window close is not supported on this platform") }
}

async fn handle_window_move(app: &str, x: i64, y: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"System Events\" to set position of first window of process \"{}\" to {{{}, {}}}",
            escape_applescript(app), x, y
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
            escape_powershell(app), x, y, escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    { let _ = (app, x, y); ToolResult::error("Window move is not supported on this platform") }
}

// --- Input simulation ---

async fn handle_input(
    action: &str,
    input: &serde_json::Value,
    snapshot_store: &tokio::sync::Mutex<SnapshotStore>,
) -> ToolResult {
    // Resolve element_id → coordinates if provided
    let element_id = input["element_id"].as_str().unwrap_or("");
    let snapshot_id = input["snapshot_id"].as_str().unwrap_or("");

    if !element_id.is_empty() {
        let store = snapshot_store.lock().await;
        let element = if !snapshot_id.is_empty() {
            store.get_element(snapshot_id, element_id)
        } else {
            store.latest().and_then(|snap| snap.elements.iter().find(|e| e.id == element_id))
        };

        match element {
            Some(elem) => {
                let (cx, cy) = elem.bounds.center();
                let label = elem.label.clone();
                drop(store);

                // For type action with element_id: click to focus first, then type
                if action == "type" {
                    let text = input["text"].as_str().unwrap_or("");
                    if text.is_empty() {
                        return ToolResult::error("'text' parameter required for type");
                    }
                    let click_result = input_click(cx, cy).await;
                    if click_result.is_error {
                        return click_result;
                    }
                    // Small delay for focus
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    let type_result = input_type(text).await;
                    return ToolResult {
                        content: format!("Clicked '{}' at ({},{}) and typed text", label, cx, cy),
                        is_error: type_result.is_error,
                        image_url: None,
                    };
                }

                // For click-family actions: use resolved coordinates
                return match action {
                    "click" => {
                        let r = input_click(cx, cy).await;
                        ToolResult {
                            content: format!("Clicked '{}' ({}) at ({},{})", label, element_id, cx, cy),
                            is_error: r.is_error,
                            image_url: None,
                        }
                    }
                    "double_click" => {
                        let r = input_double_click(cx, cy).await;
                        ToolResult {
                            content: format!("Double-clicked '{}' ({}) at ({},{})", label, element_id, cx, cy),
                            is_error: r.is_error,
                            image_url: None,
                        }
                    }
                    "right_click" => {
                        let r = input_right_click(cx, cy).await;
                        ToolResult {
                            content: format!("Right-clicked '{}' ({}) at ({},{})", label, element_id, cx, cy),
                            is_error: r.is_error,
                            image_url: None,
                        }
                    }
                    "move" => {
                        let r = input_move(cx, cy).await;
                        ToolResult {
                            content: format!("Moved cursor to '{}' ({}) at ({},{})", label, element_id, cx, cy),
                            is_error: r.is_error,
                            image_url: None,
                        }
                    }
                    _ => ToolResult::error(format!(
                        "element_id not supported for action '{}'. Use: click, double_click, right_click, type, move",
                        action
                    )),
                };
            }
            None => {
                drop(store);
                return ToolResult::error(format!(
                    "Element '{}' not found{}. Use capture(action: \"see\") first to detect elements.",
                    element_id,
                    if !snapshot_id.is_empty() { format!(" in snapshot '{}'", snapshot_id) } else { String::new() }
                ));
            }
        }
    }

    // Standard coordinate-based input (no element_id)
    match action {
        "type" => {
            let text = input["text"].as_str().unwrap_or("");
            if text.is_empty() {
                return ToolResult::error("'text' parameter required for type");
            }
            input_type(text).await
        }
        "press" => {
            let key = input["key"].as_str().unwrap_or("");
            if key.is_empty() {
                return ToolResult::error("'key' parameter required for press");
            }
            input_press(key).await
        }
        "click" => {
            let x = input["x"].as_i64().unwrap_or(0);
            let y = input["y"].as_i64().unwrap_or(0);
            input_click(x, y).await
        }
        "double_click" => {
            let x = input["x"].as_i64().unwrap_or(0);
            let y = input["y"].as_i64().unwrap_or(0);
            input_double_click(x, y).await
        }
        "right_click" => {
            let x = input["x"].as_i64().unwrap_or(0);
            let y = input["y"].as_i64().unwrap_or(0);
            input_right_click(x, y).await
        }
        "hotkey" => {
            let keys = input["keys"].as_str().unwrap_or("");
            if keys.is_empty() {
                return ToolResult::error("'keys' parameter required for hotkey (e.g. 'command+shift+s')");
            }
            input_hotkey(keys).await
        }
        "move" => {
            let x = input["x"].as_i64().unwrap_or(0);
            let y = input["y"].as_i64().unwrap_or(0);
            input_move(x, y).await
        }
        "scroll" => {
            let dx = input["dx"].as_i64().unwrap_or(0);
            let dy = input["dy"].as_i64().unwrap_or(0);
            input_scroll(dx, dy).await
        }
        "drag" => {
            let x = input["x"].as_i64().unwrap_or(0);
            let y = input["y"].as_i64().unwrap_or(0);
            let x2 = input["x2"].as_i64().unwrap_or(0);
            let y2 = input["y2"].as_i64().unwrap_or(0);
            input_drag(x, y, x2, y2).await
        }
        "paste" => input_paste().await,
        _ => ToolResult::error(format!(
            "Unknown input action '{}'. Use: click, double_click, right_click, type, press, hotkey, move, scroll, drag, paste",
            action
        )),
    }
}

async fn input_type(text: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
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
    { let _ = text; ToolResult::error("Input type is not supported on this platform") }
}

async fn input_press(key: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let key_code = key_name_to_code(key);
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
    { let _ = key; ToolResult::error("Input press is not supported on this platform") }
}

async fn input_click(x: i64, y: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
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
    { let _ = (x, y); ToolResult::error("Input click is not supported on this platform") }
}

async fn input_double_click(x: i64, y: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
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
    { let _ = (x, y); ToolResult::error("Double click is not supported on this platform") }
}

async fn input_right_click(x: i64, y: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
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
    { let _ = (x, y); ToolResult::error("Right click is not supported on this platform") }
}

async fn input_hotkey(keys: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
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
            escape_applescript(key), modifier_str
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
    { let _ = keys; ToolResult::error("Hotkey is not supported on this platform") }
}

async fn input_move(x: i64, y: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
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
    { let _ = (x, y); ToolResult::error("Mouse move is not supported on this platform") }
}

#[allow(unused_variables)]
async fn input_scroll(dx: i64, dy: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        // cliclick supports scroll: kd (scroll down) / ku (scroll up)
        if dy == 0 {
            return ToolResult::ok("No scroll delta specified");
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
                return ToolResult::ok("No scroll delta specified");
            }
            return results.into_iter().last().unwrap_or_else(|| ToolResult::ok("OK"));
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
    { let _ = (dx, dy); ToolResult::error("Scroll is not supported on this platform") }
}

async fn input_drag(x: i64, y: i64, x2: i64, y2: i64) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
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
    { let _ = (x, y, x2, y2); ToolResult::error("Drag is not supported on this platform") }
}

async fn input_paste() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
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
                return ToolResult::error("'text' parameter required for clipboard write");
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
                    "(clipboard is empty)".to_string()
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
        return ToolResult::ok("Clipboard updated");
    }
    #[cfg(target_os = "linux")]
    {
        if which("wl-copy") {
            return pipe_to_command("wl-copy", &[], text).await;
        }
        if which("xclip") {
            return pipe_to_command("xclip", &["-selection", "clipboard"], text).await;
        }
        if which("xsel") {
            return pipe_to_command("xsel", &["-ib"], text).await;
        }
        return ToolResult::error("Clipboard write requires wl-copy, xclip, or xsel");
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!("Set-Clipboard -Value '{}'", escape_powershell(text));
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    { let _ = text; ToolResult::error("Clipboard is not supported on this platform") }
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
                return ToolResult::error("'message' parameter required for notification");
            }
            notification_send(title, message).await
        }
        "alert" => {
            let title = input["title"].as_str().unwrap_or("Nebo");
            let message = input["message"].as_str().unwrap_or("");
            if message.is_empty() {
                return ToolResult::error("'message' parameter required for alert");
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
            escape_powershell(title), escape_powershell(message),
            escape_powershell(title), escape_powershell(message)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    { let _ = (title, message); ToolResult::error("Notifications are not supported on this platform") }
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
            escape_powershell(message), escape_powershell(title)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    { let _ = (title, message); ToolResult::error("Alert is not supported on this platform") }
}

// --- Screen capture ---

async fn handle_capture(
    action: &str,
    input: &serde_json::Value,
    snapshot_store: &tokio::sync::Mutex<SnapshotStore>,
    ax_cache: &std::sync::Mutex<HashMap<String, (Vec<UIElement>, Instant)>>,
) -> ToolResult {
    match action {
        "screenshot" => capture_screenshot(input).await,
        "see" => capture_see(input, snapshot_store, ax_cache).await,
        _ => ToolResult::error(format!(
            "Unknown capture action '{}'. Use: screenshot, see",
            action
        )),
    }
}

/// Capture screenshot + AX tree elements, store as a snapshot.
/// Returns the snapshot_id and element list as JSON, plus the screenshot image.
async fn capture_see(
    input: &serde_json::Value,
    snapshot_store: &tokio::sync::Mutex<SnapshotStore>,
    ax_cache: &std::sync::Mutex<HashMap<String, (Vec<UIElement>, Instant)>>,
) -> ToolResult {
    // Step 1: Take screenshot
    let screenshot = capture_screenshot(input).await;
    if screenshot.is_error {
        return screenshot;
    }

    // Step 2: Capture AX elements with positions (cached, 2s TTL)
    let app = input["app"].as_str().unwrap_or("");
    let max_elements = input["max_elements"].as_u64().unwrap_or(100).min(500) as usize;
    let cache_key = app.to_string();

    // Check cache (snapshot-then-release — lock held <1μs)
    let cached = ax_cache
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .get(&cache_key)
                .filter(|(_, ts)| ts.elapsed() < Duration::from_secs(2))
                .map(|(elems, _)| elems.clone())
        });

    let mut elements = if let Some(elems) = cached {
        elems
    } else {
        // Cache miss — subprocess runs with NO lock held
        let elems = capture_ax_elements(app).await;
        if let Ok(mut guard) = ax_cache.lock() {
            guard.insert(cache_key, (elems.clone(), Instant::now()));
        }
        elems
    };

    // Limit and assign IDs
    elements.truncate(max_elements);
    assign_element_ids(&mut elements);

    // Step 3: Build and store snapshot
    let snapshot_id = generate_snapshot_id();
    let snapshot = Snapshot {
        id: snapshot_id.clone(),
        app: if app.is_empty() { None } else { Some(app.to_string()) },
        created_at: std::time::Instant::now(),
        elements: elements.clone(),
    };

    {
        let mut store = snapshot_store.lock().await;
        store.insert(snapshot);
    }

    // Step 4: Build response
    let elements_json: Vec<serde_json::Value> = elements
        .iter()
        .filter(|e| e.actionable || !e.label.is_empty())
        .take(50) // Keep response concise for the LLM
        .map(|e| {
            serde_json::json!({
                "id": e.id,
                "role": e.role,
                "label": e.label,
                "bounds": { "x": e.bounds.x, "y": e.bounds.y, "w": e.bounds.width, "h": e.bounds.height },
                "actionable": e.actionable,
            })
        })
        .collect();

    let response = serde_json::json!({
        "snapshot_id": snapshot_id,
        "app": app,
        "element_count": elements.len(),
        "elements": elements_json,
    });

    ToolResult {
        content: serde_json::to_string_pretty(&response).unwrap_or_default(),
        is_error: false,
        image_url: screenshot.image_url,
    }
}

/// Capture AX elements with position information from the accessibility tree.
#[allow(unused_variables)]
async fn capture_ax_elements(app: &str) -> Vec<desktop_snapshot::UIElement> {
    #[cfg(target_os = "macos")]
    {
        let target = if app.is_empty() {
            "first application process whose frontmost is true".to_string()
        } else {
            format!("process \"{}\"", escape_applescript(app))
        };
        // Recursive AppleScript that returns role||label||x,y,w,h per element
        let script = format!(
            r#"tell application "System Events"
    tell {}
        set output to ""
        try
            repeat with elem in (every UI element of window 1)
                try
                    set eRole to role of elem
                    set eName to name of elem
                    if eName is missing value then set eName to description of elem
                    if eName is missing value then set eName to ""
                    set ePos to position of elem
                    set eSize to size of elem
                    set output to output & eRole & "||" & eName & "||" & (item 1 of ePos as text) & "," & (item 2 of ePos as text) & "," & (item 1 of eSize as text) & "," & (item 2 of eSize as text) & linefeed
                end try
            end repeat
        end try
        return output
    end tell
end tell"#,
            target
        );
        match run_osascript_raw(&script).await {
            Ok(output) => {
                let mut elements = parse_ax_output(&output);
                assign_element_ids(&mut elements);
                elements
            }
            Err(_) => Vec::new(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Linux/Windows: return empty for now — element detection requires platform-specific work
        Vec::new()
    }
}

async fn capture_screenshot(input: &serde_json::Value) -> ToolResult {
    let quality = input["quality"].as_str().unwrap_or("medium");
    let app = input["app"].as_str().unwrap_or("");
    let region = input["region"].as_str();

    // Use JPEG capture on macOS for low/medium to skip PNG decode overhead
    #[cfg(target_os = "macos")]
    let use_jpeg = quality != "high";
    #[cfg(not(target_os = "macos"))]
    let use_jpeg = false;

    let ext = if use_jpeg { "jpg" } else { "png" };
    let tmp_path = format!("/tmp/nebo-capture-{}.{}", std::process::id(), ext);

    #[cfg(target_os = "macos")]
    let result = {
        let mut base_args: Vec<String> = vec!["-x".to_string()];
        if use_jpeg {
            base_args.extend_from_slice(&["-t".to_string(), "jpg".to_string()]);
        }

        if !app.is_empty() {
            let wid_script = format!(
                "tell application \"System Events\" to return id of first window of process \"{}\"",
                escape_applescript(app)
            );
            match run_osascript_raw(&wid_script).await {
                Ok(wid) => {
                    let wid = wid.trim().to_string();
                    let mut args = vec!["-l".to_string(), wid];
                    args.extend(base_args);
                    args.push(tmp_path.clone());
                    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                    tokio::process::Command::new("screencapture")
                        .args(&arg_refs)
                        .output()
                        .await
                }
                Err(_) => {
                    base_args.push(tmp_path.clone());
                    let arg_refs: Vec<&str> = base_args.iter().map(|s| s.as_str()).collect();
                    tokio::process::Command::new("screencapture")
                        .args(&arg_refs)
                        .output()
                        .await
                }
            }
        } else if let Some(region) = region {
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
                return ToolResult::error("Region format: 'x,y,w,h'");
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
                    let _ = run_command("xdotool", &["search", "--name", app, "windowactivate"]).await;
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
                tokio::process::Command::new("gnome-screenshot")
                    .args(["-w", "-f", &tmp_path])
                    .output()
                    .await
            } else {
                tokio::process::Command::new("gnome-screenshot")
                    .args(["-f", &tmp_path])
                    .output()
                    .await
            }
        } else if which("scrot") {
            if !app.is_empty() {
                tokio::process::Command::new("scrot")
                    .args(["-u", &tmp_path])
                    .output()
                    .await
            } else {
                tokio::process::Command::new("scrot")
                    .args([&tmp_path])
                    .output()
                    .await
            }
        } else if which("grim") {
            // Wayland
            if let Some(region) = region {
                // Use slurp format
                tokio::process::Command::new("grim")
                    .args(["-g", region, &tmp_path])
                    .output()
                    .await
            } else {
                tokio::process::Command::new("grim")
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
    return ToolResult::error("Screenshot is not supported on this platform");

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    match result {
        Ok(output) if output.status.success() => {
            match tokio::fs::read(&tmp_path).await {
                Ok(bytes) => {
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                    compress_and_encode(&bytes, quality)
                }
                Err(e) => ToolResult::error(format!("Failed to read screenshot: {}", e)),
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            ToolResult::error(format!("Screenshot failed: {}", stderr))
        }
        Err(e) => ToolResult::error(format!("Failed to run screenshot tool: {}", e)),
    }
}

/// Compress a screenshot to JPEG at the given quality level, resize, and base64-encode.
/// Accepts both PNG and JPEG input (auto-detected via magic bytes).
///
/// Quality levels:
/// - "low":    800px max width, 50% JPEG
/// - "medium": 1280px max width, 65% JPEG (default)
/// - "high":   original format, no compression
fn compress_and_encode(img_bytes: &[u8], quality: &str) -> ToolResult {
    use base64::Engine;
    use image::ImageReader;
    use std::io::Cursor;

    let is_jpeg = img_bytes.len() >= 2 && img_bytes[0] == 0xFF && img_bytes[1] == 0xD8;

    if quality == "high" {
        let mime = if is_jpeg { "image/jpeg" } else { "image/png" };
        let b64 = base64::engine::general_purpose::STANDARD.encode(img_bytes);
        return ToolResult {
            content: format!("Screenshot captured (high quality, {} bytes)", img_bytes.len()),
            is_error: false,
            image_url: Some(format!("data:{};base64,{}", mime, b64)),
        };
    }

    let (max_width, jpeg_quality) = match quality {
        "low" => (800u32, 50u8),
        _ => (1280u32, 65u8), // "medium" or any other value
    };

    let img = match ImageReader::new(Cursor::new(img_bytes))
        .with_guessed_format()
        .and_then(|r| r.decode().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)))
    {
        Ok(img) => img,
        Err(e) => {
            // Fall back to raw bytes if decode fails
            let mime = if is_jpeg { "image/jpeg" } else { "image/png" };
            tracing::warn!(error = %e, "failed to decode screenshot for compression, returning raw");
            let b64 = base64::engine::general_purpose::STANDARD.encode(img_bytes);
            return ToolResult {
                content: format!("Screenshot captured ({} bytes, uncompressed)", img_bytes.len()),
                is_error: false,
                image_url: Some(format!("data:{};base64,{}", mime, b64)),
            };
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
            let original_kb = img_bytes.len() / 1024;
            let compressed_kb = jpeg_bytes.len() / 1024;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);
            ToolResult {
                content: format!(
                    "Screenshot captured ({}KB → {}KB, {}x{} {}% JPEG)",
                    original_kb, compressed_kb, img.width(), img.height(), jpeg_quality
                ),
                is_error: false,
                image_url: Some(format!("data:image/jpeg;base64,{}", b64)),
            }
        }
        Err(e) => {
            let mime = if is_jpeg { "image/jpeg" } else { "image/png" };
            tracing::warn!(error = %e, "JPEG encode failed, returning raw image");
            let b64 = base64::engine::general_purpose::STANDARD.encode(img_bytes);
            ToolResult {
                content: format!("Screenshot captured ({} bytes, uncompressed)", img_bytes.len()),
                is_error: false,
                image_url: Some(format!("data:{};base64,{}", mime, b64)),
            }
        }
    }
}

// --- UI / Accessibility ---

#[allow(unused_variables)]
async fn handle_ui(action: &str, input: &serde_json::Value) -> ToolResult {
    let app = input["app"].as_str().unwrap_or("");
    let role = input["role"].as_str().unwrap_or("");
    let label = input["label"].as_str().unwrap_or("");
    let value = input["value"].as_str().unwrap_or("");

    match action {
        "tree" => {
            if app.is_empty() {
                return ToolResult::error("'app' parameter required for ui tree");
            }
            ui_tree(app, role).await
        }
        "find" => {
            if label.is_empty() && role.is_empty() {
                return ToolResult::error("'label' or 'role' parameter required for ui find");
            }
            ui_find(app, role, label).await
        }
        "click" => {
            if label.is_empty() {
                return ToolResult::error("'label' parameter required for ui click");
            }
            ui_click(app, label).await
        }
        "get_value" => {
            if label.is_empty() {
                return ToolResult::error("'label' parameter required for ui get_value");
            }
            ui_get_value(app, label).await
        }
        "set_value" => {
            if label.is_empty() || value.is_empty() {
                return ToolResult::error("'label' and 'value' parameters required for ui set_value");
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
            escape_applescript(app), role_filter
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
            escape_powershell(app), escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("UI accessibility is not supported on this platform")
}

#[allow(unused_variables)]
async fn ui_find(app: &str, role: &str, label: &str) -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        let target = if app.is_empty() {
            "first application process whose frontmost is true".to_string()
        } else {
            format!("process \"{}\"", escape_applescript(app))
        };
        let search = if !label.is_empty() {
            format!("whose name contains \"{}\"", escape_applescript(label))
        } else if !role.is_empty() {
            format!("whose role is \"{}\"", escape_applescript(role))
        } else {
            String::new()
        };
        let script = format!(
            r#"tell application "System Events"
    tell {}
        set results to ""
        repeat with elem in (every UI element of window 1 {})
            set results to results & (role of elem) & " | " & (name of elem) & " | " & (position of elem) & linefeed
        end repeat
        return results
    end tell
end tell"#,
            target, search
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error("UI find on Linux requires AT-SPI2 (not yet fully implemented)");
    }
    #[cfg(target_os = "windows")]
    {
        let cond = if !label.is_empty() {
            format!("New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, '{}')", escape_powershell(label))
        } else {
            "[System.Windows.Automation.Condition]::TrueCondition".to_string()
        };
        let script = format!(
            r#"Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = {}
$results = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $cond)
foreach ($e in $results) {{
    "$($e.Current.ControlType.ProgrammaticName) | $($e.Current.Name) | $($e.Current.BoundingRectangle)"
}}"#,
            cond
        );
        return run_powershell(&script).await;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    ToolResult::error("UI find is not supported on this platform")
}

#[allow(unused_variables)]
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
            target, escape_applescript(label)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error("UI click on Linux requires AT-SPI2 (not yet fully implemented)");
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
            escape_powershell(label), escape_powershell(label), escape_powershell(label)
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
            target, escape_applescript(label)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error("UI get_value on Linux requires AT-SPI2 (not yet fully implemented)");
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
            escape_powershell(label), escape_powershell(label)
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
            target, escape_applescript(label), escape_applescript(value)
        );
        return run_osascript(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error("UI set_value on Linux requires AT-SPI2 (not yet fully implemented)");
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
            escape_powershell(label), escape_powershell(value), escape_powershell(label)
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
                return ToolResult::error("'app' parameter required for menu list");
            }
            menu_list(app).await
        }
        "menus" => {
            if app.is_empty() {
                return ToolResult::error("'app' parameter required for menus");
            }
            menu_menus(app).await
        }
        "click" => {
            if app.is_empty() || name.is_empty() {
                return ToolResult::error("'app' and 'name' parameters required for menu click");
            }
            menu_click(app, name).await
        }
        "status" => menu_status_list().await,
        "click_status" => {
            if name.is_empty() {
                return ToolResult::error("'name' parameter required for click_status");
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
        let script = format!(
            r#"Add-Type @"
using System; using System.Runtime.InteropServices;
public class MenuHelper {{
    [DllImport("user32.dll")] public static extern IntPtr GetMenu(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr hMenu);
    [DllImport("user32.dll")] public static extern bool GetMenuItemInfo(IntPtr hMenu, uint uItem, bool fByPosition, ref MENUITEMINFO lpmii);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Auto)]
    public struct MENUITEMINFO {{
        public uint cbSize; public uint fMask; public uint fType; public uint fState;
        public uint wID; public IntPtr hSubMenu; public IntPtr hbmpChecked; public IntPtr hbmpUnchecked;
        public IntPtr dwItemData; public string dwTypeData; public uint cch; public IntPtr hbmpItem;
    }}
}}
"@
$p = Get-Process | Where-Object {{ $_.MainWindowTitle -match '{}' }} | Select-Object -First 1
if ($p) {{ "Menu bar found for $($p.ProcessName)" }} else {{ "App '{}' not found" }}"#,
            escape_powershell(app), escape_powershell(app)
        );
        return run_powershell(&script).await;
    }
    #[cfg(target_os = "linux")]
    {
        return ToolResult::error("Menu bar access is not supported on Linux (most apps use client-side decorations)");
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
        return ToolResult::error("Menu enumeration on Windows requires UI Automation (use ui tree instead)");
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
        return ToolResult::error("Menu click on Windows requires UI Automation (use ui click instead)");
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
                return ToolResult::error("'name' parameter required for dialog click");
            }
            dialog_click(app, name).await
        }
        "fill" => {
            if value.is_empty() {
                return ToolResult::error("'value' parameter required for dialog fill");
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
            return "Dialog detected: " & dialogCount & " dialog(s), " & sheetCount & " sheet(s)"
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
            target, escape_applescript(name), escape_applescript(name)
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
            escape_powershell(name), escape_powershell(name), escape_powershell(name)
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
            target, field_target, escape_applescript(value),
            field_target, escape_applescript(value)
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
                return ToolResult::error("'index' parameter required for space switch");
            }
            space_switch(index).await
        }
        "move_window" => {
            if index == 0 {
                return ToolResult::error("'index' parameter required for space move_window");
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
        // Mission Control spaces — no clean API, use defaults read
        let script = r#"do shell script "defaults read com.apple.spaces spans-displays 2>/dev/null; echo '---'; defaults read com.apple.dock wvous-tl-corner 2>/dev/null || echo 'Use Mission Control to see spaces'"
"#;
        return run_osascript(script).await;
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
        // Windows virtual desktops — limited PowerShell access
        let script = r#"try {
    $vd = [Windows.UI.ViewManagement.UIViewSettings, Windows.UI.ViewManagement, ContentType=WindowsRuntime]
    "Virtual desktops are available (use Win+Tab to view)"
} catch {
    "Virtual desktop API requires Windows 10+"
}"#;
        return run_powershell(script).await;
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
            1 => "18", 2 => "19", 3 => "20", 4 => "21",
            5 => "23", 6 => "22", 7 => "26", 8 => "28", 9 => "25",
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
        return run_powershell(&script).await;
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
            "Moving windows between spaces on macOS requires third-party tools (e.g., yabai)"
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
        return ToolResult::error("Moving windows between virtual desktops requires Windows API (not available via PowerShell)");
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
                return ToolResult::error("'name' parameter required for shortcut run");
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
                result.push_str(&format!("{}:\n", dir));
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
        return run_powershell("Get-Command -CommandType Application,Script | Select-Object -First 50 Name, Source | Format-Table -AutoSize").await;
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
        // Fallback: try running as a command
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
                return ToolResult::error("'text' parameter required for tts speak");
            }
            let voice = input["voice"].as_str().unwrap_or("");
            let rate = input["rate"].as_i64().unwrap_or(0);
            tts_speak(text, voice, rate).await
        }
        _ => ToolResult::error(format!(
            "Unknown tts action '{}'. Use: speak",
            action
        )),
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
        return ToolResult::error("TTS requires espeak or spd-say (install espeak or speech-dispatcher)");
    }
    #[cfg(target_os = "windows")]
    {
        let rate_str = if rate > 0 {
            format!("$synth.Rate = {}\n", (rate as f64 / 30.0).clamp(-10.0, 10.0) as i64)
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
            voice_str, rate_str, escape_powershell(text)
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
                return ToolResult::error("'app' parameter required for dock is_running");
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
        return run_osascript(script).await;
    }
    #[cfg(not(target_os = "macos"))]
    ToolResult::error("Dock badges are only available on macOS")
}

#[allow(unused_variables)]
async fn dock_recent() -> ToolResult {
    #[cfg(target_os = "macos")]
    {
        return run_command("defaults", &["read", "com.apple.dock", "recent-apps"]).await;
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
            escape_applescript(app), escape_applescript(app), escape_applescript(app)
        );
        return run_osascript(&script).await;
    }
    #[cfg(not(target_os = "macos"))]
    { let _ = app; ToolResult::error("Dock is_running is only available on macOS") }
}

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

// --- macOS helpers ---

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

// --- Cross-platform helpers ---

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

#[allow(dead_code)]
async fn run_command_raw(cmd: &str, args: &[&str]) -> Result<String, String> {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("Command '{}' failed: {}", cmd, e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("{}", stderr))
    }
}

#[cfg(target_os = "windows")]
async fn run_powershell(script: &str) -> ToolResult {
    #[cfg(target_os = "windows")]
    {
        let daemon = ps_daemon();
        match daemon.execute(script, Duration::from_secs(30)).await {
            Ok(out) => return ToolResult::ok(if out.is_empty() { "OK".to_string() } else { out }),
            Err(e) => {
                tracing::debug!(error = %e, "persistent PowerShell failed, falling back to subprocess");
            }
        }
    }
    // Fallback (non-Windows or daemon failure)
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

#[cfg(any(target_os = "linux", target_os = "windows"))]
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
        Ok(status) if status.success() => ToolResult::ok("OK"),
        Ok(status) => ToolResult::error(format!("{} exited with status {}", cmd, status)),
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
    use super::*;

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
        // New params should exist
        assert!(schema["properties"]["keys"].is_object());
        assert!(schema["properties"]["x2"].is_object());
        assert!(schema["properties"]["y2"].is_object());
        assert!(schema["properties"]["dx"].is_object());
        assert!(schema["properties"]["dy"].is_object());
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

    #[tokio::test]
    async fn test_unknown_actions_for_new_resources() {
        // Ensure new resources return proper error for unknown actions
        for resource in &["ui", "menu", "dialog", "space", "shortcut", "tts", "dock"] {
            let tool = DesktopTool::new();
            let ctx = ToolContext::default();
            let input = serde_json::json!({"resource": resource, "action": "nonexistent"});
            let result = tool.execute_dyn(&ctx, input).await;
            assert!(result.is_error, "Expected error for {}/nonexistent", resource);
        }
    }

    #[tokio::test]
    async fn test_input_missing_params() {
        let store = tokio::sync::Mutex::new(SnapshotStore::new());
        // type without text
        let result = handle_input("type", &serde_json::json!({}), &store).await;
        assert!(result.is_error);
        assert!(result.content.contains("text"));

        // press without key
        let result = handle_input("press", &serde_json::json!({}), &store).await;
        assert!(result.is_error);
        assert!(result.content.contains("key"));

        // hotkey without keys
        let result = handle_input("hotkey", &serde_json::json!({}), &store).await;
        assert!(result.is_error);
        assert!(result.content.contains("keys"));
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
        // Window list should succeed on macOS (may be empty in CI)
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
