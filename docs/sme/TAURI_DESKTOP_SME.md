# TAURI_DESKTOP_SME.md

Subject-matter expert reference for the Nebo Tauri 2 desktop application.

Covers architecture, window management, system tray, global hotkeys, the
custom `neboapp://` protocol, Chrome native messaging relay, sleep/wake
recovery, build pipeline, security model, and platform-specific behaviour.

---

## 1. Architecture Overview

Nebo Desktop is a **single-binary** Tauri 2 application that bundles:

1. The **Rust backend** (Axum HTTP server + SQLite + AI agent runtime)
2. The **SvelteKit frontend** (pre-built static assets or Vite dev server)
3. A native **system tray**, **global hotkeys**, and a **custom URI protocol**

The Tauri process is the outermost shell. It spawns the Rust server on a
background thread, waits for it to become ready, then opens the main
webview window pointing at the server's HTTP port.

```
+-----------------------------------------------------------------------+
|                        Nebo Desktop Process                           |
|                                                                       |
|  +-------------------------------+  +------------------------------+  |
|  |        Tauri Main Thread      |  |    Server Thread (tokio)     |  |
|  |                               |  |                              |  |
|  |  - Window management          |  |  - Axum HTTP on :27895       |  |
|  |  - System tray                |  |  - WebSocket hub             |  |
|  |  - Global hotkeys             |  |  - Agent runner              |  |
|  |  - neboapp:// protocol        |  |  - SQLite (r2d2 pool)       |  |
|  |  - Chrome native relay        |  |  - Tool registry             |  |
|  |  - Window state persistence   |  |  - Memory system             |  |
|  |  - Sleep/wake detection       |  |  - NeboLoop gateway          |  |
|  |  - Drag-and-drop relay        |  |  - Plugin sandbox            |  |
|  |                               |  |  - MCP bridge                |  |
|  +-------------------------------+  +------------------------------+  |
|                                                                       |
|  +-------------------------------+  +------------------------------+  |
|  |  Main Webview ("main")        |  |  App Webviews ("app-*")     |  |
|  |  URL: localhost:5173 (dev)    |  |  URL: neboapp://{id}/       |  |
|  |  URL: localhost:27895 (prod)  |  |  Custom protocol handler    |  |
|  +-------------------------------+  +------------------------------+  |
|                                                                       |
|  +-------------------------------+                                    |
|  |  Prompt Webview ("prompt")    |                                    |
|  |  600x80, floating, no chrome  |                                    |
|  |  URL: localhost:27895/prompt  |                                    |
|  +-------------------------------+                                    |
+-----------------------------------------------------------------------+
```

### Entry Points

Nebo has two entry points that share the same server crate:

| Entry Point | Crate | Binary | Window |
|---|---|---|---|
| `src-tauri/src/main.rs` | `nebo` | `Nebo.app` / `Nebo.exe` | Tauri webview |
| `crates/cli/src/main.rs` | `nebo-cli` | `nebo` | Headless (terminal) |

Both call `server::run(cfg, quiet)` to start the Axum server. The desktop
version wraps it in a background thread; the CLI awaits it directly.

---

## 2. Startup Sequence

```
main()
  |
  +-- [1] Check for Chrome extension launch args
  |     If chrome-extension:// arg found --> run_native_messaging() and exit
  |
  +-- [2] Load .env (dotenvy::dotenv)
  |
  +-- [3] Initialize tracing
  |     - stdout layer (ANSI colors, RUST_LOG filter)
  |     - file layer (~/.nebo/logs/nebo.log, append mode)
  |
  +-- [4] Install panic hook
  |     - Logs to tracing::error
  |     - Writes to ~/.nebo/logs/nebo-crash.log
  |
  +-- [5] Load configuration
  |     - config::Config::load_embedded()  (etc/nebo.yaml baked in)
  |     - config::load_settings()          (~/.nebo/settings.json)
  |     - Merge auth secrets from settings into config
  |     - config::ensure_data_dir()
  |
  +-- [6] Spawn server thread
  |     std::thread::spawn --> tokio::Runtime::new() --> server::run(cfg, true)
  |     On exit: std::process::exit(0) -- kills entire process
  |
  +-- [7] Wait for server (poll)
  |     TcpStream::connect("127.0.0.1:27895") up to 60 times, 250ms apart
  |     (15-second timeout, then proceed anyway with warning)
  |
  +-- [8] Load saved window state
  |     load_state("main") from ~/.nebo/data/window-state.json
  |
  +-- [9] Build Tauri application
  |     - Register invoke_handler (get_window_state)
  |     - Register plugins (shell, global-shortcut, single-instance)
  |     - Register neboapp:// URI scheme protocol
  |     - Run setup closure (window, tray, hotkeys)
  |
  +-- [10] Enter Tauri event loop
        .run() with RunEvent handler for sleep/wake
```

### Server Thread Lifecycle

The server thread creates its own tokio runtime independently of Tauri's
event loop. This is intentional: the Axum server needs a multi-threaded
runtime, while Tauri's main thread manages the native event loop.

```rust
std::thread::spawn(move || {
    let rt = tokio::runtime::Runtime::new().expect("...");
    rt.block_on(async { server::run(cfg, true).await });
    std::process::exit(0);  // Kill everything when server exits
});
```

