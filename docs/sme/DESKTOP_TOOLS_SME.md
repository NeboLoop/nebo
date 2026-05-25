# Desktop Tools SME

Subject Matter Expert reference for Nebo's desktop automation subsystem.
Covers the `DesktopTool`, `DesktopDaemon`, `DesktopSnapshot`, their integration
with the `OsTool`, the tool registry, and the Tauri desktop shell.

Last updated: 2026-05-15

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [DesktopTool — The Core](#desktoptool--the-core)
3. [Resource/Action Matrix](#resourceaction-matrix)
4. [Window Management](#window-management)
5. [Input Simulation](#input-simulation)
6. [Clipboard](#clipboard)
7. [Screen Capture and Snapshots](#screen-capture-and-snapshots)
8. [DesktopSnapshot — Element Tracking](#desktopsnapshot--element-tracking)
9. [UI / Accessibility Integration](#ui--accessibility-integration)
10. [Menu Access](#menu-access)
11. [Dialog Interaction](#dialog-interaction)
12. [Virtual Desktops (Spaces)](#virtual-desktops-spaces)
13. [Shortcuts](#shortcuts)
14. [Text-to-Speech (TTS)](#text-to-speech-tts)
15. [Dock (macOS)](#dock-macos)
16. [Notifications](#notifications)
17. [DesktopDaemon — Persistent PowerShell (Windows)](#desktopdaemon--persistent-powershell-windows)
18. [OsTool Integration](#ostool-integration)
19. [Tool Registry and Activation](#tool-registry-and-activation)
20. [Resource Permits and Concurrency](#resource-permits-and-concurrency)
21. [Tauri Integration](#tauri-integration)
22. [Key Structs and Signatures](#key-structs-and-signatures)
23. [Security Considerations](#security-considerations)
24. [Platform-Specific Behavior](#platform-specific-behavior)
25. [Error Handling](#error-handling)
26. [Image Compression Pipeline](#image-compression-pipeline)
27. [Tests](#tests)

---

## Architecture Overview

```
+-------------------------------------------------------------------+
|                         Agent Runner                              |
|  (crates/agent/src/runner.rs)                                     |
+-------------------------------------------------------------------+
        |                                        |
        v                                        v
+------------------+                 +----------------------+
|   Tool Registry  |                 |   Tool Filter        |
|   (registry.rs)  |                 |   (tool_filter.rs)   |
+------------------+                 +----------------------+
        |                                        |
        |   keyword match: "click", "screenshot" |
        |   "window", "menu", "tts", etc.        |
        |<---------------------------------------+
        |
        v
+------------------+
|     OsTool       |  <-- Unified namespace for all local operations
|   (os_tool.rs)   |      25 resources: file, shell, desktop, etc.
+------------------+
        |
        | resource = "window"|"input"|"clipboard"|"capture"|
        |   "notification"|"ui"|"menu"|"dialog"|"space"|
        |   "shortcut"|"tts"|"dock"
        v
+---------------------+
|    DesktopTool      |  <-- 12 desktop-specific resources
|  (desktop_tool.rs)  |
+---------------------+
   |          |         |
   v          v         v
+-------+  +--------+  +----------+
| macOS |  | Linux  |  | Windows  |
+-------+  +--------+  +----------+
| osascript  xdotool   PowerShell |
| cliclick   wmctrl    user32.dll |
| pbpaste    wl-paste  UIAutomat. |
| screencap  scrot     .NET APIs  |
| say        espeak    SAPI       |
+-------+  +--------+  +----------+
                            |
                            v
                 +-------------------+
                 |  DesktopDaemon    | (Windows only)
                 | (desktop_daemon)  |  Persistent PowerShell
                 +-------------------+  process with sentinel
                                        protocol

+---------------------+
|  DesktopSnapshot    |  In-memory LRU snapshot store
| (desktop_snapshot)  |  AX tree element tracking
+---------------------+  Element ID assignment (B1, T2, etc.)
```

### Data Flow: see-then-click

The flagship workflow for desktop automation is "see then click" — the agent
takes a snapshot of the screen, receives element IDs, and then interacts with
specific elements by ID:

```
1. Agent calls: os(resource: "capture", action: "see", app: "Safari")
       |
       v
2. capture_screenshot() -----> screencapture/scrot/PowerShell
       |                        returns raw image bytes
       v
3. compress_and_encode() ----> JPEG resize + base64 data URL
       |
       v
4. capture_ax_elements() ----> AppleScript AX tree query
       |                        returns role||label||x,y,w,h lines
       v
5. parse_ax_output() --------> Vec<UIElement>
       |
       v
6. assign_element_ids() -----> B1, B2, T1, S1, L1, M1, ...
       |
       v
7. SnapshotStore.insert() ---> LRU store (max 25, 10min TTL)
       |
       v
8. Return: { snapshot_id, elements[], image_url }

9. Agent calls: os(resource: "input", action: "click", element_id: "B3")
       |
       v
10. SnapshotStore.get_element() --> UIElement { bounds: Rect }
       |
       v
11. Rect.center() --> (cx, cy) screen coordinates
       |
       v
12. input_click(cx, cy) --> cliclick/xdotool/user32.dll
```

---

## DesktopTool -- The Core

**File:** `crates/tools/src/desktop_tool.rs` (~3450 lines)

The `DesktopTool` struct is the central dispatcher for all desktop automation.
It implements `DynTool` and routes 12 resources to platform-specific handlers.

```rust
pub struct DesktopTool {
    input_lock: tokio::sync::Mutex<()>,       // Serializes mouse+keyboard
    clipboard_lock: tokio::sync::Mutex<()>,   // Serializes clipboard ops
    snapshot_store: tokio::sync::Mutex<SnapshotStore>,  // LRU snapshots
    ax_cache: std::sync::Mutex<HashMap<String, (Vec<UIElement>, Instant)>>,  // AX cache (2s TTL)
}
```

**Key design decisions:**

- `input_lock` serializes ALL mouse and keyboard operations. There is only one
  physical input device, so concurrent clicks would be nonsensical.
- `clipboard_lock` is separate from `input_lock` because clipboard reads don't
  require input serialization.
- `ax_cache` uses a synchronous `std::sync::Mutex` because the lock is held
  for less than 1 microsecond (snapshot-then-release pattern). The actual AX
  subprocess runs with NO lock held.
- `requires_approval()` returns `true` — all desktop operations require user
  confirmation before execution. The parent `OsTool` overrides this with
  per-resource granularity via `requires_approval_for()`.

---

## Resource/Action Matrix

| Resource       | Actions                                                    | Lock     | Approval |
|----------------|-----------------------------------------------------------|----------|----------|
| `window`       | list, focus, minimize, maximize, resize, close, move       | input    | yes      |
| `input`        | click, double_click, right_click, type, press, hotkey, move, scroll, drag, paste | input | yes |
| `clipboard`    | read, write, clear                                         | clipboard| no       |
| `notification` | send, alert                                                | none     | no       |
| `capture`      | screenshot, see                                            | none     | no       |
| `ui`           | tree, find, click, get_value, set_value, list_apps         | input    | yes      |
| `menu`         | list, menus, click, status, click_status                   | input    | yes      |
| `dialog`       | detect, list, click, fill, dismiss                         | input    | yes      |
| `space`        | list, switch, move_window                                  | none     | yes      |
| `shortcut`     | list, run                                                  | none     | yes      |
| `tts`          | speak                                                      | none     | no       |
| `dock`         | badges, recent, is_running                                 | none     | no       |

The "Approval" column reflects the `OsTool.AUTO_APPROVE_RESOURCES` list:
`["file", "shell", "clipboard", "capture", "search", "notification", "tts", "dock"]`
are auto-approved. Everything else requires user confirmation.

---

## Window Management

Handles listing, focusing, minimizing, maximizing, resizing, closing, and
moving application windows across all three platforms.

### Platform Implementations

| Action    | macOS                               | Linux                  | Windows                    |
|-----------|-------------------------------------|------------------------|----------------------------|
| list      | System Events (AppleScript)         | wmctrl -l -G / xdotool| Get-Process (MainWindowTitle)|
| focus     | `tell app "X" to activate`          | wmctrl -a / xdotool windowactivate | SetForegroundWindow (P/Invoke) |
| minimize  | `set miniaturized of window to true`| xdotool windowminimize | ShowWindow(hWnd, 6)        |
| maximize  | `set position + size of window`     | wmctrl -b add,maximized| ShowWindow(hWnd, 3)        |
| resize    | `set size of window to {w,h}`       | xdotool windowsize     | MoveWindow (P/Invoke)      |
| close     | Click button 1 of window            | xdotool key alt+F4    | CloseMainWindow()          |
| move      | `set position of window to {x,y}`   | xdotool windowmove    | MoveWindow with GetWindowRect|

**macOS maximize** uses a hardcoded 1920x1055 size with position {0,25}. This
is an approximation since macOS does not have a native "maximize" concept
(only full-screen via green button).

---

## Input Simulation

All input actions are serialized behind `input_lock`. When an `element_id` is
provided, the tool resolves it to screen coordinates via the snapshot store,
then dispatches to the appropriate platform primitive.

### Element ID Resolution Flow

```
element_id provided?
  |
  +-- No --> use raw x,y coordinates from input
  |
  +-- Yes --> SnapshotStore.get_element(snapshot_id, element_id)
                |
                +-- Found --> UIElement.bounds.center() --> (cx, cy)
                |               |
                |               +-- action=="type"? --> click(cx,cy) + sleep(100ms) + type(text)
                |               +-- action=="click"? --> click(cx,cy)
                |               +-- etc.
                |
                +-- Not Found --> error: "use capture(action: see) first"
```

### Platform Tools

| Action       | macOS              | Linux                         | Windows                    |
|--------------|--------------------|------------------------------ |----------------------------|
| click        | cliclick c:x,y     | xdotool mousemove + click 1   | SetCursorPos + mouse_event |
| double_click | cliclick dc:x,y    | xdotool click --repeat 2 1    | mouse_event x2 w/ 50ms gap|
| right_click  | cliclick rc:x,y    | xdotool mousemove + click 3   | mouse_event 0x0008/0x0010  |
| type         | System Events keystroke | xdotool type --clearmodifiers | WScript.Shell.SendKeys |
| press        | key code (mapped)  | xdotool key                   | SendKeys (mapped)          |
| hotkey       | keystroke + using modifiers | xdotool key combo      | SendKeys ^+%prefixes       |
| move         | cliclick m:x,y     | xdotool mousemove             | SetCursorPos               |
| scroll       | cliclick ku/kd     | xdotool click btn 4/5/6/7     | mouse_event 0x0800 * 120   |
| drag         | cliclick dd:x,y du:x2,y2 | xdotool mousedown+move+up | SetCursorPos + mouse_event |
| paste        | keystroke "v" using command down | xdotool key ctrl+v | SendKeys('^v')           |

### AX Cache Invalidation

After a successful input action, the AX cache is cleared:

```rust
if !result.is_error {
    if let Ok(mut guard) = self.ax_cache.lock() {
        guard.clear();
    }
}
```

This ensures the next `see` action re-queries the accessibility tree, since
user input may have changed the UI state.

### Key Mapping

**macOS key codes** (AppleScript `key code` values):
- return=36, tab=48, space=49, delete=51, escape=53
- left=123, right=124, down=125, up=126
- f1=122, f2=120, f3=99, f4=118, f5=96
- Unknown keys default to 36 (return)

**Windows SendKeys** mapping:
- return={ENTER}, tab={TAB}, space=" ", escape={ESC}
- Arrows: {LEFT}, {RIGHT}, {DOWN}, {UP}
- Function keys: {F1}-{F5}
- Special: {HOME}, {END}, {PGUP}, {PGDN}, {INSERT}, {DELETE}

**Hotkey modifiers:**
- macOS: command/cmd, shift, option/alt, control/ctrl -> AppleScript `using` clause
- Windows: ctrl->^, alt->%, shift->+ as SendKeys prefix
- Linux: xdotool uses "+" notation natively

---

## Clipboard

Separate `clipboard_lock` mutex. Three actions: read, write, clear.

| Action | macOS      | Linux                       | Windows          |
|--------|------------|-----------------------------|------------------|
| read   | pbpaste    | wl-paste > xclip -o > xsel  | Get-Clipboard    |
| write  | pbcopy     | wl-copy > xclip > xsel      | Set-Clipboard    |
| clear  | AppleScript| wl-copy "" > xclip "" > xsel| Set-Clipboard $null |

Linux clipboard tries Wayland tools first (`wl-paste`/`wl-copy`), then X11
(`xclip`, `xsel`).

---

## Screen Capture and Snapshots

Two capture actions: `screenshot` (raw image) and `see` (image + AX tree).

### Screenshot Pipeline

```
capture_screenshot(input)
  |
  +-- Extract: quality (low/medium/high), app, region
  |
  +-- Platform-specific capture to /tmp/nebo-capture-{pid}.{jpg|png}
  |     macOS: screencapture -x [-t jpg] [-l wid] [-R region]
  |     Linux: gnome-screenshot / scrot / grim
  |     Windows: System.Drawing.Graphics.CopyFromScreen
  |
  +-- Read bytes from tmp file, delete tmp file
  |
  +-- compress_and_encode(bytes, quality)
        |
        +-- high:   raw bytes -> base64 (no resize, no recompress)
        +-- medium: decode -> resize to 1280px max -> 65% JPEG -> base64
        +-- low:    decode -> resize to 800px max  -> 50% JPEG -> base64
```

**macOS-specific optimization:** For low/medium quality, `screencapture` is
told to output JPEG directly (`-t jpg`), avoiding a PNG->decode->JPEG round
trip.

### See (Snapshot) Pipeline

`capture_see()` combines screenshot + AX tree into a `Snapshot`:

1. Take screenshot (reuses `capture_screenshot`)
2. Query AX elements (with 2-second cache)
3. Truncate to `max_elements` (default 100, max 500)
4. Assign element IDs via `assign_element_ids()`
5. Store in `SnapshotStore` (LRU, max 25, 10-minute TTL)
6. Return JSON with `snapshot_id`, `elements[]`, and `image_url`

The response filters to at most 50 elements that are actionable or labeled,
keeping the LLM response concise.

---

## DesktopSnapshot -- Element Tracking

**File:** `crates/tools/src/desktop_snapshot.rs` (~427 lines)

### Key Types

```rust
pub struct Rect {
    pub x: i64, pub y: i64,
    pub width: i64, pub height: i64,
}
impl Rect {
    pub fn center(&self) -> (i64, i64)  // (x + w/2, y + h/2)
}

pub struct UIElement {
    pub id: String,                    // "B1", "T2", "S3"
    pub role: String,                  // "AXButton", "AXTextField"
    pub label: String,                 // Human-readable name
    pub bounds: Rect,                  // Screen coordinates
    pub actionable: bool,              // Can be clicked/typed into
    pub keyboard_shortcut: Option<String>,
}

pub struct Snapshot {
    pub id: String,                    // "snap_1711900000000_a3f2"
    pub app: Option<String>,
    pub created_at: Instant,
    pub elements: Vec<UIElement>,
}
```

### SnapshotStore

LRU cache with time-based expiry:

```
+---------------------------------------------------+
|  SnapshotStore                                    |
|  snapshots: VecDeque<Snapshot>                    |
|                                                   |
|  MAX_SNAPSHOTS = 25                               |
|  SNAPSHOT_TTL  = 600 seconds (10 minutes)         |
|                                                   |
|  insert() -> cleanup expired, LRU evict, push_back|
|  get(id)  -> reverse scan, skip expired           |
|  latest() -> most recent non-expired              |
|  get_element(snap_id, elem_id) -> Option<&UIElement>|
+---------------------------------------------------+
```

### Element ID Assignment

Role-based prefix mapping:

| Role contains                                  | Prefix | Example |
|------------------------------------------------|--------|---------|
| button, checkbox, radio, popup                 | B      | B1, B2  |
| textfield, textarea, searchfield, combobox     | T      | T1, T2  |
| link                                           | L      | L1      |
| statictext, heading, label                     | S      | S1      |
| image                                          | I      | I1      |
| group, list, table, outline                    | G      | G1      |
| menu                                           | M      | M1      |
| (anything else)                                | X      | X1      |

IDs are assigned sequentially per prefix: first button is B1, second is B2, etc.

### Parsing AX Output

macOS AppleScript returns lines in `role||label||x,y,w,h` format:

```
AXButton||Submit||100,200,80,30
AXTextField||Name||50,100,200,30
AXStaticText||Help text||10,50,400,20
```

`parse_ax_output()` splits on `||`, parses the coordinate tuple, and
determines `actionable` based on role prefix (B, T, L, M are actionable).

### Snapshot ID Generation

```rust
pub fn generate_snapshot_id() -> String {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).as_millis();
    let rand: u16 = (ts as u16) ^ (process::id() as u16);
    format!("snap_{}_{:04x}", ts, rand)
}
// Example: "snap_1716000000000_a3f2"
```

---

## UI / Accessibility Integration

The `ui` resource provides direct access to the accessibility tree without
taking a screenshot.

| Action    | macOS                                      | Linux                      | Windows                          |
|-----------|--------------------------------------------|----------------------------|----------------------------------|
| tree      | AppleScript: every UI element of window 1  | AT-SPI2 via python3+gdbus  | UIAutomation FindAll descendants |
| find      | AppleScript: filter by name/role           | Not implemented            | UIAutomation PropertyCondition   |
| click     | AppleScript: click UI element by name      | Not implemented            | InvokePattern.Invoke()           |
| get_value | AppleScript: value of UI element           | Not implemented            | ValuePattern.Current.Value       |
| set_value | AppleScript: set value of UI element       | Not implemented            | ValuePattern.SetValue()          |
| list_apps | System Events: every process (visible)     | wmctrl -l / ls /usr/share/applications | Get-Process (MainWindowTitle) |

**Linux accessibility note:** AT-SPI2 support is minimal. Only `tree` has a
basic implementation via `python3` calling `busctl` and `gdbus`. The `find`,
`click`, `get_value`, and `set_value` actions return explicit "not yet fully
implemented" errors on Linux.

---

## Menu Access

Application menu bar access. Most complete on macOS, limited on other platforms.

| Action       | macOS                                    | Linux | Windows                    |
|--------------|------------------------------------------|-------|----------------------------|
| list         | menu bar items of process                | N/A   | GetMenu P/Invoke (basic)   |
| menus        | All menu items under each bar item       | N/A   | Not supported              |
| click        | click menu item (supports "Menu > Item") | N/A   | Not supported (use ui click)|
| status       | menu bar 2 of SystemUIServer             | N/A   | Shell.Application tray     |
| click_status | click menu bar item in status bar        | N/A   | Not supported              |

**Menu click path syntax (macOS):** The `name` parameter supports `"File > Save"`
format. If `>` is present, it splits into menu bar item + menu item. Otherwise,
it searches all menu bar items for a matching menu item.

---

## Dialog Interaction

Detects, lists, and interacts with modal dialogs and sheets.

| Action  | macOS                                     | Linux | Windows                      |
|---------|-------------------------------------------|-------|------------------------------|
| detect  | Count sheets + AXDialog/AXSheet windows   | N/A   | ControlType Window + #32770  |
| list    | Every UI element of sheet 1 / window 1    | N/A   | Descendants of dialog window |
| click   | Click button by name in sheet / window    | N/A   | InvokePattern on button      |
| fill    | Set value of text field in sheet / window  | N/A   | ValuePattern on Edit control |
| dismiss | key code 53 (Escape)                      | xdotool key Escape | SendKeys {ESC}    |

**macOS sheet handling:** Dialog operations try the sheet first (`sheet 1 of window 1`),
then fall back to the window itself (`window 1`). This handles both sheet-style
dialogs (attached to window) and standalone dialog windows.

---

## Virtual Desktops (Spaces)

| Action      | macOS                                | Linux          | Windows                   |
|-------------|--------------------------------------|----------------|---------------------------|
| list        | defaults read com.apple.spaces       | wmctrl -d      | Limited (no clean API)    |
| switch      | key code + control down (Ctrl+1-9)   | wmctrl -s      | Win+Ctrl+Right (repeated) |
| move_window | Not supported (needs yabai)          | wmctrl -r -t   | Not supported             |

**macOS space switching** maps space indices 1-9 to AppleScript key codes:
1=18, 2=19, 3=20, 4=21, 5=23, 6=22, 7=26, 8=28, 9=25. This requires
Mission Control keyboard shortcuts to be enabled in System Preferences.

---

## Shortcuts

| Action | macOS               | Linux                              | Windows                        |
|--------|---------------------|------------------------------------|--------------------------------|
| list   | `shortcuts list`    | ls ~/.local/bin, ~/bin, /usr/local/bin | Get-Command Application,Script |
| run    | `shortcuts run X`   | Try paths then fallback to command | powershell & 'name'            |

On macOS, this integrates with the Shortcuts.app framework.

---

## Text-to-Speech (TTS)

| Platform | Tool     | Voice param | Rate param                    |
|----------|----------|-------------|-------------------------------|
| macOS    | `say`    | `-v name`   | `-r wpm`                      |
| Linux    | espeak / spd-say | `-v name` | `-s wpm` / `-r wpm`    |
| Windows  | System.Speech.Synthesis | SelectVoice | Rate = wpm/30 clamped [-10,10] |

---

## Dock (macOS)

macOS-only resource. Three actions:

- **badges:** Lists foreground processes (approximation — true badge counts
  require private API)
- **recent:** `defaults read com.apple.dock recent-apps`
- **is_running:** `exists process "AppName"` via System Events

All three return explicit "only available on macOS" errors on other platforms.

---

## Notifications

| Action | macOS                          | Linux                            | Windows                         |
|--------|--------------------------------|----------------------------------|---------------------------------|
| send   | display notification           | notify-send                      | BurntToast / System.Windows.Forms |
| alert  | display alert (blocking modal) | zenity / kdialog / notify-send   | System.Windows.MessageBox       |

**Windows notification fallback:** If the BurntToast PowerShell module is
installed, it uses toast notifications. Otherwise, it falls back to the
.NET `NotifyIcon` balloon tip.

---

## DesktopDaemon -- Persistent PowerShell (Windows)

**File:** `crates/tools/src/desktop_daemon.rs` (~141 lines)

Windows-only optimization. Every PowerShell subprocess has ~500-1000ms startup
overhead. The daemon keeps a single PowerShell process alive across all
desktop tool invocations.

### Architecture

```
+----------------------------------+
|  DesktopDaemon                   |
|  inner: Mutex<Option<DaemonProcess>>  |
+----------------------------------+
         |
         v
+----------------------------------+
|  DaemonProcess                   |
|  child: Child                    |
|  stdin: ChildStdin               |
|  reader: BufReader<ChildStdout>  |
+----------------------------------+
         |
         v
+----------------------------------+
|  powershell -NoProfile -NoLogo   |
|  -Command -                      |
|  (reads from stdin, writes to    |
|   stdout until killed)           |
+----------------------------------+
```

### Sentinel Protocol

Scripts are delimited by a sentinel string `___NEBO_END___`:

```
1. Write to stdin:  <script>\nWrite-Output '___NEBO_END___'\n
2. Read stdout lines until sentinel line appears
3. Everything before sentinel = script output
```

### Lifecycle

```rust
pub async fn execute(&self, script: &str, timeout: Duration) -> Result<String, String>
```

1. Lock the inner `Mutex`
2. Check if process is alive (`try_wait()`)
3. If dead or not started, spawn new PowerShell with `kill_on_drop(true)`
4. Write script + sentinel to stdin
5. Read lines until sentinel or timeout
6. On timeout: kill the stuck process, clear state for restart
7. On EOF: clear state, return partial output if any
8. On write error: clear state for restart next call

### Global Singleton

```rust
static PS_DAEMON: OnceLock<Arc<DesktopDaemon>> = OnceLock::new();

fn ps_daemon() -> &'static Arc<DesktopDaemon> {
    PS_DAEMON.get_or_init(|| Arc::new(DesktopDaemon::new()))
}
```

Initialized lazily on first Windows desktop tool use. One per process.
The internal `Mutex` serializes all access — not mutable global state.

### Fallback

If the daemon fails (write error, process crash), the tool falls back to
spawning a fresh `powershell -NoProfile -Command` subprocess:

```rust
async fn run_powershell(script: &str) -> ToolResult {
    // Try daemon first
    match daemon.execute(script, Duration::from_secs(30)).await {
        Ok(out) => return ToolResult::ok(out),
        Err(e) => tracing::debug!("persistent PowerShell failed, falling back"),
    }
    // Fallback to subprocess
    run_command("powershell", &["-NoProfile", "-Command", script]).await
}
```

---

## OsTool Integration

The `DesktopTool` is NOT registered independently in the tool registry. It is
embedded inside `OsTool` as a field:

```rust
pub struct OsTool {
    file_tool: FileTool,
    shell_tool: ShellTool,
    desktop_tool: DesktopTool,    // <-- embedded
    app_tool: AppTool,
    settings_tool: SettingsTool,
    music_tool: MusicTool,
    keychain_tool: KeychainTool,
    spotlight_tool: SpotlightTool,
    store: Option<Arc<db::Store>>,
}
```

Routing in `OsTool.execute_dyn()`:

```rust
match resource.as_str() {
    "file" => self.file_tool.execute(ctx, input),
    "shell" => self.shell_tool.execute(ctx, input).await,

    // Desktop resources — delegate to DesktopTool
    "window" | "input" | "clipboard" | "capture" | "notification" | "ui" | "menu"
    | "dialog" | "space" | "shortcut" | "tts" | "dock" => {
        self.desktop_tool.execute_dyn(ctx, input).await
    }

    "app" => self.app_tool.execute_dyn(ctx, input).await,
    "settings" => { /* remapped to SettingsTool */ },
    // ...
}
```

### Resource Inference

When the LLM omits the `resource` field, `OsTool` can infer it from the action:

```
click, type, press, move, double_click, right_click, hotkey, scroll, drag, paste -> "input"
screenshot, see -> "capture"
speak -> "tts"
```

### Tool Corrections

If the LLM tries to call `desktop(...)` directly (legacy name), the registry
returns a correction message:

```
"INSTEAD USE: os(resource: \"window\", action: \"list\") or
 os(resource: \"capture\", action: \"screenshot\") — desktop is now under os"
```

---

## Tool Registry and Activation

The `os` tool is deferred (not loaded into the LLM context until needed).
Desktop-related keywords trigger its activation via `tool_filter.rs`:

```rust
("desktop", &[
    "click", "mouse", "keyboard", "window", "screenshot", "screen",
    "capture", "visible", "see what", "gui", "menu", "dialog",
    "accessibility", "drag", "scroll", "type in", "hotkey", "tts",
    "speak", "say aloud", "dock", "virtual desktop",
]),
```

When any of these keywords appear in the conversation, the "desktop"
sub-context activates, which causes the `os` tool to be included in the
LLM's available tools. This saves ~8-10K tokens on requests that don't
involve OS operations.

---

## Resource Permits and Concurrency

Desktop tools that control physical input devices declare `ResourceKind::Screen`:

```rust
fn resource_permit(&self, input: &serde_json::Value) -> Option<ResourceKind> {
    match resource {
        "window" | "input" | "ui" | "menu" | "dialog" | "space" | "shortcut" => {
            Some(ResourceKind::Screen)
        }
        // capture, app, clipboard, notification, tts, dock -> None (parallelizable)
        _ => None,
    }
}
```

The registry acquires a per-resource mutex before executing, preventing
concurrent agents/workflows from fighting over the same physical device.
`capture`, `clipboard`, `tts`, and `notification` are parallelizable and don't
acquire the screen permit.

Additionally, `is_concurrent_safe()` marks `capture` actions (`screenshot`, `see`)
as safe for concurrent execution within a single agent run.

---

## Tauri Integration

The Tauri desktop shell (`src-tauri/src/main.rs`) runs the Nebo server as an
embedded binary. The desktop tool operates independently of Tauri — it uses
platform-native APIs (AppleScript, xdotool, PowerShell) rather than Tauri
commands.

The relationship is indirect:
- Tauri provides the native window shell and system tray
- The embedded HTTP server runs on :27895
- Desktop tool commands execute via spawned subprocesses
- There is no direct Tauri IPC for desktop automation

This means the desktop tool works identically whether Nebo runs as a Tauri
desktop app or as a headless CLI server — the only requirement is that the
underlying platform tools are available.

---

## Key Structs and Signatures

### DesktopTool

```rust
pub struct DesktopTool { ... }

impl DesktopTool {
    pub fn new() -> Self;
}

impl DynTool for DesktopTool {
    fn name(&self) -> &str;                     // "desktop"
    fn description(&self) -> String;            // Full resource/action docs
    fn schema(&self) -> serde_json::Value;      // JSON Schema
    fn requires_approval(&self) -> bool;        // true
    fn execute_dyn(&self, ctx, input) -> Pin<Box<dyn Future<Output=ToolResult>>>;
}
```

### DesktopDaemon (Windows)

```rust
pub struct DesktopDaemon { inner: Mutex<Option<DaemonProcess>> }

impl DesktopDaemon {
    pub fn new() -> Self;
    pub async fn execute(&self, script: &str, timeout: Duration) -> Result<String, String>;
}
```

### DesktopSnapshot types

```rust
pub struct Rect { pub x: i64, pub y: i64, pub width: i64, pub height: i64 }
pub struct UIElement { pub id, role, label, bounds, actionable, keyboard_shortcut }
pub struct Snapshot { pub id, app, created_at, elements }
pub struct SnapshotStore { snapshots: VecDeque<Snapshot> }

pub fn generate_snapshot_id() -> String;
pub fn assign_element_ids(elements: &mut [UIElement]);
pub fn parse_ax_output(output: &str) -> Vec<UIElement>;
```

### Handler Functions

Each resource has a top-level handler and per-action implementations:

```rust
async fn handle_window(action, input) -> ToolResult;
async fn handle_input(action, input, snapshot_store) -> ToolResult;
async fn handle_clipboard(action, input) -> ToolResult;
async fn handle_notification(action, input) -> ToolResult;
async fn handle_capture(action, input, snapshot_store, ax_cache) -> ToolResult;
async fn handle_ui(action, input) -> ToolResult;
async fn handle_menu(action, input) -> ToolResult;
async fn handle_dialog(action, input) -> ToolResult;
async fn handle_space(action, input) -> ToolResult;
async fn handle_shortcut(action, input) -> ToolResult;
async fn handle_tts(action, input) -> ToolResult;
async fn handle_dock(action, input) -> ToolResult;
```

### Helper Functions

```rust
// macOS
async fn run_osascript(script: &str) -> ToolResult;
async fn run_osascript_raw(script: &str) -> Result<String, String>;
fn escape_applescript(s: &str) -> String;
fn key_name_to_code(key: &str) -> &str;

// Cross-platform
async fn run_command(cmd: &str, args: &[&str]) -> ToolResult;
async fn run_command_raw(cmd: &str, args: &[&str]) -> Result<String, String>;

// Windows
async fn run_powershell(script: &str) -> ToolResult;
fn escape_powershell(s: &str) -> String;
fn key_name_to_sendkeys(key: &str) -> &str;

// Linux
fn which(cmd: &str) -> bool;
async fn pipe_to_command(cmd: &str, args: &[&str], text: &str) -> ToolResult;

// Image processing
fn compress_and_encode(img_bytes: &[u8], quality: &str) -> ToolResult;
```

---

## Security Considerations

### Accessibility Permissions

**macOS:** Desktop automation requires Accessibility permission in
System Preferences > Security & Privacy > Privacy > Accessibility.
Without it, AppleScript calls to System Events will fail with
"not allowed assistive access" errors. The application (Nebo.app or Terminal)
must be explicitly granted this permission.

**Screen Recording:** The `screencapture` tool on macOS 10.15+ requires
Screen Recording permission. Without it, captured images will be blank or
show only the desktop wallpaper.

**Windows:** UI Automation and `user32.dll` P/Invoke calls generally work
without elevated permissions for applications running in the same session.
However, interacting with elevated (admin) windows from a non-elevated
process will fail silently.

**Linux:** `xdotool` works under X11 without special permissions. Under
Wayland, most input simulation and window management tools are restricted
by design. The `wlroots`-based `ydotool` is an alternative but requires
root or input group membership.

### Input Injection Risks

The desktop tool can:
- Type arbitrary text into any focused application
- Click any screen coordinate
- Send keyboard shortcuts (including destructive ones like Ctrl+W, Cmd+Q)
- Read clipboard contents (may contain passwords)
- Take screenshots (may capture sensitive information)

Mitigations:
1. **User approval required** for input, window, ui, menu, dialog, space,
   and shortcut operations (not auto-approved)
2. **ResourceKind::Screen permit** serializes access — no concurrent
   screen automation from multiple agents
3. **Entity resource grants** can deny "screen" access per-entity
4. **Screenshot compression** reduces data size but images may still
   contain sensitive content in the base64 data URL

### AppleScript Injection

The `escape_applescript()` function escapes backslashes and double quotes:
```rust
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
```

The `escape_powershell()` function escapes single quotes:
```rust
fn escape_powershell(s: &str) -> String {
    s.replace('\'', "''")
}
```

These prevent injection attacks when user-provided text (app names, labels,
typed text) is interpolated into scripts. However, the escaping is basic
and could potentially be bypassed with crafted Unicode or control characters.

---

## Platform-Specific Behavior

### macOS

- **Primary automation:** AppleScript via `osascript` + `cliclick` for mouse
- **Dependencies:** `cliclick` (brew install cliclick) for mouse operations
- **AX tree:** Full accessibility tree via System Events AppleScript
- **Screenshot:** Native `screencapture` with JPEG output support
- **TTS:** Built-in `say` command
- **Shortcuts:** Shortcuts.app integration
- **Dock:** Full dock query support
- **Unique features:** Menu bar access, status menu, Mission Control spaces

### Linux

- **Primary automation:** `xdotool` (X11) + `wmctrl`
- **Dependencies:** xdotool, wmctrl (optional), wl-paste/wl-copy (Wayland)
- **AX tree:** Minimal AT-SPI2 support via python3+gdbus
- **Screenshot:** gnome-screenshot / scrot (X11) / grim (Wayland)
- **TTS:** espeak / spd-say
- **Limitations:** No menu access, limited dialog support, no dock
- **Wayland:** Input simulation is severely restricted by protocol design

### Windows

- **Primary automation:** PowerShell + .NET + P/Invoke (user32.dll)
- **UI Automation:** System.Windows.Automation (UIAutomation)
- **AX tree:** Empty (not yet implemented via capture_ax_elements)
- **Screenshot:** System.Drawing.Graphics.CopyFromScreen
- **TTS:** System.Speech.Synthesis.SpeechSynthesizer
- **Unique:** DesktopDaemon for persistent PowerShell
- **Notifications:** BurntToast module (optional) or balloon tips

### Feature Availability Matrix

| Feature            | macOS | Linux | Windows |
|--------------------|-------|-------|---------|
| Window management  | Full  | wmctrl/xdotool | Full |
| Mouse click        | cliclick | xdotool | P/Invoke |
| Keyboard input     | AppleScript | xdotool | SendKeys |
| Clipboard          | Full  | Full (X11+Wayland) | Full |
| Screenshot         | Full  | Full  | Full    |
| AX tree (see)      | Full  | Empty | Empty   |
| UI tree/find/click | Full  | Partial | Full  |
| Menu access        | Full  | None  | Partial |
| Dialog interaction | Full  | Partial | Full  |
| Virtual desktops   | Partial | Full | Partial |
| TTS                | Full  | Full  | Full    |
| Dock               | Full  | N/A   | N/A     |
| Shortcuts          | Full  | Partial | Partial |
| Persistent daemon  | N/A   | N/A   | Full    |

---

## Error Handling

### Pattern

All handler functions return `ToolResult`:

```rust
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    pub image_url: Option<String>,
}
```

Errors are returned as `ToolResult::error(message)` with descriptive messages
that include the available actions or required parameters.

### Common Error Categories

1. **Missing parameters:** `"'app' parameter required for focus"` — returned
   when a required field is empty or absent.

2. **Unknown action:** `"Unknown window action 'X'. Use: list, focus, ..."` —
   includes the full list of valid actions.

3. **Platform unsupported:** `"Window management is not supported on this
   platform"` — for platforms not in the `cfg(target_os)` set.

4. **Missing dependency:** `"Window list requires wmctrl or xdotool (install
   with your package manager)"` — Linux-specific when tools are not installed.

5. **Element not found:** `"Element 'B99' not found. Use capture(action: \"see\")
   first to detect elements."` — when an element_id doesn't exist in any snapshot.

6. **Subprocess failure:** `"Command 'cliclick' failed: ..."` — when the
   underlying tool exits non-zero or fails to start.

7. **Screenshot failure:** Image decode/encode errors fall back to returning
   raw uncompressed bytes with a warning log.

### Graceful Degradation

- Screenshot compression: if image decode or JPEG encode fails, returns raw
  bytes with `tracing::warn` and uncompressed size in content.
- PowerShell daemon: if persistent process fails, falls back to subprocess.
- Linux tools: cascading fallbacks (wmctrl -> xdotool, wl-paste -> xclip -> xsel).
- AX cache: if lock poisoned, returns empty cache result (treats as cache miss).

---

## Image Compression Pipeline

The `compress_and_encode()` function handles screenshot post-processing:

```
Input: raw image bytes (PNG or JPEG)
       quality: "low" | "medium" | "high"

Auto-detect format via magic bytes:
  0xFF 0xD8 = JPEG
  otherwise = PNG

Quality levels:
  "high":   No processing, raw base64 encode
  "medium": Decode -> resize to max 1280px width -> 65% JPEG -> base64
  "low":    Decode -> resize to max 800px width  -> 50% JPEG -> base64

Resize: image::imageops::FilterType::Triangle (bilinear)
Encode: image::codecs::jpeg::JpegEncoder::new_with_quality

Output: ToolResult with image_url = "data:image/{mime};base64,{data}"
        content = "Screenshot captured (123KB -> 45KB, 1280x720 65% JPEG)"
```

The `image` crate is used for decode, resize, and JPEG encode. The JPEG
output is always used for low/medium regardless of input format.

---

## Tests

Test coverage in `desktop_tool.rs`:

| Test                              | What it verifies                           |
|-----------------------------------|--------------------------------------------|
| test_tool_metadata                | name, description, requires_approval, schema resources |
| test_schema_params                | All 12+ parameter fields exist in schema   |
| test_escape_applescript (macOS)   | Backslash and quote escaping               |
| test_key_name_to_code (macOS)     | Key name -> AppleScript key code mapping   |
| test_unknown_resource             | Error message for invalid resource         |
| test_missing_resource             | Error when resource field absent           |
| test_unknown_actions_for_new_resources | All 7 resources reject "nonexistent" |
| test_input_missing_params         | type/press/hotkey without required params  |
| test_tts_missing_text             | speak without text                         |
| test_dock_missing_app             | is_running without app                     |
| test_clipboard_read (macOS)       | Live clipboard read succeeds               |
| test_window_list (macOS)          | Live window list succeeds                  |
| test_window_focus_missing_app     | focus without app parameter                |
| test_window_move_missing_app      | move without app parameter                 |
| test_clipboard_clear              | Invalid action includes "clear" in error   |
| test_notification_alert_missing_message | alert without message              |

Test coverage in `desktop_snapshot.rs`:

| Test                              | What it verifies                           |
|-----------------------------------|--------------------------------------------|
| test_snapshot_store_insert_and_retrieve | Insert + get by ID works           |
| test_snapshot_store_latest        | latest() returns most recent               |
| test_snapshot_store_lru_eviction  | 30 inserts evicts oldest, stays <= 25      |
| test_get_element                  | Element lookup within snapshot             |
| test_rect_center                  | Center point calculation                   |
| test_element_id_generation        | B1, T1, B2, S1 assignment                 |
| test_parse_ax_output              | Two valid lines parsed correctly           |
| test_parse_ax_output_malformed    | Malformed lines skipped                    |
| test_generate_snapshot_id         | Format: starts with "snap_", length > 15   |

Test coverage in `desktop_daemon.rs`:

| Test                | What it verifies             |
|---------------------|------------------------------|
| test_daemon_creation| Inner state starts as None   |
