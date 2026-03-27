# Browser Automation System — Comprehensive SME Reference

Source: `crates/browser/`, `crates/tools/src/web_tool.rs`, `crates/cli/src/main.rs`, `chrome-extension/`

This document covers the full browser automation pipeline in the Rust rewrite: every crate, struct, function, message type, content script, and failure mode.

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Message Flow](#2-message-flow)
3. [Chrome Extension](#3-chrome-extension)
4. [Native Messaging Relay Bridge](#4-native-messaging-relay-bridge)
5. [Server-Side WebSocket Handler](#5-server-side-websocket-handler)
6. [ExtensionBridge](#6-extensionbridge)
7. [ActionExecutor](#7-actionexecutor)
8. [Browser Manager](#8-browser-manager)
9. [Browser Config & Profiles](#9-browser-config--profiles)
10. [Chrome Launch & Detection](#10-chrome-launch--detection)
11. [Sessions & Pages](#11-sessions--pages)
12. [Accessibility Snapshots](#12-accessibility-snapshots)
13. [SnapshotStore](#13-snapshotstore)
14. [Action Parameter Structs](#14-action-parameter-structs)
15. [Storage Helpers](#15-storage-helpers)
16. [WebTool Integration](#16-webtool-integration)
17. [Native Messaging Host Manifest](#17-native-messaging-host-manifest)
18. [Content Scripts](#18-content-scripts)
19. [Native Message Protocol](#19-native-message-protocol)
20. [Timeouts and Error Handling](#20-timeouts-and-error-handling)
21. [Connection Lifecycle](#21-connection-lifecycle)
22. [Audit Logging](#22-audit-logging)
23. [Known Issues and Failure Modes](#23-known-issues-and-failure-modes)
24. [Debugging Guide](#24-debugging-guide)

---

## 1. Architecture Overview

Browser automation uses a **four-hop relay chain**. The agent never talks to Chrome directly — everything flows through the Chrome extension's native messaging bridge.

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                            BROWSER AUTOMATION CHAIN                                 │
│                                                                                     │
│  Agent (web_tool)                                                                   │
│    │                                                                                │
│    ▼                                                                                │
│  ExtensionBridge.execute(tool, args)         ← Rust, in-process                     │
│    │  routes to default browser's connection channel                                │
│    │                                                                                │
│    ▼                                                                                │
│  WS Handler (/ws/extension)                  ← Rust, Axum WebSocket                │
│    │  reads from per-browser channel, sends JSON over WS                            │
│    │                                                                                │
│    ▼                                                                                │
│  Relay Bridge Process (nebo binary)          ← Rust, separate OS process            │
│    │  reads WS message, writes to stdout (4-byte length prefix + JSON)              │
│    │                                                                                │
│    ▼                                                                                │
│  Chrome Extension (service worker)           ← TypeScript, runs in Chrome           │
│    │  reads native message, dispatches to tools.ts                                  │
│    │  executes via CDP (chrome.debugger) or content scripts                         │
│    │                                                                                │
│    ▼                                                                                │
│  Result flows back the exact same path in reverse                                   │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

**Key insights:**
- The `nebo` binary serves double duty: normal mode (full server + agent), native messaging mode (lightweight stdin/stdout relay bridge detected by `chrome-extension://` arg)
- Multiple browsers can connect simultaneously — each gets its own relay process and connection channel
- Tool requests are routed to the system default browser's connection (detected at startup via macOS `defaults read`)

---

## 2. Message Flow

### Request (agent → extension)

```
1. Agent calls web(action: "navigate", url: "https://example.com")
2. WebTool.handle_browser() → manager.executor() → ActionExecutor.execute()
3. ActionExecutor → ExtensionBridge.execute("navigate", {"url": "..."})
4. ExtensionBridge:
   a. Finds connection matching default browser (or any available)
   b. Assigns monotonic request ID
   c. Creates oneshot channel for response
   d. Stores (id → sender) in pending HashMap
   e. Sends ToolRequest to that connection's mpsc channel
5. WS handler (extension_ws_handler):
   a. recv from this connection's request channel
   b. Formats JSON: {"type": "execute_tool", "id": N, "tool": "navigate", "args": {...}}
   c. Sends over WebSocket
6. Relay bridge (run_native_messaging):
   a. Reads WS text message
   b. Writes to stdout: 4-byte LE length + JSON bytes
7. Chrome extension (native.ts handleMessage):
   a. Reads message, dispatches to handleToolRequest()
   b. Calls ensureAgentTab() to get/create dedicated agent tab
   c. Calls executeTool("navigate", args, tabId)
8. tools.ts navigate():
   a. ensureDebuggerAttached(tabId) — attaches CDP 1.3
   b. chrome.debugger.sendCommand(tabId, "Page.navigate", {url})
   c. Waits 1000ms for page load
   d. Returns ok("Navigated to ...")
```

### Response (extension → agent)

```
1. Extension: sendToolResult(id, {content: "Navigated to ..."})
2. native.ts: port.postMessage({type: "tool_response", id, result: content})
3. Relay bridge: reads stdin (4-byte len + JSON), forwards as WS text message
4. WS handler: parses JSON, calls bridge.deliver_result(id, Ok(result))
5. ExtensionBridge: looks up oneshot sender in pending map, sends result
6. ExtensionBridge.execute() returns Ok(result) (was awaiting oneshot receiver)
7. ActionExecutor returns to WebTool
8. WebTool formats and returns ToolResult to agent
```

---

## 3. Chrome Extension

**Location:** `chrome-extension/`
**Manifest:** MV3, version 0.2.0
**Extension IDs:**
- Production (Web Store): `heaeiepdllbncnnlfniglgmbfmmemkcg`
- Development (unpacked): `bmkkjdcmjiebhegfibdnbimjpkmaickm`

**Build:** `esbuild` (TypeScript → JS), output in `dist/`

### 3.1 Manifest Permissions

| Permission | Purpose |
|-----------|---------|
| `debugger` | CDP access for navigate, click, screenshot, evaluate |
| `tabs` | Create/close/query tabs for agent tab lifecycle |
| `activeTab` | Fallback tab access |
| `storage` | Extension settings persistence |
| `alarms` | Keep-alive alarm (every 24s) to prevent service worker suspension |
| `scripting` | Inject content scripts and execute functions in tab context |
| `nativeMessaging` | Connect to `dev.neboloop.nebo` native messaging host |
| `<all_urls>` (host) | Access any URL for content script injection and CDP |

### 3.2 Source Files

| File | Purpose |
|------|---------|
| `src/background.ts` | Service worker entry point. Connects native messaging on startup, handles toolbar click, keep-alive alarm, stop-agent messages. |
| `src/native.ts` | Native messaging connection management. Handles `execute_tool`, `show_indicators`, `hide_indicators`, `ping` messages. Manages agent tab lifecycle (create/reuse/close). |
| `src/tools.ts` | Tool implementations. 17+ tools using CDP and content scripts. |
| `src/badge.ts` | Extension badge UI (ON/OFF/connecting/error states). |
| `src/types.ts` | Badge state types and color config. |
| `src/options.ts` | Options page — connection test (opens native port, sends hello, checks for response). |
| `src/content/accessibility-tree.ts` | Content script injected at `document_start` on all frames. Provides `window.__neboGenerateAccessibilityTree()` for page reading. |
| `src/content/visual-indicator.ts` | Content script injected at `document_idle` on top frame. Pulsing orange glow border + "Stop Nebo" button. |

### 3.3 Agent Tab Lifecycle

The extension maintains a dedicated **agent tab** (`agentTabId` in `native.ts`):

1. **Creation:** First tool request triggers `ensureAgentTab()`, which creates `about:blank` tab
2. **Reuse:** Subsequent requests reuse the same tab (checked via `chrome.tabs.get()`)
3. **Tracking:** `chrome.tabs.onRemoved` clears `agentTabId` if user closes the tab
4. **Cleanup:** `hide_indicators` message closes the agent tab after a 400ms delay
5. **Visual indicators:** Shown on agent tab only (glow + stop button), hidden during tool execution to avoid screenshot noise

### 3.4 Tool Implementations

| Tool | Method | Mechanism |
|------|--------|-----------|
| `navigate` | CDP `Page.navigate` | Attaches debugger, sends command, waits 1s |
| `read_page` | `chrome.scripting.executeScript` | Calls accessibility tree content script, retries by injecting manually if not loaded |
| `click` | CDP `Input.dispatchMouseEvent` | Resolves element via WeakRef map, gets bounding rect, dispatches mousePressed + mouseReleased |
| `double_click` | CDP `Input.dispatchMouseEvent` | Two rapid click sequences |
| `triple_click` | CDP `Input.dispatchMouseEvent` | Three rapid click sequences (selects all text in a field) |
| `right_click` | CDP `Input.dispatchMouseEvent` | Click with `button: 2` (context menu) |
| `hover` | CDP `Input.dispatchMouseEvent` | mouseMoved to element center |
| `fill` / `form_input` | `chrome.scripting.executeScript` | Sets `.value` + dispatches input/change events |
| `type` | CDP `Input.dispatchKeyEvent` | Character-by-character keyDown/keyUp |
| `select` | `chrome.scripting.executeScript` | Sets `<select>.value` + dispatches change event |
| `screenshot` | CDP `Page.captureScreenshot` | Returns base64 PNG |
| `scroll` | `chrome.scripting.executeScript` | `window.scrollBy(x, y)` |
| `scroll_to` | `chrome.scripting.executeScript` | Element `.scrollIntoView()` |
| `press` | CDP `Input.dispatchKeyEvent` | Mapped key names (Enter, Tab, etc.) to CDP key codes, supports chords (cmd+a) and sequences |
| `drag` | CDP `Input.dispatchMouseEvent` | mousePressed at start → mouseMoved to end → mouseReleased |
| `go_back` | CDP `Page.navigateToHistoryEntry(-1)` | Falls back to `history.back()` |
| `go_forward` | CDP `Page.navigateToHistoryEntry(1)` | Falls back to `history.forward()` |
| `wait` | `setTimeout` | Capped at 10s |
| `evaluate` | CDP `Runtime.evaluate` | Runs arbitrary JS, returns value |
| `new_tab` | `chrome.tabs.create` | Updates agentTabId to the new tab |
| `close_tab` | `chrome.tabs.remove` | Clears agentTabId if closing agent tab |
| `list_tabs` | `chrome.tabs.query({})` | Returns all tabs (no agent tab needed) |
| `devtools_console` | CDP | Browser console logs |
| `devtools_source` | CDP | Page source |
| `devtools_storage` | CDP | localStorage/sessionStorage |
| `devtools_dom` | CDP | DOM inspection |
| `devtools_cookies` | CDP | Cookie inspection |
| `devtools_performance` | CDP | Performance metrics |

### 3.5 Keep-Alive Mechanism

MV3 service workers suspend after ~30s of inactivity. The extension uses a Chrome alarm (`keep-alive`) firing every 24s (0.4 minutes) to:
1. Prevent service worker suspension
2. Reconnect native messaging if disconnected

Created on `runtime.onInstalled` and verified on startup.

### 3.6 Native Messaging Reconnect

On disconnect (`handleDisconnect` in `native.ts`):
1. Sets `connected = false`, shows disconnected badge
2. Schedules reconnect after 2s (single timer, no overlap)
3. Keep-alive alarm also calls `connect()` if `!isConnected()`

---

## 4. Native Messaging Relay Bridge

**File:** `crates/cli/src/main.rs`, function `run_native_messaging()`

### 4.1 Detection

Before CLI arg parsing (`main()` lines 88-98):
```rust
let args: Vec<String> = std::env::args().collect();
if args.iter().any(|a| a.starts_with("chrome-extension://")) {
    return run_native_messaging().await;
}
```

Chrome passes `chrome-extension://EXTENSION_ID/` as a CLI argument when launching the native messaging host. The binary detects this and enters relay mode.

Also available as explicit `nebo relay` subcommand.

### 4.2 Connection

Connects to `ws://127.0.0.1:27895/ws/extension` with exponential backoff:
- 10 attempts max
- Delay: `min(500 * 2^(attempt-1), 5000)` ms → 500, 1000, 2000, 4000, 5000, 5000...
- On exhaustion: `process::exit(1)` — Chrome will relaunch via onDisconnect

### 4.3 Browser Identification

Before entering the relay loop, the bridge:
1. Detects which browser launched it via `detect_parent_browser()` (checks parent process name via `ps`)
2. Sends a `hello` message to the server: `{"type": "hello", "browser": "chrome", "relay": true}`
3. The WS handler reads this first message to identify the browser and register the connection

### 4.4 Bidirectional Bridge

Two concurrent tokio tasks:

**Task 1: stdin → WS (extension → server)**
- Reads 4-byte LE length prefix + JSON from stdin (Chrome native messaging protocol)
- Handles `hello` locally (responds with `{"type": "connected"}` via stdout) AND forwards to server
- Handles `ping` locally (responds with `{"type": "pong"}` via stdout)
- Everything else forwarded as WS text message

**Task 2: WS → stdout (server → extension)**
- Reads WS text messages
- Writes as native messages (4-byte LE length + JSON) to stdout

### 4.5 Termination

When either task finishes (stdin closes or WS breaks):
- `tokio::select!` returns
- `process::exit(0)` — force exit because tokio's blocking stdin thread prevents clean shutdown
- Chrome's `onDisconnect` fires, extension schedules reconnect after 2s

---

## 5. Server-Side WebSocket Handler

**File:** `crates/server/src/handlers/ws.rs`, function `extension_ws_handler`
**Route:** `GET /ws/extension`

### 5.1 Handler Entry

```rust
pub async fn extension_ws_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    let bridge = state.extension_bridge.clone();
    ws.on_upgrade(move |socket| handle_extension_ws(socket, bridge))
}
```

### 5.2 Connection Handling

```rust
async fn handle_extension_ws(socket: WebSocket, bridge: Arc<ExtensionBridge>) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // 1. Wait for first message — a "hello" with "browser" field
    // 2. Register connection: bridge.connect(browser) → (conn_id, request_rx)
    // 3. Task 1: Read from request_rx → send to WS (tool requests to extension)
    // 4. Task 2: Read from WS → deliver results to bridge

    tokio::select! { ... }

    bridge.disconnect(conn_id).await;
}
```

### 5.3 Send Task (bridge → WS → relay → extension)

Loops on the per-connection `request_rx`. For each `ToolRequest`:
```json
{"type": "execute_tool", "id": 1, "tool": "navigate", "args": {"url": "..."}}
```

### 5.4 Receive Task (extension → relay → WS → bridge)

Parses incoming WS messages. For `type: "tool_response"`:
- Extracts `id` (i64) and either `error` (string) or `result` (JSON value)
- Calls `bridge.deliver_result(id, result)`

Also handles `hello`/`connected` (debug log) and unknown types (debug log).

---

## 6. ExtensionBridge

**File:** `crates/browser/src/extension_bridge.rs` (261 lines)
**Shared via:** `AppState.extension_bridge` (Arc)

### 6.1 State

```rust
pub struct ExtensionBridge {
    /// Active browser connections keyed by connection ID.
    connections: Arc<Mutex<HashMap<i64, BrowserConnection>>>,
    /// Pending responses keyed by request ID.
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>,
    /// Monotonic connection/request ID counter.
    next_id: Arc<AtomicI64>,
    /// The system default browser bundle ID (detected at startup).
    default_browser: Arc<Mutex<Option<String>>>,
    /// Timestamp of last active connection (for grace period on reconnect).
    last_connected: Arc<Mutex<Option<Instant>>>,
}
```

Each browser gets its own `BrowserConnection`:
```rust
struct BrowserConnection {
    tx: mpsc::Sender<ToolRequest>,  // capacity 64
    browser: String,                // e.g. "chrome", "brave"
}
```

### 6.2 Key Methods

**`new() -> Self`**
Creates the bridge and spawns a background task to detect the default browser via macOS `defaults read com.apple.LaunchServices/com.apple.launchservices.secure LSHandlers` for the HTTPS handler. Maps bundle IDs to short names: chrome, brave, firefox, safari, edge, arc.

**`connect(browser: String) -> (i64, mpsc::Receiver<ToolRequest>)`**
Creates a new `BrowserConnection` with a per-browser mpsc channel. Returns the connection ID and the receiver (consumed by the WS handler). Updates `last_connected` timestamp.

**`disconnect(conn_id: i64)`**
Removes the connection. If last connection drops, updates `last_connected` but does NOT reject pending requests — individual 30s timeouts handle truly dead connections. This prevents false rejections during the ~2s reconnect window.

**`is_connected() -> bool`**
Non-blocking check via `try_lock()` on connections map. If locked, assumes connected.

**`was_recently_connected(within: Duration) -> bool`**
Returns true if connected OR if `last_connected` is within the given duration. Used for the 3-second grace period in WebTool.

**`wait_for_connection(timeout: Duration) -> bool`**
Polls `is_connected()` every 100ms up to the timeout. Used by WebTool to wait for reconnection.

**`execute(tool, args) -> Result<Value, String>`**
1. Lock connections, find one matching default browser (or fall back to any)
2. Clone the target connection's `tx` channel
3. Assign request ID, create oneshot channel, store in pending
4. Send `ToolRequest` via the connection's channel
5. Wait with **30s timeout** on oneshot receiver
6. On timeout: remove from pending, return error with pending count
7. On receive: return the result

**`deliver_result(id, result)`**
Looks up oneshot sender in pending map, sends result. Called by WS handler.

### 6.3 ToolRequest Struct

```rust
pub struct ToolRequest {
    pub id: i64,
    pub tool: String,
    pub args: serde_json::Value,
}
```

---

## 7. ActionExecutor

**File:** `crates/browser/src/executor.rs` (58 lines)

Thin wrapper around `ExtensionBridge`. Used by `WebTool` to execute browser actions.

```rust
pub struct ActionExecutor {
    bridge: Arc<ExtensionBridge>,
}
```

- `is_connected()`: Delegates to `bridge.is_connected()`
- `was_recently_connected(within)`: Delegates to `bridge.was_recently_connected()`
- `wait_for_connection(timeout)`: Delegates to `bridge.wait_for_connection()`
- `execute(tool, args) -> Result<Value, BrowserError>`: Logs action, delegates to `bridge.execute()`, maps `String` errors to `BrowserError::Other`

---

## 8. Browser Manager

**File:** `crates/browser/src/manager.rs` (193 lines)

Manages browser instances, sessions, and the extension bridge.

```rust
pub struct Manager {
    config: BrowserConfig,
    data_dir: String,
    browsers: RwLock<HashMap<String, RunningChrome>>,  // Managed Chrome instances
    sessions: RwLock<HashMap<String, Arc<Session>>>,    // CDP sessions
    bridge: Arc<ExtensionBridge>,                       // Built-in, always created
}
```

**Key methods for extension automation:**
- `bridge() -> Arc<ExtensionBridge>`: Returns the bridge (used by server for WS wiring)
- `executor() -> Option<ActionExecutor>`: Always returns `Some(ActionExecutor)` wrapping the bridge
- `extension_connected() -> bool`: Delegates to `bridge.is_connected()`

**Managed Chrome methods** (for "nebo" driver profiles, NOT extension path):
- `launch(profile_name)`: Resolves profile config, launches Chrome with remote debugging, creates session
- `stop(profile_name)`: Kills Chrome process, removes session
- `get_or_create_session(profile_name)`: Returns existing or creates new (launches Chrome for "nebo" driver)
- `list_profiles()`: Returns `ProfileStatus` for all configured profiles
- `shutdown()`: Kills all managed Chrome instances

---

## 9. Browser Config & Profiles

**File:** `crates/browser/src/config.rs` (106 lines)

### 9.1 BrowserConfig

```rust
pub struct BrowserConfig {
    pub enabled: bool,              // default: false
    pub control_port: u16,          // default: 9223
    pub executable_path: Option<String>,
    pub headless: bool,             // default: false
    pub no_sandbox: bool,           // default: false
    pub profiles: HashMap<String, ProfileConfig>,
}
```

### 9.2 Default Profiles

```
"nebo": driver="nebo" (managed), cdp_port=9222, color="#6366f1"
"chrome": driver="extension" (extension-based, no managed Chrome)
```

### 9.3 ProfileConfig

```rust
pub struct ProfileConfig {
    pub driver: String,             // "nebo" or "extension"
    pub cdp_port: Option<u16>,
    pub cdp_url: Option<String>,
    pub color: Option<String>,
}
```

### 9.4 ResolvedProfile

`resolve_profile(name, data_dir)` returns a `ResolvedProfile` with all defaults applied:
- `cdp_port`: defaults to 9222
- `color`: defaults to "#6366f1"
- `user_data_dir`: `{data_dir}/browser/{name}`
- `cdp_is_loopback`: always true

---

## 10. Chrome Launch & Detection

**File:** `crates/browser/src/chrome.rs` (208 lines)

### 10.1 Binary Detection (`find_chrome()`)

**macOS:** Checks these paths in order:
- `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`
- `/Applications/Chromium.app/Contents/MacOS/Chromium`
- `/Applications/Brave Browser.app/Contents/MacOS/Brave Browser`
- `/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge`
- `/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary`

**Linux:** `which` for: google-chrome, google-chrome-stable, chromium, chromium-browser, brave-browser, microsoft-edge

**Windows:** Checks program files paths, then registry query `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\chrome.exe`

### 10.2 RunningChrome

```rust
pub struct RunningChrome {
    pub pid: u32,
    pub executable: PathBuf,
    pub user_data_dir: String,
    pub cdp_port: u16,
    child: tokio::process::Child,
}
```

**`launch(executable, user_data_dir, cdp_port, headless, no_sandbox)`:**
1. Detect or use configured executable path
2. Create user data dir, clean stale lock files (SingletonLock, SingletonSocket, SingletonCookie)
3. Launch with args: `--remote-debugging-port`, `--user-data-dir`, `--no-first-run`, `--no-default-browser-check`, `--disable-background-timer-throttling`, `--disable-backgrounding-occluded-windows`
4. Optional: `--headless=new`, `--no-sandbox`
5. Wait for CDP port (polls `http://127.0.0.1:{port}/json/version` with 15s timeout, 200ms intervals)

**`ws_url()`:** Fetches `/json/version` and extracts `webSocketDebuggerUrl`.

**`kill()` / `Drop`:** Kills the child process.

---

## 11. Sessions & Pages

**File:** `crates/browser/src/session.rs` (185 lines)

### 11.1 Session

```rust
pub struct Session {
    pub profile_name: String,
    pub cdp_url: String,
    pages: RwLock<HashMap<String, Arc<Page>>>,
    active_target: RwLock<Option<String>>,
}
```

Methods: `add_page()`, `active_page()`, `set_active()`, `get_page()`, `remove_page()`, `page_ids()`, `page_count()`.

### 11.2 Page

```rust
pub struct Page {
    pub target_id: String,
    state: RwLock<PageState>,
    refs: RwLock<Vec<ElementRef>>,
}
```

**PageState:** `url`, `title`, `console_messages` (Vec, keeps last 100), `errors` (Vec, keeps last 50).

**Key methods:**
- `update_state(url, title)`: Update URL and title
- `add_console_message(msg)`: Append (caps at 100)
- `add_error(err)`: Append (caps at 50)
- `set_refs(refs)` / `get_refs()` / `clear_refs()`: Manage element references from accessibility snapshots
- `resolve_selector(input)`: If input starts with 'e' (e.g. "e1"), look up in refs and return CSS selector; otherwise return input as-is

---

## 12. Accessibility Snapshots

**File:** `crates/browser/src/snapshot.rs` (252 lines)

### 12.1 annotate_snapshot(snapshot, include_refs)

Takes a raw ARIA snapshot string and adds `[eN]` refs to interactive elements (sequential: e1, e2, e3...). Returns `(annotated_text, Vec<ElementRef>)`.

Interactive roles (from `actions.rs`): button, link, textbox, checkbox, radio, combobox, listbox, menuitem, tab, slider, spinbutton, switch, searchbox, textarea.

### 12.2 annotate_with_role_ids(snapshot)

Alternative annotation using **role-based IDs** (B1, T2, L3...) instead of sequential. Returns `(annotated_text, Vec<AnnotatedElement>)`.

Role prefix mapping: B=button, T=textbox/textarea/searchbox, L=link, C=checkbox, M=menuitem, S=slider/spinbutton, A=tab, R=radio, D=combobox/listbox, W=switch, E=generic.

Labels are truncated to 40 chars with "..." suffix.

---

## 13. SnapshotStore

**File:** `crates/browser/src/snapshot_store.rs` (170 lines)

TTL-based in-memory cache for annotated accessibility snapshots. Stored in `AppState`.

```rust
pub struct SnapshotStore {
    snapshots: RwLock<HashMap<String, Snapshot>>,
    ttl_secs: u64,  // 3600 = 1 hour
}
```

**Snapshot struct:** `id`, `created_at` (Instant), `app`, `window_title`, `annotated_text`, `elements` (Vec<AnnotatedElement>), `raw_image` (Option<Vec<u8>>).

**AnnotatedElement struct:** `id`, `role`, `label`, `bounds` (Option<(i32,i32,i32,i32)>), `actionable` (bool), `selector`.

**Methods:**
- `put(snapshot)` / `get(id)` / `latest()`
- `lookup_element(snapshot_id, element_id)`: Find element by role-based ID in a specific snapshot
- `lookup_element_latest(element_id)`: Find in most recent snapshot
- `cleanup()`: Remove expired snapshots (>1 hour)
- `len()`: Count cached snapshots

Uses `std::sync::RwLock` (not tokio) — read-heavy, never held across `.await`.

---

## 14. Action Parameter Structs

**File:** `crates/browser/src/actions.rs` (167 lines)

Typed option structs for each browser action:

| Struct | Key Fields | Defaults |
|--------|-----------|----------|
| `NavigateOptions` | `url`, `wait_until`, `timeout_ms` | wait_until="domcontentloaded", timeout=30000 |
| `ClickOptions` | `ref`, `selector`, `button`, `count`, `timeout_ms` | button="left", count=1, timeout=30000 |
| `TypeOptions` | `ref`, `selector`, `text`, `delay_ms`, `timeout_ms` | delay=0, timeout=30000 |
| `FillOptions` | `ref`, `selector`, `value`, `timeout_ms` | timeout=30000 |
| `SelectOptions` | `ref`, `selector`, `value`, `timeout_ms` | timeout=30000 |
| `HoverOptions` | `ref`, `selector`, `timeout_ms` | timeout=30000 |
| `PressOptions` | `key`, `ref`, `selector` | — |
| `ScrollOptions` | `direction`, `amount` | direction="down", amount=300 |
| `WaitOptions` | `ref`, `selector`, `state`, `timeout_ms` | state="visible", timeout=30000 |
| `ScreenshotOptions` | `ref`, `selector`, `full_page` | full_page=false |

**`resolve_target(page, ref, selector)`**: Resolves ref (via page.resolve_selector) or selector, returning error if neither provided.

**`INTERACTIVE_ROLES`**: Constant list of 14 roles used for snapshot annotation.

---

## 15. Storage Helpers

**File:** `crates/browser/src/storage.rs` (91 lines)

Helpers for web storage manipulation via CDP evaluate:

- `StorageKind` enum: `Local` / `Session` (maps to `localStorage` / `sessionStorage`)
- `StorageState`: Serializable state with cookies + local/session storage entries
- `StorageEntry`: `origin`, `key`, `value`
- JS snippet generators: `js_get_all_storage()`, `js_get_storage()`, `js_set_storage()`, `js_remove_storage()`, `js_clear_storage()`

---

## 16. WebTool Integration

**File:** `crates/tools/src/web_tool.rs` (1,026 lines)

The `web` domain tool handles four resource types: `http`, `search`, `browser`, `devtools`.

### 16.1 Resource Routing

```
action → infer_resource():
├─ fetch, get, post, put, delete, head, sanitize, patch → "http"
├─ search, query → "search"
├─ navigate, read_page, click, double_click, triple_click, right_click,
│  fill, form_input, type, screenshot, evaluate, launch, close,
│  list_pages, list_tabs, new_tab, close_tab, back, go_back, forward,
│  go_forward, reload, scroll, scroll_to, hover, select, press, key,
│  wait, drag, status, text, snapshot → "browser"
└─ console, source, storage, dom, cookies, performance → "devtools"
```

### 16.2 Browser Action Routing

```
web(action: "navigate", url: "...")
  → infer_resource("navigate") returns "browser"
  → handle_browser(input)
    → Check manager exists (Option<Arc<Manager>>)
    → "status" action works even disconnected
    → Get executor: manager.executor()
    → Check executor.is_connected()
    → If disconnected but was_recently_connected(3s):
        → wait_for_connection(3s) — poll 100ms intervals
        → If still not connected: "Browser extension reconnecting — try again in a moment"
    → If disconnected and NOT recently connected:
        → "Browser extension not connected. Install the Nebo Chrome/Brave extension..."
    → handle_browser_via_extension(executor, action, input)
```

### 16.3 Action Name Mapping

| Web Tool Action | Extension Tool |
|----------------|---------------|
| `snapshot`, `read_page` | `read_page` |
| `navigate` | `navigate` |
| `click` | `click` |
| `double_click` | `double_click` |
| `triple_click` | `triple_click` |
| `right_click` | `right_click` |
| `hover` | `hover` |
| `fill`, `form_input` | `form_input` |
| `type` | `type` |
| `select` | `select` |
| `screenshot` | `screenshot` |
| `scroll` | `scroll` |
| `scroll_to` | `scroll_to` |
| `press`, `key` | `press` |
| `drag` | `drag` |
| `back`, `go_back` | `go_back` |
| `forward`, `go_forward` | `go_forward` |
| `wait` | `wait` |
| `evaluate` | `evaluate` |
| `list_tabs` | `list_tabs` |
| `new_tab` | `new_tab` (requires non-empty URL) |
| `close_tab`, `close` | `close_tab` |

### 16.4 Argument Forwarding

`build_extension_args()` selectively forwards parameters per action:

| Action | Forwarded Keys |
|--------|---------------|
| `navigate`, `new_tab` | `url` |
| `click`, `double_click`, `triple_click`, `right_click` | `ref`, `selector`, `coordinate`, `x`, `y`, `modifiers` |
| `hover` | `ref`, `coordinate`, `x`, `y` |
| `fill`, `form_input` | `ref`, `selector`, `value` |
| `type` | `text` |
| `select` | `ref`, `selector`, `value` |
| `scroll` | `direction`, `amount`, `scroll_direction`, `scroll_amount`, `coordinate` |
| `scroll_to` | `ref` |
| `press`, `key` | `key`, `text`, `repeat` |
| `drag` | `start_coordinate`, `coordinate` |
| `wait` | `ms`, `duration` |
| `evaluate` | `expression`, `text` |
| `read_page` | `filter`, `depth`, `maxChars`, `refId` |
| DevTools actions | `url`, `selector`, `expression`, `filter` |

### 16.5 DevTools Resource

Maps user-facing actions to extension tool names:

| Action | Extension Tool |
|--------|---------------|
| `console` | `devtools_console` |
| `source` | `devtools_source` |
| `storage` | `devtools_storage` |
| `dom` | `devtools_dom` |
| `cookies` | `devtools_cookies` |
| `performance` | `devtools_performance` |

### 16.6 HTTP Resource

- Methods: GET, POST, PUT, DELETE, HEAD, PATCH
- Custom headers support, body support
- `sanitize` action: extracts visible text, chunks for LLM (default 4000 chars, configurable via `chunk_size`)
- SSRF protection: blocks private IPs (localhost, 127.*, 0.*, 10.*, 172.16-31.*, 192.168.*, [::1], 169.254.*)
- Response bodies >50KB shown in 20KB chunks with offset pagination
- HTML responses auto-stripped of tags for readability

### 16.7 Search Resource

BYOK providers checked in order:
1. `search-brave` (Brave Search API, X-Subscription-Token header)
2. `search-tavily` (Tavily API, key in JSON body)
3. `search-google` (Google CSE, key + cx params)
4. `search-serpapi` (SerpAPI, key as query param)

Fallback: Brave HTML scraping (no API key needed).

### 16.8 Resource Permissions

- `browser` and `devtools`: `ResourceKind::Browser` (exclusive lock during tool execution)
- `http` and `search`: No resource lock (parallelizable)
- `requires_approval`: true for all WebTool calls

### 16.9 Error Messages

| Condition | Error |
|-----------|-------|
| No browser manager | "Browser automation is not available. Use web(action: \"fetch\"...)" |
| Extension disconnected, recently connected | "Browser extension reconnecting — try again in a moment." |
| Extension disconnected, not recent | "Browser extension not connected. Install the Nebo Chrome/Brave extension..." |
| Unsupported action | Lists all available actions |
| new_tab without URL | "new_tab requires a URL." |

---

## 17. Native Messaging Host Manifest

**File:** `crates/browser/src/native_host.rs` (550 lines)

### 17.1 Installation

Called from `crates/server/src/lib.rs` on server startup, using `needs_manifest_update()` to check:

```rust
let nebo_binary = find_cli_binary_path();
let local_ext_id = cfg.browser_extension_id.as_deref().unwrap_or("");
if browser::native_host::needs_manifest_update(&nebo_binary, local_ext_id) {
    browser::native_host::install_manifest(&nebo_binary, local_ext_id);
}
```

**Important:** When running as `nebo-desktop` (Tauri GUI), the manifest points to the sibling `nebo` CLI binary — NOT the Tauri binary. The relay code lives in the CLI.

### 17.2 Manifest Content

Written as `dev.neboloop.nebo.json`:

```json
{
  "name": "dev.neboloop.nebo",
  "description": "Nebo Browser Automation Host",
  "path": "/path/to/nebo",
  "type": "stdio",
  "allowed_origins": [
    "chrome-extension://heaeiepdllbncnnlfniglgmbfmmemkcg/",
    "chrome-extension://bmkkjdcmjiebhegfibdnbimjpkmaickm/",
    "chrome-extension://LOCAL_EXTENSION_ID/"
  ]
}
```

Extension IDs in `allowed_origins`:
- `heaeiepdllbncnnlfniglgmbfmmemkcg` — Production (Chrome Web Store)
- `bmkkjdcmjiebhegfibdnbimjpkmaickm` — Development (unpacked from repo)
- Optional local extension ID: configurable via `cfg.browser_extension_id`

### 17.3 Browser Directories

**macOS:**
- `~/Library/Application Support/Google/Chrome/NativeMessagingHosts/`
- `~/Library/Application Support/BraveSoftware/Brave-Browser/NativeMessagingHosts/`
- `~/Library/Application Support/Microsoft Edge/NativeMessagingHosts/`
- `~/Library/Application Support/Chromium/NativeMessagingHosts/`

**Linux:**
- `~/.config/google-chrome/NativeMessagingHosts/`
- `~/.config/BraveSoftware/Brave-Browser/NativeMessagingHosts/`
- `~/.config/microsoft-edge/NativeMessagingHosts/`
- `~/.config/chromium/NativeMessagingHosts/`

**Windows:**
- `%LOCALAPPDATA%\Google\Chrome\User Data\NativeMessagingHosts\`
- `%LOCALAPPDATA%\BraveSoftware\Brave-Browser\User Data\NativeMessagingHosts\`
- `%LOCALAPPDATA%\Microsoft\Edge\User Data\NativeMessagingHosts\`

### 17.4 Windows Registry

Windows requires registry entries for Chrome to discover the host:
- `HKCU\Software\Google\Chrome\NativeMessagingHosts\dev.neboloop.nebo`
- `HKCU\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\dev.neboloop.nebo`
- `HKCU\Software\Microsoft\Edge\NativeMessagingHosts\dev.neboloop.nebo`

Each key's default value points to the manifest JSON file path.

### 17.5 Manifest Validation (`needs_manifest_update`)

Checks ALL installed manifests for:
1. Binary `path` matches current executable
2. `allowed_origins` includes both production AND dev extension IDs
3. Additional local extension ID (if configured)
4. On Windows: registry entries exist and point to correct manifest paths
5. Returns true if no manifests found at all (needs fresh install)
6. Returns true on corrupt JSON (needs reinstall)

### 17.6 NativeHost Struct (Direct Path)

The `NativeHost` struct in `native_host.rs` implements a direct stdin/stdout native messaging protocol — used when the binary IS the native messaging host (not via the relay bridge). Has its own `execute_tool()` with 30s timeout, `show_indicators()`, `hide_indicators()`, and a `run()` read loop for incoming messages.

---

## 18. Content Scripts

### 18.1 Accessibility Tree (`content/accessibility-tree.ts`)

**Injected:** `document_start`, all frames, all URLs
**Exposes:** `window.__neboGenerateAccessibilityTree(filter, depth, maxChars, refId)`
**Also exposes:** `window.__neboElementMap` (WeakRef-based element map)

Generates a text representation of the page's accessibility tree:

```
page "Hacker News" url="https://news.ycombinator.com"
  navigation [ref_1]
    link "Hacker News" [ref_2] href="news"
    link "new" [ref_3] href="newest"
  list [ref_4]
    listitem [ref_5]
      link "Show HN: Something cool" [ref_6] href="..."
```

**Features:**
- Role inference from HTML tags (40+ tag-to-role mappings)
- Interactive element detection (click handlers, tabindex, cursor: pointer)
- `all` filter: shows all meaningful elements
- `interactive` filter: only interactive elements (links, buttons, inputs)
- Viewport culling: skips off-screen elements (with 100px margin)
- `refId` focus: can zoom into a subtree starting from a specific ref
- `maxChars` limit: stops generation at character budget
- Depth limit: default 15 levels
- WeakRef element map: refs resolve to actual DOM elements for click/fill operations
- Accessible name resolution: aria-label → aria-labelledby → `<label for>` → parent label → placeholder → title → alt → text content

### 18.2 Visual Indicator (`content/visual-indicator.ts`)

**Injected:** `document_idle`, top frame only, all URLs
**Messages handled:**
- `SHOW_AGENT_INDICATORS`: Fade in glow border + slide in stop button
- `HIDE_AGENT_INDICATORS`: Fade out + remove elements after 350ms
- `HIDE_FOR_TOOL_USE`: Hide indicators without removing (prevents screenshot contamination)
- `SHOW_AFTER_TOOL_USE`: Restore indicators if they were visible before hide

**Visual elements:**
- Full-screen fixed overlay with pulsing inset box-shadow (Nebo brand orange #FFBE18)
- "Stop Nebo" button at bottom center, z-index 2147483647
- Both elements created lazily on first `show()` call
- Cleaned up on `beforeunload`

---

## 19. Native Message Protocol

Chrome native messaging uses a simple binary protocol: **4-byte little-endian length prefix + JSON body**.

Maximum message size: 1 MB (Chrome hard limit).

### 19.1 NativeMessage Struct

**File:** `crates/browser/src/native_types.rs` (112 lines)

```rust
pub struct NativeMessage {
    pub msg_type: String,     // "type" in JSON
    pub id: Option<i64>,
    pub tool: String,
    pub args: Option<Value>,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub version: Option<String>,
    pub extension_id: Option<String>,
}
```

Convenience constructors: `ok()`, `pong()`, `connected()`, `error_msg()`, `tool_request()`, `tool_response()`, `show_indicators()`, `hide_indicators()`.

### 19.2 Message Types Table

| Type | Direction | Fields | Purpose |
|------|-----------|--------|---------|
| `hello` | ext → host | `version`, `extension_id` | Handshake from extension |
| `connected` | host → ext | (none) | Handshake acknowledgement |
| `ping` | either | (none) | Keepalive |
| `pong` | either | (none) | Keepalive response |
| `execute_tool` | host → ext | `id`, `tool`, `args` | Request tool execution |
| `tool_response` | ext → host | `id`, `result` OR `error` | Tool execution result |
| `show_indicators` | host → ext | (none) | Show visual indicators on agent tab |
| `hide_indicators` | host → ext | (none) | Hide indicators and close agent tab |
| `tab_attached` | ext → host | `args` | Tab debugger attached (informational) |
| `tab_detached` | ext → host | `args` | Tab debugger detached (informational) |
| `stop_agent` | ext → host | (none) | User clicked "Stop Nebo" button |

---

## 20. Timeouts and Error Handling

### 20.1 Timeout Locations

| Component | Timeout | Location |
|-----------|---------|----------|
| ExtensionBridge.execute() | **30 seconds** | `extension_bridge.rs:170` |
| NativeHost.execute_tool() | **30 seconds** | `native_host.rs:179` |
| Reconnect grace period | **3 seconds** | `web_tool.rs:344-349` |
| Reconnect polling interval | **100ms** | `extension_bridge.rs:123` |
| Relay WS connection backoff | 500ms → 5s (10 attempts) | `cli/main.rs:428` |
| Extension reconnect | 2 seconds | `native.ts:224` |
| Extension keep-alive alarm | 24 seconds | `background.ts:42` |
| Navigate page load wait | 1 second (fixed) | `tools.ts:184` |
| CDP port wait (Chrome launch) | 15 seconds (200ms poll) | `chrome.rs:161` |
| HTTP client timeout | 30 seconds | `web_tool.rs:17` |
| Options page connection test | 3 seconds | `options.ts:38` |
| Indicator hide animation | 350ms | `visual-indicator.ts:178` |
| Agent tab close delay | 400ms | `native.ts:201` |
| Snapshot store TTL | 1 hour | `snapshot_store.rs:39` |

### 20.2 Error Propagation

```
Extension error (tools.ts catch block)
  → {type: "tool_response", id: N, error: "message"}
  → Relay forwards to server via WS
  → WS handler calls bridge.deliver_result(id, Err("message"))
  → ExtensionBridge.execute() returns Err("message")
  → ActionExecutor maps to BrowserError::Other
  → WebTool returns ToolResult::error("Browser action failed: message")
  → Agent sees error in tool result
```

### 20.3 Disconnection Handling

When the relay bridge process dies (WS closes):
1. WS handler's recv task exits → `tokio::select!` fires
2. `bridge.disconnect(conn_id)` called
3. If last connection: updates `last_connected` timestamp but does NOT reject pending requests
4. Individual tool 30s timeouts handle truly dead connections
5. Extension's `onDisconnect` fires → schedules 2s reconnect
6. WebTool's 3s grace period catches transient reconnects

When the extension disconnects (stdin closes):
1. Relay's send task exits (stdin read fails)
2. `tokio::select!` fires, `process::exit(0)`
3. Chrome's `onDisconnect` fires in extension → reconnect

---

## 21. Connection Lifecycle

### 21.1 Startup Sequence

```
1. Server starts (lib.rs run())
   a. Creates BrowserManager → creates ExtensionBridge (detects default browser in background)
   b. Checks needs_manifest_update() → installs manifest if needed
   c. Registers /ws/extension route

2. User opens Chrome with Nebo extension
   a. Service worker loads (background.ts)
   b. Calls native.connect()
   c. chrome.runtime.connectNative("dev.neboloop.nebo")
   d. Chrome reads manifest, launches /path/to/nebo with chrome-extension://ID/ arg

3. Relay bridge process starts (run_native_messaging)
   a. Detects parent browser via detect_parent_browser() (checks parent process name)
   b. Connects WS to ws://127.0.0.1:27895/ws/extension (with backoff)
   c. Sends {"type": "hello", "browser": "chrome", "relay": true} as first message
   d. WS handler reads first message, extracts browser name
   e. Calls bridge.connect("chrome") → registers connection with per-browser channel

4. Extension sends hello via native messaging
   a. Relay handles locally (responds with "connected") AND forwards to server
   b. WS handler receives hello, logs it

5. Bridge is now fully connected
   a. ExtensionBridge.is_connected() returns true
   b. Agent tool calls can flow through, routed to this browser's channel
```

### 21.2 Multiple Browser Handling

The bridge supports multiple simultaneous browser connections. Each relay process registers with its detected browser name. Tool requests are routed to the connection matching the default browser (detected via macOS LSHandlers). If no match, falls back to any available connection.

This handles:
- Multiple browsers running the Nebo extension simultaneously
- Overlap during reconnection — a new relay can connect before the old one fully disconnects

### 21.3 Server Restart Recovery

1. Server restarts → WS breaks
2. Relay's recv task sees WS close → `process::exit(0)`
3. Chrome's `onDisconnect` fires → extension schedules 2s reconnect
4. Extension calls `chrome.runtime.connectNative()` → Chrome launches new relay
5. New relay retries WS with backoff → connects when server is ready
6. Bridge reconnected

---

## 22. Audit Logging

**File:** `crates/browser/src/audit.rs` (22 lines)

Logs every tool request via `tracing`. Sensitive tools (`evaluate`, `screenshot`) get `warn!` level; others get `info!`.

Called from `NativeHost.execute_tool()` only (the direct desktop path). The extension bridge path logs at the `ActionExecutor` level.

---

## 23. Known Issues and Failure Modes

### 23.1 30s Timeout Too Short for Complex Pages

**Issue:** Navigation to heavy pages (e.g., SPAs that load async content) may exceed the 30s timeout. The navigate tool in `tools.ts` only waits 1s after `Page.navigate` — but the agent may call `read_page` before the page is fully loaded, getting partial or empty content. The *real* timeout is the 30s on the ExtensionBridge, which is appropriate for the round-trip but doesn't help with page-load timing.

### 23.2 Manifest Path Staleness

**Status: Fixed.** The `needs_manifest_update()` function now compares the manifest `path` field against the current binary path on startup and reinstalls if stale. Also validates `allowed_origins` and Windows registry entries.

### 23.3 Extension Disconnection During Tool Execution

If the extension or relay disconnects mid-tool-execution, the pending oneshot channel times out after 30s. The tool may have partially executed (e.g., navigation started but result never returned). The bridge no longer immediately rejects pending requests on disconnect — it lets timeouts handle truly dead connections.

### 23.4 Agent Tab and about:blank

The agent tab starts as `about:blank`. The first `navigate` command changes it to the target URL. If `read_page` is called before navigate, it reads `about:blank` — which returns an empty tree.

### 23.5 CDP Debugger Permission Dialog

When the extension first attaches the debugger to a tab, Chrome shows a yellow "debugging this browser" infobar. This can confuse users and may persist if the debugger isn't properly detached.

### 23.6 Content Script Injection Race

The accessibility tree content script is injected at `document_start`, but for newly created tabs (via `chrome.tabs.create`), there's a race — `ensureAgentTab()` creates the tab and immediately proceeds. The content script may not be loaded yet when `read_page` is called. The tool handles this by retrying with manual injection, but this adds latency.

### 23.7 "Cannot access contents of url" Error

This is Chrome's error when trying to execute scripts on restricted pages: `chrome://`, `chrome-extension://`, `about:blank` (sometimes), and URLs blocked by CSP. The extension catches this and returns it as a tool error.

### 23.8 Default Browser Detection Limited to macOS

The `detect_default_browser()` function only works on macOS (reads LSHandlers). On Linux and Windows, it falls back to "unknown", which means tool requests go to any available connection rather than the user's default browser. Not a problem when only one browser runs the extension.

---

## 24. Debugging Guide

### 24.1 Check Connection Status

**Agent tool:** `web(action: "status")`
Returns `Browser extension connected: true/false`

**Extension badge:**
- "ON" (yellow) = connected
- Empty = disconnected
- "..." = connecting
- "!" (red) = error

### 24.2 Verify Native Messaging Manifest

```bash
# macOS Chrome
cat ~/Library/Application\ Support/Google/Chrome/NativeMessagingHosts/dev.neboloop.nebo.json

# Check if the binary path is correct
# The "path" field should point to the actual nebo CLI binary
```

### 24.3 Check Relay Process

```bash
# See if relay is running
ps aux | grep nebo | grep chrome-extension

# Check server logs for WS connection
# Look for "extension connected" / "extension disconnected"
```

### 24.4 Extension Console

1. Navigate to `chrome://extensions/`
2. Find "Nebo Browser Relay"
3. Click "Inspect views: service worker"
4. Check console for `[nebo]` prefixed messages:
   - `[nebo] Native host connected`
   - `[nebo] Created agent tab: N`
   - `[nebo] Native host disconnected: ...`

### 24.5 Common Failure Causes

| Symptom | Likely Cause |
|---------|-------------|
| "Chrome extension not connected" | Extension not installed, or native messaging host not found |
| "Browser extension reconnecting" | Transient disconnect, will auto-recover in ~2s |
| "Tool 'X' timed out after 30s" | Relay not running, WS not connected, or extension service worker suspended |
| "Cannot access contents of url" | Trying to access chrome:// or extension pages |
| "Element ref_N not found" | Page changed since last read_page (WeakRef was GC'd) |
| Badge shows empty (disconnected) | Nebo server not running, or manifest path wrong |
| Extension connects then immediately disconnects | Manifest path points to wrong binary, or binary crashes |

---

## File Index

| File | Lines | Description |
|------|-------|-------------|
| `browser/src/lib.rs` | 92 | Crate root — re-exports, BrowserError enum (10 variants), ElementRef, ConsoleMessage, PageError, Cookie structs |
| `browser/src/config.rs` | 106 | BrowserConfig, ProfileConfig, ResolvedProfile — profile detection, defaults, resolution |
| `browser/src/chrome.rs` | 208 | Chrome binary detection (macOS/Linux/Windows), RunningChrome launch, CDP port wait, process lifecycle |
| `browser/src/session.rs` | 185 | Session (multi-page, active target) and Page (state, refs, console, errors) |
| `browser/src/extension_bridge.rs` | 261 | Multi-browser bridge — per-browser channels, default browser routing, grace period, 30s timeout |
| `browser/src/executor.rs` | 58 | Thin wrapper — delegates to ExtensionBridge, adds grace period helpers |
| `browser/src/manager.rs` | 193 | Manager — owns ExtensionBridge, managed Chrome profiles, sessions |
| `browser/src/native_host.rs` | 550 | Native messaging host — manifest install/update/validate, stdin/stdout protocol, Windows registry, direct path |
| `browser/src/native_types.rs` | 112 | NativeMessage struct — all message types with convenience constructors |
| `browser/src/actions.rs` | 167 | Action option structs (Navigate, Click, Type, Fill, Select, Hover, Press, Scroll, Wait, Screenshot), INTERACTIVE_ROLES |
| `browser/src/snapshot.rs` | 252 | Accessibility tree annotation — sequential [eN] refs and role-based [B1/T2/L3] IDs |
| `browser/src/snapshot_store.rs` | 170 | TTL in-memory cache for annotated snapshots — put/get/lookup/cleanup, 1-hour TTL |
| `browser/src/storage.rs` | 91 | Web storage helpers — StorageKind, StorageState, JS snippet generators |
| `browser/src/audit.rs` | 22 | Security audit logging for sensitive tools (evaluate, screenshot) |
| `tools/src/web_tool.rs` | 1,026 | WebTool — 4 resources (http, search, browser, devtools), SSRF protection, search providers |
| `cli/src/main.rs` (~170 lines) | — | Relay bridge — stdin/stdout ↔ WebSocket, browser detection, hello handshake |
| `server/src/handlers/ws.rs` (~95 lines) | — | WS handler for /ws/extension — per-browser connection, tool relay |
| `server/src/lib.rs` (~30 lines) | — | Browser init + manifest install/update |
| `chrome-extension/src/background.ts` | 63 | Service worker — startup, keep-alive, toolbar |
| `chrome-extension/src/native.ts` | 255 | Native messaging — connection, agent tab, tool dispatch |
| `chrome-extension/src/tools.ts` | 455 | Tool implementations — CDP + content scripts |
| `chrome-extension/src/badge.ts` | 32 | Badge state management |
| `chrome-extension/src/types.ts` | 18 | Badge type definitions |
| `chrome-extension/src/options.ts` | 69 | Options page — connection test |
| `chrome-extension/src/content/accessibility-tree.ts` | 340 | Content script — a11y tree generation |
| `chrome-extension/src/content/visual-indicator.ts` | 226 | Content script — glow + stop button |
| `chrome-extension/manifest.json` | 51 | MV3 manifest — permissions, content scripts |