The `quiet: true` flag suppresses the "Starting server on ..." banner
(the Tauri window is the user's interface, not the terminal).

If the server exits for any reason (SIGTERM, unrecoverable error), the
entire process terminates via `std::process::exit(0)`.

---

## 3. Window Management

### 3.1 Window Types

Nebo creates up to three categories of webview windows:

| Label | Purpose | URL Source | Decorations | Persistence |
|---|---|---|---|---|
| `"main"` | Primary app shell | `frontend_url()` | Yes (title bar) | Saved to disk |
| `"prompt"` | Quick-input overlay | `{SERVER_URL}/prompt` | No (borderless) | Not saved |
| `"app-{agentId}"` | Per-app pop-out | `neboapp://{agentId}/` | Yes | Saved to disk |

### 3.2 Main Window

Created during `setup()`:

```rust
WebviewWindowBuilder::new(app, "main", WebviewUrl::External(frontend_url().parse().unwrap()))
    .title("Nebo")
    .inner_size(w, h)            // Restored or default (1280x860)
    .min_inner_size(800.0, 600.0)
    .visible(false)              // Hidden until fully loaded
    .on_navigation(|url| { ... })
    .on_new_window(|url, _| { ... })
    .build()?;
```

The window starts hidden (`visible(false)`) to prevent a flash of white
while the webview loads. After build, the saved position is restored and
the window is made visible:

```rust
if let Some(ref s) = saved {
    let _ = window.set_position(LogicalPosition::new(s.x, s.y));
}
WINDOW_READY.store(true, Ordering::SeqCst);
let _ = window.show();
```

`WINDOW_READY` is a static `AtomicBool` that gates window-state saving.
Without it, the initial `Moved`/`Resized` events from window creation
would overwrite the user's saved position.

### 3.3 Frontend URL Resolution

```rust
fn frontend_url() -> &'static str {
    if cfg!(debug_assertions) {
        "http://localhost:5173"    // Vite HMR dev server
    } else {
        "http://localhost:27895"   // Nebo backend serves built SvelteKit
    }
}
```

In development (`cargo tauri dev`), the Vite dev server at :5173 provides
hot module replacement. In production, the pre-built SvelteKit app is
served by the Axum backend at :27895.

### 3.4 Navigation Guards

Two guards prevent the webview from navigating away from the app:

**`on_navigation`** -- fires for in-page navigations (links, form submits):
- Allow: `localhost`, `127.0.0.1`, Stripe domains
- Deny + open externally: everything else

**`on_new_window`** -- fires for `window.open()` / `target="_blank"`:
- Allow: `localhost`, `127.0.0.1`, Stripe domains
- Deny + open externally: everything else

Stripe domains are whitelisted for the in-webview payment flow (PaymentElement,
Link, hCaptcha, 3D-Secure):

```rust
fn is_stripe_domain(host: &str) -> bool {
    host.ends_with(".stripe.com") || host == "stripe.com"
        || host.ends_with(".stripecdn.com") || host == "stripecdn.com"
        || host.ends_with(".stripe.network") || host == "stripe.network"
        || host.ends_with(".hcaptcha.com") || host == "hcaptcha.com"
        || host.ends_with(".link.co") || host == "link.co"
}
```

### 3.5 External URL Opening

External URLs are opened via `open::that()` (system default browser).
A dedup mechanism prevents rapid-fire double-opens:

```rust
static LAST_OPENED_URL: Mutex<Option<(String, Instant)>> = Mutex::new(None);

fn open_external(url: &str) {
    let mut last = LAST_OPENED_URL.lock().unwrap();
    if let Some((ref prev_url, ref when)) = *last {
        if prev_url == url && when.elapsed().as_millis() < 2000 {
            return;  // Suppress duplicate within 2 seconds
        }
    }
    *last = Some((url.to_string(), Instant::now()));
    let _ = open::that(url);
}
```

Both `on_navigation` and `on_new_window` can fire for the same URL
(browser-specific behaviour), hence the 2-second dedup window.

### 3.6 Prompt Window (Quick Input)

A floating borderless window for rapid text input, toggled by the global
hotkey (`Cmd+Shift+Space` on macOS, `Ctrl+Shift+Space` elsewhere).

```rust
fn toggle_prompt_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("prompt") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.set_focus();
        }
    } else {
        // First use: create the window
        WebviewWindowBuilder::new(app, "prompt", WebviewUrl::External(url))
            .inner_size(600.0, 80.0)
            .resizable(false)
            .decorations(false)
            .always_on_top(true)
            .visible(true)
            .center()
            .build();
    }
}
```

Properties:
- 600x80 logical pixels, centered on screen
- No title bar (`decorations(false)`)
- Always on top
- Not resizable
- Created lazily on first activation, then show/hide toggled

### 3.7 App Windows (Pop-out Apps)

Third-party apps (agents with UI) open in dedicated windows with the
`neboapp://` custom protocol. Created from the frontend via the app
launcher (`app/src/lib/apps/launcher.ts`):

```typescript
const appUrl = `neboapp://${agentId}/`;
const wv = new WebviewWindow(label, {
    url: appUrl,
    title: cfg.title,
    width: saved?.width ?? cfg.width,
    height: saved?.height ?? cfg.height,
    x: saved?.x,
    y: saved?.y,
    resizable: cfg.resizable
});
```

The launcher:
1. Checks for an existing window by label (`app-{agentId}`)
2. If found, focuses it; if stale handle, destroys and recreates
3. Invokes the `get_window_state` Tauri command to restore saved geometry
4. Falls back to browser popup if not running inside Tauri

---

## 4. Window State Persistence

All window positions and sizes are stored in a single JSON file at
`~/.nebo/data/window-state.json`.

### 4.1 Data Model

```rust
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct WindowState {
    x: f64,      // Logical pixels
    y: f64,
    width: f64,
    height: f64,
}

type WindowStates = HashMap<String, WindowState>;
// { "main": {...}, "app-portfolio": {...}, ... }
```

All values are stored in **logical pixels** (DPI-independent). The
`save_state()` function divides physical pixels by the scale factor:

```rust
let width = size.width as f64 / scale;
let height = size.height as f64 / scale;
let x = pos.x as f64 / scale;
let y = pos.y as f64 / scale;
```

### 4.2 Minimum Size Guard

Windows smaller than 400x300 logical pixels are not saved and not restored.
This prevents saving state from partially-initialized or corrupted windows.

### 4.3 Save Triggers

State is saved on `WindowEvent::Moved` and `WindowEvent::Resized`, with
two guards:

1. **WINDOW_READY gate**: The `"main"` window only saves after
   `WINDOW_READY` is set to `true` (after initial show). This prevents
   saving stale geometry from window creation events.
2. **Minimized/maximized skip**: State is not saved when the window is
   minimized or maximized (those are transient states).

### 4.4 Migration from Old Format

The file format was originally a single `WindowState` object (for just
the main window). The loader detects both formats:

```rust
fn load_all_states() -> WindowStates {
    // Try new format (map of label -> state)
    if let Ok(map) = serde_json::from_str::<WindowStates>(&data) {
        return map;
    }
    // Migrate old format (single WindowState -> map with "main" key)
    if let Ok(s) = serde_json::from_str::<WindowState>(&data) {
        let mut map = WindowStates::new();
        map.insert("main".to_string(), s);
        return map;
    }
    WindowStates::new()
}
```

### 4.5 Tauri Command: get_window_state

Exposed to the frontend so app windows can restore their saved geometry:

```rust
#[tauri::command]
fn get_window_state(label: String) -> Option<WindowState> {
    load_state(&label)
}
```

Registered via `tauri::generate_handler![get_window_state]`.

---

## 5. System Tray

The system tray provides persistent access to Nebo when the main window
is hidden (close-to-tray behavior).

### 5.1 Tray Icon

```rust
let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?;
TrayIconBuilder::new()
    .icon(tray_icon)
    .icon_as_template(true)  // macOS: adapts to menu bar light/dark
    .tooltip("Nebo -- Running")
```

The icon is embedded at compile time from `src-tauri/icons/tray-icon.png`.
`icon_as_template(true)` tells macOS to treat it as a template image
(automatically inverts for dark menu bars).

### 5.2 Tray Menu

```
+---------------------------+
|  Show Nebo                |
|  Hide                     |
|---------------------------|
|  Check for Updates...     |
|  Help & Documentation     |
|  Send Feedback            |
|---------------------------|
|  Quit Nebo                |
+---------------------------+
```

Menu item IDs and actions:

| ID | Action |
|---|---|
| `"show"` | Unminimize + show + focus the main window |
| `"hide"` | Hide the main window |
| `"check_update"` | Show + focus main window, then eval `window.__NEBO_CHECK_UPDATE__()` |
| `"help"` | Open `https://neboloop.com/docs` in browser |
| `"feedback"` | Open `https://github.com/NeboLoop/nebo/issues` in browser |
| `"quit"` | `app.exit(0)` -- terminates the entire process |

### 5.3 Tray Icon Click

Left-click on the tray icon shows/focuses the main window:

```rust
.on_tray_icon_event(|tray, event| {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event {
        // Unminimize + show + focus
    }
})
```

Only `MouseButtonState::Up` is handled (not Down) to match platform
conventions. Right-click shows the context menu (default Tauri behavior).

---

## 6. Global Hotkeys

### 6.1 Registration

```rust
app.global_shortcut().on_shortcut(
    if cfg!(target_os = "macos") {
        "CmdOrCtrl+Shift+Space"
    } else {
        "Ctrl+Shift+Space"
    },
    move |_app, shortcut, event| {
        if event.state != ShortcutState::Pressed { return; }
        toggle_prompt_window(&handle);
    },
)?;
```

The shortcut is registered via the `tauri-plugin-global-shortcut` plugin.
It fires system-wide, even when Nebo is not focused.

| Platform | Shortcut | Effect |
|---|---|---|
| macOS | Cmd+Shift+Space | Toggle prompt window |
| Windows | Ctrl+Shift+Space | Toggle prompt window |
| Linux | Ctrl+Shift+Space | Toggle prompt window |

Only the `Pressed` state triggers the toggle (not `Released`).

---

## 7. App Lifecycle Events

### 7.1 Close Requested (Hide to Tray)

The main window does **not** close when the user clicks the close button.
Instead, it hides to the system tray:

```rust
tauri::WindowEvent::CloseRequested { api, .. } => {
    save_state(window);
    if window.label() == "main" {
        api.prevent_close();
        let _ = window.hide();
    }
    // App windows close normally
}
```

The server continues running in the background. Users quit via:
- Tray menu "Quit Nebo"
- `Cmd+Q` (macOS) / `Alt+F4` after re-showing (platform-specific)

App windows (`app-*`) close normally -- their state is saved first.

### 7.2 Drag and Drop

Tauri intercepts OS-level file drops before they reach the browser's
`ondrop` event. The Tauri handler bridges this via `eval()`:

```
OS file drop
  |
  +-- DragEnter  --> eval("if(window.__NEBO_DRAG_ENTER__)window.__NEBO_DRAG_ENTER__()")
  +-- DragLeave  --> eval("if(window.__NEBO_DRAG_LEAVE__)window.__NEBO_DRAG_LEAVE__()")
  +-- Drop       --> eval("if(window.__NEBO_INSERT_FILES__) window.__NEBO_INSERT_FILES__([...])")
```

Dropped file paths are serialized as a JSON string array and passed to the
frontend's `window.__NEBO_INSERT_FILES__` global callback.

### 7.3 Sleep/Wake Recovery (RunEvent::Resumed)

When the system wakes from sleep, Tauri fires `RunEvent::Resumed`. Nebo
uses this to reconnect the NeboLoop WebSocket gateway:

```rust
.run(|_app, event| {
    if let tauri::RunEvent::Resumed { .. } = event {
        // Debounce: only reconnect if >5s since last resume
        // Fire-and-forget POST to /api/v1/neboloop/reconnect
    }
});
```

**Debounce mechanism**: Tauri fires `Resumed` many times per wake cycle.
The `LAST_RESUME` static mutex tracks the last event time; reconnect only
fires if 5+ seconds have passed.

**Reconnect method**: A raw TCP POST is used instead of an HTTP client to
avoid adding extra dependencies. The request hits the local backend's
`/api/v1/neboloop/reconnect` endpoint, which tears down the stale WS
connection and re-establishes it.

```
Tauri RunEvent::Resumed
  |
  +-- Debounce check (5s window)
  |
  +-- std::thread::spawn (fire-and-forget)
       |
       +-- TcpStream::connect("127.0.0.1:27895")
       +-- POST /api/v1/neboloop/reconnect HTTP/1.1
```

The server also has its own reconnect watcher with exponential backoff and
wall-clock drift detection (for cases where the Tauri `Resumed` event
does not fire, e.g., headless mode on a laptop).

### 7.4 Single Instance Enforcement

The `tauri-plugin-single-instance` plugin ensures only one Nebo instance
runs at a time. If a second instance is launched, the existing one is
brought to the foreground:

```rust
.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}))
```

---

## 8. Custom URI Protocol: neboapp://

The `neboapp://` protocol serves third-party app UIs from the local
filesystem, with bridge script injection and server-side fallback.

### 8.1 URL Format

```
neboapp://{agent_id}/{path}
```

Example: `neboapp://portfolio/index.html` serves the Portfolio app's UI.

### 8.2 Resolution Flow

```
neboapp://{agent_id}/{path}
  |
  +-- [1] Resolve UI directory
  |     ~/.nebo/data/user/agents/{id}/ui/   (user agents, higher priority)
  |     ~/.nebo/data/nebo/agents/{id}/ui/   (marketplace agents)
  |     Case-insensitive fallback scan
  |     404 if not found
  |
  +-- [2] Directory traversal check
  |     Reject paths containing ".."
  |
  +-- [3] Try exact file
  |     Empty path --> index.html
  |     If HTML: inject bridge script into <head>
  |     Return with correct MIME type
  |
  +-- [4] Not a file --> proxy to server
  |     Try: http://127.0.0.1:27895{path}
  |     Try: http://127.0.0.1:27895/apps/{agent_id}/api{path}
  |     Return first non-404 response
  |
  +-- [5] SPA fallback
  |     If path has no extension: serve index.html (with bridge injection)
  |
  +-- [6] 404 Not Found
```

### 8.3 Bridge Script Injection

Every HTML page served via `neboapp://` gets a bridge script injected
into `<head>`. The bridge:

1. Injects `<meta>` tags for the app SDK:
   - `nebo-app-id` -- the agent ID
   - `nebo-base-url` -- `http://127.0.0.1:27895`
   - `htmx-config` -- disables HTMX same-origin restriction

2. Monkey-patches `window.fetch` and `XMLHttpRequest.prototype.open` to
   rewrite `neboapp://` URLs to `http://127.0.0.1:27895/apps/{id}/api/`:

```javascript
// Simplified bridge logic:
var api = "http://127.0.0.1:27895/apps/{id}/api";
function rewrite(url) {
    var match = url.match(/^neboapp:\/\/[^\/]+(\/.*)/);
    if (!match) return url;
    return api + match[1];
}
// Patch fetch() and XMLHttpRequest to use rewrite()
```

This allows apps written with relative API paths to transparently route
through the Nebo server's sidecar proxy.

### 8.4 MIME Type Detection

The protocol handler includes a static MIME type map:

| Extension | MIME Type |
|---|---|
| `.html`, `.htm` | `text/html; charset=utf-8` |
| `.css` | `text/css; charset=utf-8` |
| `.js`, `.mjs` | `application/javascript; charset=utf-8` |
| `.json` | `application/json; charset=utf-8` |
| `.png` | `image/png` |
| `.jpg`, `.jpeg` | `image/jpeg` |
| `.gif` | `image/gif` |
| `.svg` | `image/svg+xml` |
| `.ico` | `image/x-icon` |
| `.woff` | `font/woff` |
| `.woff2` | `font/woff2` |
| `.ttf` | `font/ttf` |
| `.wasm` | `application/wasm` |
| `.webp` | `image/webp` |
| `.avif` | `image/avif` |
| `.mp4` | `video/mp4` |
| `.webm` | `video/webm` |
| (other) | `application/octet-stream` |

### 8.5 Server Proxy Fallback

When a path does not match a static file, the protocol handler proxies
the request to the Nebo backend using `ureq` (synchronous HTTP client).
Two URLs are tried in order:

1. `http://127.0.0.1:27895{path}` -- direct path
2. `http://127.0.0.1:27895/apps/{agent_id}/api{path}` -- sidecar API

The first non-404 response is returned. This allows apps to use API
routes that are handled by their sidecar process on the server.

---

## 9. Chrome Native Messaging Relay

The Nebo binary doubles as a Chrome native messaging host. When launched
by a Chrome extension, it operates as a **headless stdin/stdout relay**
between the extension and the Nebo server's WebSocket.

### 9.1 Detection

```rust
let args: Vec<String> = std::env::args().collect();
if args.iter().any(|a| a.starts_with("chrome-extension://")) {
    // Run as relay, not as desktop app
    rt.block_on(run_native_messaging());
    return;
}
```

Chrome passes `chrome-extension://EXTENSION_ID/` as a command-line
argument when launching native messaging hosts.

### 9.2 Relay Architecture

```
+-------------------+     stdin/stdout     +-------------------+     WebSocket      +------------------+
|  Chrome Extension | <--(native msg)----> |  Nebo Relay       | <--(ws)----------> |  Nebo Server     |
|  (browser)        |   (length-prefixed   |  (this binary)    |   ws://127.0.0.1   |  (Axum)          |
|                   |    JSON)             |                   |   :27895/ws/       |                  |
+-------------------+                      +-------------------+   extension        +------------------+
```

### 9.3 Native Messaging Protocol

Chrome native messaging uses **length-prefixed JSON**:
- 4 bytes (little-endian u32): message length
- N bytes: JSON payload

The relay reads this format from stdin, parses JSON, and forwards to the
WebSocket. Messages from the WebSocket are written back to stdout in the
same format.

### 9.4 Special Message Handling

| Message Type | Handling |
|---|---|
| `"hello"` | Responds with `{"type": "connected"}` via stdout, also forwards to WS |
| `"ping"` | Responds with `{"type": "pong"}` via stdout (not forwarded) |
| (other) | Forwarded to WebSocket as-is |

### 9.5 Browser Detection

The relay detects which browser launched it by inspecting the parent
process name (`ps -p PPID -o comm=` on Unix):

```rust
fn detect_parent_browser() -> String {
    // Checks parent process name for: brave, chrome, firefox, edge, arc
    // Falls back to "unknown"
}
```

The detected browser name is sent in the initial `hello` message to the
server so the UI can display the connected browser.

### 9.6 Connection Retry

The relay retries the WebSocket connection with exponential backoff (up to
10 attempts, 500ms to 5000ms). This handles the case where the relay is
launched before the server has fully started.

---

## 10. IPC: Tauri Commands and Events

### 10.1 Registered Commands

| Command | Signature | Purpose |
|---|---|---|
| `get_window_state` | `fn(label: String) -> Option<WindowState>` | Returns saved window geometry for a label |

Registered via:
```rust
.invoke_handler(tauri::generate_handler![get_window_state])
```

Called from the frontend:
```typescript
const saved = await invoke<WindowState | null>('get_window_state', { label });
```

### 10.2 Eval-Based Communication (Rust -> Frontend)

Several features use `window.eval()` to call global JavaScript functions:

| Global Function | Caller | Purpose |
|---|---|---|
| `window.__NEBO_CHECK_UPDATE__()` | Tray menu "Check for Updates" | Triggers update check UI |
| `window.__NEBO_DRAG_ENTER__()` | DragDrop::Enter event | Signals drag-enter to frontend |
| `window.__NEBO_DRAG_LEAVE__()` | DragDrop::Leave event | Signals drag-leave to frontend |
| `window.__NEBO_INSERT_FILES__(json)` | DragDrop::Drop event | Passes dropped file paths to frontend |

All use the pattern `if(window.__NEBO_X__)window.__NEBO_X__()` for safe
invocation when the handler is not yet registered.

### 10.3 Frontend -> Backend Communication

The frontend communicates with the backend primarily via:
- **HTTP API** (`fetch('/api/v1/...')`) -- routed to Axum
- **WebSocket** (`ws://localhost:27895/ws`) -- real-time events
- **Tauri invoke** -- only for `get_window_state`
- **Tauri WebviewWindow API** -- window creation/management

---

## 11. Tauri Plugins

### 11.1 Active Plugins

| Plugin | Crate | Purpose |
|---|---|---|
| `tauri-plugin-shell` | `tauri-plugin-shell = "2"` | Open URLs in system browser via `shell:allow-open` |
| `tauri-plugin-global-shortcut` | `tauri-plugin-global-shortcut = "2"` | System-wide keyboard shortcuts |
| `tauri-plugin-single-instance` | `tauri-plugin-single-instance = "2"` | Prevent multiple instances |

### 11.2 Plugin Configuration

The `plugins` field in `tauri.conf.json` is empty (`{}`). All plugin
configuration is done programmatically in Rust.

---

## 12. Capabilities and Permissions

Defined in `src-tauri/capabilities/default.json`:

```json
{
  "identifier": "default",
  "description": "Default capabilities for the main window and app windows",
  "windows": ["main", "prompt", "app-*"],
  "remote": {
    "urls": ["http://localhost:*", "neboapp://*"]
  },
  "permissions": [
    "core:default",
    "core:window:allow-create",
    "core:window:allow-set-focus",
    "core:window:allow-close",
    "core:webview:allow-create-webview-window",
    "shell:allow-open",
    "global-shortcut:allow-register",
    "global-shortcut:allow-unregister"
  ]
}
```

### 12.1 Window Scope

Capabilities apply to three window patterns:
- `"main"` -- the primary window
- `"prompt"` -- the quick-input overlay
- `"app-*"` -- any app pop-out window (wildcard)

### 12.2 Remote URL Access

The webview is allowed to load:
- `http://localhost:*` -- any port on localhost (dev server, backend)
- `neboapp://*` -- the custom protocol for all agent IDs

### 12.3 Granted Permissions

| Permission | Purpose |
|---|---|
| `core:default` | Basic Tauri IPC |
| `core:window:allow-create` | Frontend can create new windows |
| `core:window:allow-set-focus` | Frontend can focus windows |
| `core:window:allow-close` | Frontend can close windows |
| `core:webview:allow-create-webview-window` | Frontend can create webview windows |
| `shell:allow-open` | Frontend can open URLs in system browser |
| `global-shortcut:allow-register` | Register global hotkeys |
| `global-shortcut:allow-unregister` | Unregister global hotkeys |

---

## 13. Security Model

### 13.1 Content Security Policy

CSP is **disabled** (`"csp": null`):

```json
"app": {
    "security": {
        "csp": null
    }
}
```

This is intentional because the webview loads content from the local
server (`localhost:27895`) rather than from Tauri's built-in asset
protocol. CSP restrictions would break the HTTP-based architecture.

### 13.2 Navigation Restrictions

Despite the relaxed CSP, navigation is tightly controlled:
- `on_navigation` blocks all external URLs (opens in system browser)
- `on_new_window` blocks all external pop-ups (opens in system browser)
- Stripe domains are the only external exception (payment flows)

### 13.3 Directory Traversal Prevention

The `neboapp://` protocol handler explicitly rejects paths containing `..`:

```rust
if clean_path.contains("..") {
    return http::Response::builder()
        .status(400)
        .body(b"Bad Request".to_vec())
        .unwrap();
}
```

### 13.4 macOS Entitlements

The app is signed with the following entitlements
(`assets/macos/nebo.entitlements`):

| Entitlement | Purpose |
|---|---|
| `com.apple.security.files.user-selected.read-write` | File open/save dialogs |
| `com.apple.security.files.downloads.read-write` | Downloads folder access |
| `com.apple.security.network.client` | Outbound network (API calls) |
| `com.apple.security.network.server` | Inbound network (local HTTP server) |
| `com.apple.security.device.audio-input` | Microphone for voice input |
| `com.apple.security.device.camera` | Camera for video capture |
| `com.apple.security.automation.apple-events` | Controlling Music, Calendar, etc. |
| `com.apple.security.personal-information.addressbook` | Contacts access |
| `com.apple.security.personal-information.calendars` | Calendar access |

### 13.5 Windows Subsystem

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
```

In release builds on Windows, this hides the console window. In debug
builds, the console remains visible for log output.

---

## 14. Build Configuration

### 14.1 tauri.conf.json

```json
{
  "productName": "Nebo",
  "version": "0.9.0",
  "identifier": "dev.neboloop.nebo",
  "build": {
    "beforeDevCommand": "cd ../app && pnpm dev",
    "devUrl": "http://localhost:5173",
    "frontendDist": "../app/build"
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ],
    "resources": {
      "bundled-napps/**/*": "bundled-napps/"
    }
  }
}
```

Key fields:

| Field | Value | Purpose |
|---|---|---|
| `productName` | `"Nebo"` | Display name in OS, title bar |
| `identifier` | `"dev.neboloop.nebo"` | Reverse-DNS bundle identifier |
| `build.beforeDevCommand` | `cd ../app && pnpm dev` | Auto-starts Vite dev server |
| `build.devUrl` | `http://localhost:5173` | Vite dev server URL |
| `build.frontendDist` | `../app/build` | Production SvelteKit output |
| `bundle.targets` | `"all"` | Build for all platforms |
| `bundle.resources` | `bundled-napps/**/*` | Bundled agents/plugins/skills |

### 14.2 Bundled Resources (napps)

The `bundled-napps/` directory is included in the app bundle:

```
src-tauri/bundled-napps/
  agents/         (.gitkeep)
  plugins/        (.manifest + binaries)
  skills/         (.gitkeep)
```

The `.manifest` file lists bundled plugin names:
```
gws, digest, nebo-pdf, nebo-office, email, peek, imessage,
reminders, watchdog, social, devlink, imagegen, ffmpeg, slack
```

### 14.3 Cargo Dependencies

```toml
[dependencies]
# Tauri core
tauri = { version = "2", features = ["tray-icon", "image-png"] }
tauri-plugin-shell = "2"
tauri-plugin-single-instance = "2"
tauri-plugin-global-shortcut = "2"

# Internal crates
server = { workspace = true }
config = { workspace = true }

# HTTP types (for protocol handler)
http = "1"
ureq = "2"              # Sync HTTP client for neboapp:// proxy

# System browser
open = "5"

# Shared workspace deps
tokio, serde, serde_json, dotenvy, tracing, tracing-subscriber,
tokio-tungstenite, futures, anyhow
```

Tauri features enabled:
- `tray-icon` -- system tray support
- `image-png` -- PNG image decoding for tray icon

### 14.4 Build Pipeline (Makefile)

```
make build-desktop
  |
  +-- make bundle-napps          # Copy plugin binaries into bundled-napps/
  +-- cd app && pnpm build       # Build SvelteKit frontend
  +-- cargo tauri build          # Compile Rust + package with Tauri

make app-bundle
  |
  +-- make build-desktop
  +-- cp Nebo.app to dist/
  +-- codesign with Developer ID + entitlements

make dmg
  |
  +-- make app-bundle
  +-- create-dmg (or hdiutil fallback)

make notarize
  |
  +-- make dmg
  +-- xcrun notarytool submit
  +-- xcrun stapler staple

make install
  |
  +-- make notarize
  +-- cp dist/Nebo.app /Applications/
```

### 14.5 Icon Assets

```
src-tauri/icons/
  tray-icon.png            # System tray (embedded via include_bytes!)
  icon.icns                # macOS app icon
  icon.ico                 # Windows app icon
  icon.png                 # Generic fallback
  32x32.png                # Small icon
  64x64.png                # Medium icon
  128x128.png              # Large icon
  128x128@2x.png           # Retina large icon
  Square*.png              # Windows tile icons (various sizes)
  StoreLogo.png            # Windows Store logo
  android/                 # Android launcher icons (mipmap-*)
  ios/                     # iOS AppIcon set
```

---

## 15. Platform-Specific Behavior

### 15.1 macOS

- **Hotkey**: `CmdOrCtrl+Shift+Space` (Cmd on macOS)
- **Tray icon**: Template image (auto-adapts to light/dark menu bar)
- **Close button**: Hides to tray (standard macOS behavior)
- **Quit**: `Cmd+Q` or tray menu "Quit Nebo"
- **Code signing**: Developer ID Application + entitlements
- **Notarization**: xcrun notarytool + stapler
- **Bundle format**: `.app` directory, distributed as `.dmg`

### 15.2 Windows

- **Console**: Hidden in release builds (`windows_subsystem = "windows"`)
- **Hotkey**: `Ctrl+Shift+Space`
- **Tray icon**: Standard Windows notification area
- **Close button**: Hides to tray
- **Bundle format**: `.exe` with NSIS or WiX installer

### 15.3 Linux

- **Hotkey**: `Ctrl+Shift+Space`
- **Tray icon**: AppIndicator or StatusNotifier (depends on DE)
- **Close button**: Hides to tray
- **Bundle format**: `.deb`, `.AppImage`

---

## 16. Logging and Crash Handling

### 16.1 Dual-Layer Logging

Tracing is configured with two layers:

1. **stdout**: ANSI-colored output to the terminal (useful during `cargo tauri dev`)
2. **File**: Appended to `~/.nebo/logs/nebo.log` (no ANSI, same filter)

Both layers use `RUST_LOG` environment variable for filtering, defaulting
to `info` level.

### 16.2 Panic Hook

A custom panic hook captures panics before the process dies:

```rust
std::panic::set_hook(Box::new(|info| {
    // Log via tracing::error
    // Write to ~/.nebo/logs/nebo-crash.log
    // Print to stderr
}));
```

The crash log is a separate file (`nebo-crash.log`) that persists even
if the tracing subscriber is in a broken state.

---

## 17. Data Directories

| Path | Contents |
|---|---|
| `~/.nebo/data/` | SQLite database, window state, agent data |
| `~/.nebo/data/window-state.json` | Window positions and sizes |
| `~/.nebo/data/user/agents/` | User-installed agent UIs |
| `~/.nebo/data/nebo/agents/` | Marketplace agent UIs |
| `~/.nebo/logs/nebo.log` | Application log |
| `~/.nebo/logs/nebo-crash.log` | Panic crash log |
| `~/.nebo/settings.json` | User auth, API keys |

The data directory can be overridden via `NEBO_DATA_DIR` environment
variable.

---

## 18. Key Statics and Globals

```rust
/// True after the main window has been shown. Gates window state saving.
static WINDOW_READY: AtomicBool = AtomicBool::new(false);

/// Deduplicates external URL opens (2-second window).
static LAST_OPENED_URL: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// Debounces sleep/wake reconnect events (5-second window).
static LAST_RESUME: Mutex<Option<Instant>> = Mutex::new(None);

/// Server backend address.
const SERVER_URL: &str = "http://localhost:27895";
```

---

## 19. Key Functions Reference

| Function | Purpose |
|---|---|
| `main()` | Entry point: relay detection, logging, config, server spawn, Tauri build |
| `frontend_url()` | Returns dev server URL (debug) or backend URL (release) |
| `is_stripe_domain(host)` | Checks if a domain should be allowed in-webview for payment flows |
| `open_external(url)` | Opens URL in system browser with 2-second dedup |
| `resolve_app_ui_dir(agent_id)` | Finds the filesystem path for an app agent's UI directory |
| `neboapp_bridge(agent_id)` | Generates the bridge `<script>` + `<meta>` tags injected into app HTML |
| `mime_from_extension(path)` | Maps file extension to MIME type string |
| `toggle_prompt_window(app)` | Creates or shows/hides the floating prompt window |
| `wait_for_server()` | Polls TCP port 27895 until the server is accepting connections |
| `load_all_states()` | Reads window-state.json, handles format migration |
| `load_state(label)` | Returns saved WindowState for a given window label |
| `save_state(window)` | Reads current window geometry and writes to disk |
| `get_window_state(label)` | Tauri command: exposes load_state to frontend |
| `run_native_messaging()` | Chrome native messaging relay (stdin/stdout <-> WebSocket) |
| `write_native_message(stdout, msg)` | Writes a length-prefixed JSON message to stdout |
| `detect_parent_browser()` | Identifies which browser launched the native messaging host |

---

## 20. Relationship to Other SME Documents

| Document | Relationship |
|---|---|
| `CHAT_SYSTEM.md` | Chat dispatch + WS hub that the main webview connects to |
| `APPS.md` | App platform whose UIs are served via `neboapp://` protocol |
| `TOOLS.md` | Tool registry loaded by the server thread |
| `MEMORY_AND_PROMPT.md` | Memory system running in the server thread |
| `SECURITY.md` | Broader security model including auth, encryption |
| `PLUGIN_SYSTEM.md` | Plugins bundled in `bundled-napps/` |
| `SIDECAR_TOOLS_SME.md` | Sidecar processes that app windows proxy to |

---

## 21. Common Development Scenarios

### Running in Development

```bash
# Terminal 1: Backend with hot reload
make dev

# Terminal 2: Frontend with HMR
cd app && pnpm dev

# OR: Tauri dev (starts both, but slower reload)
cargo tauri dev
```

When using `cargo tauri dev`, the `beforeDevCommand` in tauri.conf.json
automatically starts `pnpm dev` in the app directory.

### Building for Release

```bash
make build-desktop    # Builds frontend + Tauri app
make install          # Full pipeline: build -> sign -> notarize -> /Applications
```

### Debugging Window State

Window state is stored at `~/.nebo/data/window-state.json`:

```json
{
  "main": { "x": 100.0, "y": 200.0, "width": 1280.0, "height": 860.0 },
  "app-portfolio": { "x": 300.0, "y": 150.0, "width": 1024.0, "height": 768.0 }
}
```

Delete this file to reset all window positions to defaults.

### Debugging the Native Messaging Relay

The relay writes diagnostic messages to stderr (visible in Chrome's
native messaging log). Key messages:

```
[nebo-relay] starting native messaging bridge
[nebo-relay] connected to server at ws://127.0.0.1:27895/ws/extension
[nebo-relay] detected browser: chrome
[nebo-relay] WS connect attempt 1/10 failed (...), retrying in 500ms
[nebo-relay] stdin closed
[nebo-relay] shutting down
```

### Testing the neboapp:// Protocol

1. Install an agent with a `ui/` directory
2. Call `launchApp('agent-id', 'My App')` from the browser console
3. Or navigate to `neboapp://agent-id/` in a Tauri webview

---

## 22. Error Handling

### Server Startup Failure

If the server fails to bind port 27895 (e.g., another instance is running
despite the single-instance guard), `server::run()` returns
`NeboError::PortInUse`. The server thread logs the error and calls
`std::process::exit(0)`, terminating the entire process.

### Server Thread Timeout

If the server does not become ready within 15 seconds (60 polls x 250ms),
the Tauri window launches anyway with a warning log. The webview will show
a connection error until the server finishes starting.

### Panic Recovery

Panics in any thread are caught by the custom panic hook. The location and
payload are logged to both tracing and the crash log file. The process
then aborts (standard Rust behavior after the panic hook runs).

### Window State Corruption

If `window-state.json` contains invalid JSON or values, `load_all_states()`
returns an empty map, and windows use their default dimensions. The file
is silently overwritten on the next save.

### Native Messaging Failures

The relay handles several failure modes:
- **Server not ready**: Retries with exponential backoff (up to 10 attempts)
- **Message too large**: Rejects messages >1MB
- **Malformed JSON**: Logs error and skips the message
- **WebSocket disconnect**: Exits the relay process (Chrome will relaunch)
- **Stdin closed**: Exits the relay process (extension disconnected)
