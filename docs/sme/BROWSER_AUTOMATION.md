# Browser Automation System — Comprehensive SME Reference

Source: `crates/browser/`, `crates/tools/src/web_tool.rs`, `crates/cli/src/main.rs`, `src-tauri/src/main.rs`, sibling repo `/Users/almatuck/workspaces/nebo/chrome-extension/`

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
25. [Headless Fallback](#25-headless-fallback)

---

## 1. Architecture Overview

Extension-backed browser automation uses a **four-hop relay chain**. The agent never talks to Chrome directly in this path — everything flows through the Chrome extension's native messaging bridge. If the extension is unavailable (or fails mid-call with a transport error), `ActionExecutor` fails over per-call to the built-in CDP browser (Obscura) described in [Headless Fallback](#25-headless-fallback).

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
   a. Normalizes URL (adds https:// if no protocol)
   b. Attaches debugger + Page.enable for beforeunload detection
   c. chrome.tabs.update(tabId, {url}) with beforeunload handling
   d. waitForTabLoad(tabId) — waits up to 15s for tab status "complete"
   e. Returns ok("Navigated to ...")
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

**Location:** sibling repo `/Users/almatuck/workspaces/nebo/chrome-extension/` (not checked into this repo)
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
| `tabGroups` | Group per-session agent tabs so multiple agents can browse independently |
| `activeTab` | Fallback tab access |
| `storage` | Extension settings and `chrome.storage.session` persistence for agent tab/group state |
| `alarms` | Keep-alive alarm (every 24s) to prevent service worker suspension |
| `scripting` | Inject content scripts and execute functions in tab context |
| `nativeMessaging` | Connect to `dev.neboai.nebo` native messaging host |
| `<all_urls>` (host) | Access any URL for content script injection and CDP |

### 3.2 Source Files

| File | Purpose |
|------|---------|
| `src/background.ts` | Service worker entry point. Connects native messaging on startup, handles toolbar click, keep-alive alarm, stop-agent messages. |
| `src/native.ts` | Native messaging connection management. Handles `execute_tool`, `execute_batch`, indicators, ping, reconnects, and per-session tab/group lifecycle. |
| `src/tools.ts` | Tool implementations. 25+ tools using CDP and content scripts. |
| `src/badge.ts` | Extension badge UI (ON/OFF/connecting/error states). |
| `src/types.ts` | Badge state types and color config. |
| `src/options.ts` | Options page — connection test (opens native port, sends hello, checks for response). |
| `src/content/accessibility-tree.ts` | Content script injected at `document_start` on all frames. Provides `window.__neboGenerateAccessibilityTree()` for page reading. |
| `src/content/visual-indicator.ts` | Content script injected at `document_idle` on top frame. Pulsing orange glow border + "Stop Nebo" button. |

### 3.3 Agent Tab Lifecycle

The extension maintains dedicated **agent tabs by session** (`agentSessions: Map<session_id, {tabId, groupId}>` in `native.ts`). This lets multiple agents work simultaneously without fighting over a single browser tab.

1. **Session key:** Server forwards `session_id`; extension falls back to `_default` if absent.
2. **Creation:** If the first tool is `navigate`, the extension creates the tab directly at the target URL. Otherwise `ensureAgentTab()` creates a normal new-tab page.
3. **Grouping:** New agent tabs are added to a Chrome tab group named `Nebo` or `Nebo: <suffix>`, with rotating group colors.
4. **Persistence:** `agentSessions` is saved in `chrome.storage.session` and restored on service worker startup if the tab still exists.
5. **Reuse:** Subsequent requests for the same `session_id` reuse the same tab after `chrome.tabs.get()` verifies it still exists.
6. **Tracking:** `chrome.tabs.onRemoved` clears that session if the user closes the tab.
7. **Cleanup:** `hide_indicators` without a session cleans all sessions; with a session it hides indicators and closes every tab in that session's tab group after a short delay.
8. **Visual indicators:** Shown on the session's agent tab only, hidden during tool execution to avoid screenshot noise.

### 3.4 Tool Implementations

| Tool | Method | Mechanism |
|------|--------|-----------|
| `navigate` | `chrome.tabs.update` + `waitForTabLoad` (15s) | Normalizes URL (adds https://), handles beforeunload dialogs (force option), supports `back`/`forward` as URL |
| `read_page` | `chrome.scripting.executeScript` | Calls accessibility tree content script, retries by injecting manually if not loaded |
| `click` | CDP `Input.dispatchMouseEvent` | Domain drift check, resolves ref via WeakRef (or metadata re-query fallback), CDP `DOM.getContentQuads` for coordinates, mouseMoved (100ms pause) + mousePressed/Released (12ms gap). Supports `ref`, `coordinate`, or `x,y` + `modifiers` |
| `double_click` | CDP `Input.dispatchMouseEvent` | Two press/release cycles with incrementing clickCount, 100ms between cycles |
| `triple_click` | CDP `Input.dispatchMouseEvent` | Three press/release cycles (selects entire line/paragraph in a field) |
| `right_click` | CDP `Input.dispatchMouseEvent` | Click with `button: "right"`, `buttons: 2` (context menu) |
| `hover` | CDP `Input.dispatchMouseEvent` | Domain drift check, mouseMoved to element center. Supports ref, coordinate, or x,y |
| `fill` / `form_input` | `chrome.scripting.executeScript` | Domain drift check. Handles: select (option lookup by value/text), checkbox (boolean), radio, date/time, range (numeric), number, text input/textarea (native setter to bypass React/Vue/Angular), contenteditable. WeakRef + metadata fallback for element resolution |
| `type` | CDP `Input.dispatchKeyEvent` | Domain drift check. Character-by-character keyDown/keyUp with macOS command mapping. Falls back to `Input.insertText` for unmapped characters |
| `select` | `chrome.scripting.executeScript` | Sets `<select>.value` + dispatches change event |
| `screenshot` | CDP `Page.captureScreenshot` | Returns base64 JPEG. Adaptive token optimization: probes viewport + DPR, calculates optimal dimensions within Anthropic token budget (28px/token, max 1568 tokens), CDP capture with clip + scale. Oversized images (>1.4M base64 chars) fall back to Canvas-based resize + quality reduction loop (75% → 10% JPEG) |
| `scroll` | CDP `Input.dispatchMouseEvent` (mouseWheel) | Primary: CDP mouseWheel at viewport center or provided coordinate, with 5s race timeout. Fallback: `window.scrollBy()` via content script. Direction: up/down/left/right, amount in ticks (100px per tick, default 3 ticks) |
| `scroll_to` | `chrome.scripting.executeScript` | Element `.scrollIntoView({ behavior: 'instant', block: 'center' })` |
| `press` | CDP `Input.dispatchKeyEvent` | 50+ key mappings (Enter, Tab, Escape, F1-F12, arrow keys, Home/End, etc.). Supports modifier chords (cmd+a, ctrl+c, shift+enter). macOS: sends NSStandardKeyBindingResponding commands for proper renderer handling |
| `drag` | CDP `Input.dispatchMouseEvent` | mousePressed at start → mouseMoved to end → mouseReleased |
| `go_back` | `chrome.tabs.goBack(tabId)` | Attaches debugger for beforeunload detection, handles "Leave site?" dialogs |
| `go_forward` | `chrome.tabs.goForward(tabId)` | Attaches debugger for beforeunload detection, handles "Leave site?" dialogs |
| `wait` | `setTimeout` | Takes `duration` in seconds (or `ms` in milliseconds). Capped at 30s |
| `evaluate` | CDP `Runtime.evaluate` | Domain drift check. Runs JS expression with `returnByValue: true, awaitPromise: true` |
| `new_tab` | `chrome.tabs.create` | Sets the session's tracked agent tab and tab group |
| `close_tab` | `chrome.tabs.remove` | Clears session tracking when closing that session's agent tab |
| `list_tabs` | `chrome.tabs.query({})` | Returns all tabs (no agent tab needed) |
| `zoom` | CDP `Page.captureScreenshot` | Takes `region [x0, y0, x1, y1]` — crops and captures a zoomed screenshot of the specified viewport region. Transforms screenshot-space to viewport-space coordinates |
| `get_page_text` | `chrome.scripting.executeScript` | Extracts page text using semantic selectors (article, main, .content, #content, [role="main"]) for best content. Falls back to body with truncation to `max_chars` (default 50000) |
| `read_console_messages` | CDP `Runtime.enable` | Lazy-enabled: tracking starts on first call. Returns console logs/errors/exceptions. Supports `onlyErrors`, `pattern` (regex), `limit`, `clear` filters. Capped at 1000 messages per tab |
| `read_network_requests` | CDP `Network.enable` | Lazy-enabled: tracking starts on first call. Returns requests with URL, method, status. Supports `urlPattern`, `limit`, `clear` filters. Capped at 1000 requests per tab. Domain-scoped (resets on navigation) |
| `resize_window` | `chrome.windows.update` | Sets window dimensions. Max 7680x4320 (8K limit) |
| `file_upload` | CDP `DOM.setFileInputFiles` | Domain drift check. Finds file input via ref, marks with data attribute, resolves via CDP DOM query, sets files. Takes `paths` array and `ref` |
| `find` | `chrome.scripting.executeScript` | Generates full accessibility tree, searches for text matches across element lines. Returns matching lines with ref IDs for subsequent actions |

**Devtools tools** (mapped by Rust `web_tool.rs`, sent as `devtools_*` tool names): `devtools_console`, `devtools_source`, `devtools_storage`, `devtools_dom`, `devtools_cookies`, `devtools_performance`. **Note:** These tool names are NOT implemented in the extension's `executeTool` switch — the extension has `read_console_messages` and `read_network_requests` instead. Calling devtools actions via the web tool currently returns "Unknown tool" from the extension. This is a known mapping gap.

`execute_batch` runs a sequence of tool actions on the session tab and returns a `batch_response` array.

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
    /// Per-session cache for read_page results (2.5s TTL), keyed by
    /// session_id (or "_default") so concurrent sub-agents don't share pages.
    page_cache: Arc<Mutex<HashMap<String, PageCacheEntry>>>,
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
Removes the connection. If last connection drops, updates `last_connected` but does NOT reject pending requests — individual 60s timeouts handle truly dead connections. This prevents false rejections during the ~2s reconnect window.

**`is_connected() -> bool`**
Non-blocking check via `try_lock()` on connections map. If locked, assumes connected.

**`was_recently_connected(within: Duration) -> bool`**
Returns true if connected OR if `last_connected` is within the given duration. Used for the 3-second grace period in WebTool.

**`wait_for_connection(timeout: Duration) -> bool`**
Polls `is_connected()` every 100ms up to the timeout. Used by WebTool to wait for reconnection.

**`execute(tool, args, session_id) -> Result<Value, String>`**
1. Return this session's cached `read_page` if it is younger than 2.5s
2. Invalidate this session's cache entry before mutation tools (`click`, `navigate`, `evaluate`, tab actions, etc.)
3. Lock connections, find one matching default browser (or fall back to any)
4. Clone the target connection's `tx` channel
5. Assign request ID, create oneshot channel, store in pending
6. Send `ToolRequest` via the connection's channel, including optional `session_id`
7. Wait with **60s timeout** on oneshot receiver
8. On timeout: remove from pending, return error with pending count
9. Cache successful `read_page` results and return the result

**`batch_execute(actions, opts, session_id) -> Result<Vec<Result<Value, String>>, String>`**
Sends `execute_batch` to the extension with an action list and `stop_on_error`. Any mutation in the batch invalidates the `read_page` cache. A successful `read_page` inside the batch refreshes the cache.

**`deliver_result(id, result)`**
Looks up oneshot sender in pending map, sends result. Called by WS handler.

### 6.3 ToolRequest Struct

```rust
pub struct ToolRequest {
    pub id: i64,
    pub tool: String,
    pub args: serde_json::Value,
    pub is_batch: bool,
    pub session_id: Option<String>,
}
```

---

## 7. ActionExecutor

**File:** `crates/browser/src/executor.rs` (249 lines)

ONE entry point with an ordered backend fallback: tier 1 = the user's Chrome extension (authenticated, human session), tier 2 = the built-in Rust Chrome (Obscura) driven over CDP via `CdpBridge`. Tier 3 (direct HTTP) lives in the caller (`web_tool`).

```rust
pub struct ActionExecutor {
    bridge: Arc<ExtensionBridge>,
    cdp: Option<Arc<CdpBridge>>,
}
```

**Failover is per-call and transport-gated:** if the extension errors, `is_transport_failure()` decides whether to retry on CDP. Only disconnect/timeout/"not connected"/"native host"/"no browser" errors fail over — tool-level errors (element not found, page too big, frame error) mean the extension is working and are surfaced to the agent instead of spawning a blank CDP page.

- `is_connected()`: true if extension is connected OR the CDP backend exists
- `extension_connected()`: true only for the Chrome extension path (tier 1)
- `cdp_available()`: true if the built-in CDP backend exists (tier 2)
- `was_recently_connected(within)`: true if extension recently connected or CDP exists
- `wait_for_connection(timeout)`: returns true immediately if CDP exists, else waits for the extension
- `execute(tool, args, session_id)`: extension first; on transport failure retries the same action on CDP; `ExtensionNotConnected` if neither backend exists
- `batch_execute(actions, opts, session_id)`: extension batches in one round-trip; CDP runs the batch sequentially (`cdp_batch`, honoring `stop_on_error`)
- `click_and_read`, `fill_and_read`, `navigate_and_read`: convenience batches for common flows
- `send_command(command, session_id)`: fire-and-forget to the extension (indicators)
- `close_session(session_id)`: canonical cleanup for a finished sub-agent — sends `hide_indicators` + `close_session_tabs` to the extension AND closes the CDP session page; each side is a no-op if it never served the session

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

**File:** `crates/tools/src/web_tool.rs` (~2,700 lines)

The `web` domain tool handles four resource types: `http`, `search`, `browser`, `devtools`. The `resource` param is primary (auto-corrected via `auto_correct_resource`); when absent, `infer_resource(action)` derives it.

### 16.1 Resource Routing

```
action → infer_resource():
├─ fetch, sanitize → "http"
├─ search → "search"
├─ navigate, read_page, click, fill, type, screenshot, evaluate,
│  list_tabs, new_tab, close_tab, history, scroll, hover, select,
│  press, wait, drag, status, read_console_messages,
│  read_network_requests, resize_window, file_upload, find,
│  fill_form, browser_batch → "browser"
└─ console → "devtools"
```

There are NO aliases (no `get`/`post`/`snapshot`/`back`/`key` etc.) — one canonical action name per operation. `back`/`forward` are `history` with `direction`; click variants are `click` with `click_count`/`button` params.

### 16.2 Browser Action Routing

```
web(resource: "browser", action: "navigate", url: "...")
  → handle_browser(input)
    → Check manager exists (Option<Arc<Manager>>)
    → executor = manager.executor() — single source of truth for backend state
    → "status" works even disconnected: reports both Extension and
      Built-in Chrome (CDP) availability from the executor
    → If extension not connected: broadcast browser_extension_disconnected
      (install nudge) — even when CDP is carrying the work
    → If executor.is_connected() false:
        → was_recently_connected(3s)? → wait_for_connection(3s)
          → still down: "Browser extension reconnecting — try again in a moment."
        → else: "No browser backend available. Connect the Nebo Chrome/Brave extension."
    → navigate pre-checks:
        → binary-file URLs (.pdf/.docx/.zip/…) are SKIPPED (would only trigger
          a download + OS save dialog); agent is told to find an HTML page
        → visited-page cache: a URL navigated within the last 5 min (by this
          session OR a sibling subagent in the same group) returns the cached
          page; `fresh: true` bypasses the cache for a deliberate reload
    → handle_browser_via_extension(executor, action, input, session_id)
```

**Cross-subagent caches:** `visited_pages` (group-keyed by parent session; navigate results + search results, 5-min TTL, expired entries evicted on insert) and `nav_history` (per-session origin visit counts with last-visit timestamps; sessions idle >1h pruned on insert). Navigating the same origin ≥3 times appends a loop-detection warning telling the agent to change approach.

### 16.3 Action Name Mapping

`map_action_to_tool()` accepts canonical model actions ONLY — variants are resolved from params by the caller, never as aliases:

| Web Tool Action | Extension Tool | Resolution |
|----------------|---------------|------------|
| `read_page` | `read_page` | |
| `navigate` | `navigate` | |
| `click` | `click` / `double_click` / `triple_click` / `right_click` | from `click_count` (2/3) and `button: "right"` |
| `hover` | `hover` | |
| `fill` | `form_input` | |
| `type` | `type` | |
| `select` | `select` | |
| `screenshot` | `screenshot` | |
| `scroll` | `scroll` / `scroll_to` | `scroll_to` when `ref` present and no `direction` |
| `press` | `press` | |
| `drag` | `drag` | |
| `history` | `go_back` / `go_forward` | from `direction` (default back) |
| `wait` | `wait` | |
| `evaluate` | `evaluate` | |
| `list_tabs` / `new_tab` / `close_tab` | same | `new_tab` requires a non-empty URL |
| `read_console_messages` / `read_network_requests` / `resize_window` | same | |
| `file_upload` | `file_upload` | |
| `find` | `find` | |
| `fill_form` | (expands to a batch) | per text field: `click` → `press(SELECT_ALL_KEY)` → `type`; non-string values use `form_input`. `SELECT_ALL_KEY` is `cmd+a` on macOS, `ctrl+a` elsewhere (compile-time cfg) |
| `browser_batch` | (multiple, one round trip) | each item mapped via `map_action_to_tool`; stops on first error |

### 16.4 Argument Forwarding

`build_extension_args()` selectively forwards parameters per action:

| Action | Forwarded Keys |
|--------|---------------|
| `navigate` | `url`, `force` |
| `new_tab` | `url` |
| `click` | `ref`, `selector`, `coordinate`, `modifiers`, `click_count`, `button` |
| `hover` | `ref`, `coordinate` |
| `fill` | `ref`, `selector`, `value` |
| `type` | `text` |
| `select` | `ref`, `selector`, `value` |
| `scroll` | `direction`, `amount`, `coordinate`, `ref` |
| `press` | `key`, `text`, `repeat` |
| `drag` | `start_coordinate`, `coordinate` |
| `wait` | `ms` |
| `evaluate` | `expression`, `text` |
| `read_page` | `filter`, `depth`, `maxChars`, `refId` |
| `close_tab` | `tabId`, `tabIds` |
| `read_console_messages` | `onlyErrors`, `clear`, `pattern`, `limit` |
| `read_network_requests` | `urlPattern`, `clear`, `limit` |
| `resize_window` | `width`, `height` |
| `file_upload` | `paths`, `ref` |
| `find` | `query` |

Note `fresh` (navigate cache bypass) is consumed by the Rust side and never forwarded.

### 16.5 DevTools Resource

| Action | Extension Tool | Status |
|--------|---------------|--------|
| `console` | `read_console_messages` | Working. `filter` param translated to `pattern`; also forwards `onlyErrors`, `clear`, `limit` |
| anything else | — | "Unknown devtools action '<a>'. Available: console" |

(Network inspection is a browser-resource action: `read_network_requests`.)

### 16.6 HTTP Resource

- Methods: GET, POST, PUT, DELETE, HEAD, PATCH (one `method` param)
- Custom headers and body support
- `sanitize` action: extracts visible text, chunks for LLM (default 4000 chars, configurable via `chunk_size`, paged by `offset`)
- **SSRF guard (`check_url_allowed`)**: proper URL parse (`url` crate — canonicalizes hex/decimal/short IPv4 forms), http/https schemes only, IP literals classified by `is_public_ip` (loopback, RFC1918, link-local, CGNAT 100.64/10, 192.0.0.0/24, ≥240/4, unspecified/broadcast; IPv6 loopback/unspecified/unique-local/link-local/multicast; v4-mapped unwrapped via `to_canonical`), hostnames DNS-resolved and rejected if ANY address is non-public, fails closed on parse/resolve errors
- **Redirects followed manually** (`fetch_checked`, ≤5 hops via a non-redirecting `bare_client`): the full SSRF check (including DNS) re-runs on EVERY hop; reqwest-matching semantics (303→GET, 301/302 POST→GET, 307/308 preserve; credentials stripped on cross-host hops; body headers dropped with the body). Accepted residual risk: DNS-rebinding TOCTOU (single-user desktop threat model — documented in code)
- HTML responses return sanitized visible text; large results spill: first 15K chars inline + full text saved to `<data>/tmp/web/*.txt` for paging via `os(read)` — never a silent cut
- Non-HTML bodies >50KB shown raw in 20KB chunks with `offset` pagination

### 16.7 Search Resource

Fallback chain (first non-empty result wins):
1. **BYOK API providers** from `auth_profiles`, in order: `search-brave`, `search-tavily`, `search-google` (needs `cx` in metadata), `search-serpapi`
2. **Browser-based search** when a backend is connected — human flow on BOTH tiers (extension AND Obscura/CDP):
   - **Extension tier** (`search_via_browser_human`): navigate to the `search.brave.com` HOMEPAGE, find the search box on the interactive ref snapshot, then click → type → Enter in one batch.
   - **CDP/Obscura tier** (`search_via_cdp_human`): same homepage → click → type → Enter, but the headless tier has no ref surface so the search box is located by CSS selector; the CDP tier's humanized `click` resolves it to a center coordinate. **This closes the gap that got Obscura bot-flagged** — the headless path no longer constructs `?q=` URLs.
   - Both tiers synthesize curved mouse paths, human click durations (50–110ms hold), and irregular typing cadence (35–110ms). Neither constructs a `?q=` results URL — that's the automation signature that flags IPs.
   - **URL flow** remains only as the last-resort fallback when the human flow misses (navigate `search.brave.com/search?q=…` directly — NOT DDG, whose html endpoint bot-blocks real browsers too).
   All paths require ≥2 extracted links — 0–1 means a block/consent/still-loading page, which falls through instead of returning junk.
3. **DuckDuckGo HTML scraping** — 8s per-request budget (fail-fast; a blocked engine hangs rather than failing)
4. **Brave HTML scraping** — own 8s budget, chained when DDG times out, errors, or yields nothing. The floor of the chain; its failure message is honest about all engines being unreachable

Supporting behavior:
- `normalize_search_query()`: strips stacked `site:` filters (2+ = spam pattern) and clamps to 400 chars at a word boundary — keyword engines reject long operator-stuffed queries
- `extract_search_links()`: ONE generic results extractor for all engines (harvests external `<a href>` + anchor text, decodes DDG `uddg=` redirects, drops engine/social/nav hosts, dedups by normalized URL) — no per-engine selectors to rot
- Search results are cached in the cross-subagent `visited_pages` cache (key `search:<query>`, 5-min TTL) so siblings and repeat queries don't re-search

### 16.8 Resource Permissions

- `browser` and `devtools`: `ResourceKind::Browser` (exclusive lock during tool execution)
- `http` and `search`: No resource lock (parallelizable)
- `requires_approval`: true for all WebTool calls

### 16.9 Agent Ergonomics (post-action behavior)

- **Auto-snapshot**: every mutation action (navigate, click, type, fill, select, press, scroll, drag, hover, file_upload, history) appends a compact interactive-elements page snapshot (≤6,000 chars) — the model never needs a follow-up read_page
- **Auto-screenshot** after navigate (attached as image)
- **read_page char-limit retry ladder**: on "character limit" errors retries with depth 5 → depth 3 → filter interactive (maxChars 50000)
- **Large reads spill**: read_page/evaluate results >15K chars return a preview + full text written to a file
- **Auth-page detection** (`detect_auth_page`, two-signal threshold): warns the agent not to attempt logins
- **Error-page detection** (`detect_error_page`): flags 404/5xx pages and suggests search instead
- **Friendly errors** (`friendly_browser_error`): maps raw browser errors to recovery suggestions

### 16.10 Error Messages

| Condition | Error |
|-----------|-------|
| No browser manager | "Browser automation is not available. Use web(action: \"fetch\"...)" |
| Backend down, recently connected | "Browser extension reconnecting — try again in a moment." |
| No backend at all | "No browser backend available. Connect the Nebo Chrome/Brave extension." |
| Unsupported action | Lists all available actions |
| new_tab without URL | "new_tab requires a URL. Use navigate to change the current tab..." |

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

Written as `dev.neboai.nebo.json`:

```json
{
  "name": "dev.neboai.nebo",
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
- `HKCU\Software\Google\Chrome\NativeMessagingHosts\dev.neboai.nebo`
- `HKCU\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\dev.neboai.nebo`
- `HKCU\Software\Microsoft\Edge\NativeMessagingHosts\dev.neboai.nebo`

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

### 18.3 Post-Action Screenshots

Interaction tools (`click`, `double_click`, `triple_click`, `right_click`, `form_input`, `fill`, `type`, `select`, `scroll`, `scroll_to`, `press`, `drag`, `hover`) automatically capture a JPEG screenshot after execution for sidecar vision verification. The screenshot is raced against a 3-second timeout — if the page is re-rendering (e.g., Google Maps after click), the screenshot is silently dropped rather than blocking the tool response.

The result is attached as `{ text: "...", screenshot: { data, format } }` alongside the tool's text output.

### 18.4 Domain Drift Security

Tools that interact with page content (`click`, `hover`, `form_input`, `type`, `press`, `scroll_to`, `drag`, `evaluate`, `file_upload`) perform a domain drift check before execution: the tab's current origin is compared against the origin recorded at the last successful `read_page`/`find` (`snapshotOrigins` per tab) — i.e. the page the agent actually saw when its refs were issued. On mismatch the tool returns a security error telling the agent to `read_page` first. Agent-initiated transitions (`navigate`, `go_back`, `go_forward`, `new_tab`) clear the recorded origin until the next read. (Until 2026-06-11 this check fetched the tab URL immediately before comparing — a no-op.)

### 18.5 Element Reference Resolution

Refs are **stable**: a reverse `WeakMap` (`window.__neboElementToRef`) gives an element the same `ref_N` across tree rebuilds, the counter never resets, and live entries survive rebuilds (dead WeakRefs are pruned). A ref from an older `read_page` stays valid while its element lives — auto-snapshot rebuilds can no longer silently renumber.

The extension uses a two-tier element resolution system:

1. **Primary: WeakRef map** — `window.__neboElementMap[ref_N]` stores `WeakRef<Element>`. Fast O(1) lookup, but refs die when elements are garbage collected (common in SPAs).
2. **Fallback: Metadata re-query** — `window.__neboElementMeta[ref_N]` stores `{ tag, role, name, href?, type? }`. When WeakRef is dead, re-queries DOM by tag + filters by role/name/href/type. Only used if exactly one match is found (prevents ambiguous clicks).
3. **Auto-re-snapshot** — If both fail, `resolveRef()` regenerates the accessibility tree and retries once before returning "not found".
4. **CDP coordinates** — After finding the element, `DOM.getContentQuads` (like Playwright's `_clickablePoint`) is used for accurate coordinates that account for CSS transforms, iframes, and viewport clipping. Falls back to `getBoundingClientRect` center if CDP quads fail.

### 18.6 Beforeunload Dialog Handling

Navigation tools (`navigate`, `go_back`, `go_forward`) use `withBeforeunloadHandling()`:

1. Set policy (`accept` if `force: true`, otherwise `dismiss`) BEFORE running the action
2. CDP `Page.javascriptDialogOpening` event handler reads the policy and responds
3. Wait 300ms for potential dialog to fire
4. Three outcomes:
   - `none` — no dialog appeared, navigation succeeded
   - `accepted` — dialog appeared, was accepted (user loses unsaved changes), result includes warning suffix
   - `blocked` — dialog appeared, was dismissed (page stays, user told to address unsaved state or retry with `force: true`)

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
| `execute_batch` | host → ext | `id`, `actions`, `stop_on_error`, optional `session_id` | Request sequential execution of multiple tools in one extension round-trip |
| `tool_response` | ext → host | `id`, `result` OR `error` | Tool execution result |
| `batch_response` | ext → host | `id`, `result[]` OR `error` | Batch execution result array |
| `show_indicators` | host → ext | optional `session_id` | Show visual indicators on that session's agent tab |
| `hide_indicators` | host → ext | optional `session_id` | Hide indicators and close that session's tab group; no session closes all agent sessions |
| `tab_attached` | ext → host | `args` | Tab debugger attached (informational) |
| `tab_detached` | ext → host | `args` | Tab debugger detached (informational) |
| `stop_agent` | ext → host | (none) | User clicked "Stop Nebo" button |

---

## 20. Timeouts and Error Handling

### 20.1 Timeout Locations

| Component | Timeout | Location |
|-----------|---------|----------|
| ExtensionBridge.execute() / batch_execute() | **60 seconds** | `extension_bridge.rs` |
| NativeHost.execute_tool() | **30 seconds** | `native_host.rs` |
| Reconnect grace period | **3 seconds** | `web_tool.rs` |
| Reconnect polling interval | **100ms** | `extension_bridge.rs` |
| Relay WS connection backoff | 500ms → 5s (10 attempts) | `cli/main.rs`, `src-tauri/main.rs` |
| Extension reconnect | 2 seconds | sibling `chrome-extension/src/native.ts` |
| Extension keep-alive alarm | 24 seconds | sibling `chrome-extension/src/background.ts` |
| `read_page` cache TTL (per session) | 2.5 seconds | `extension_bridge.rs` |
| CDP new_page / navigate / evaluate | 30s / 45s / 20s | `cdp_bridge.rs` |
| Debugger attach timeout | **8 seconds** | sibling `chrome-extension/src/tools.ts` (`DEBUGGER_ATTACH_TIMEOUT`) |
| Content-script execution timeout | 45 seconds | sibling `chrome-extension/src/tools.ts` (`EXECUTE_SCRIPT_TIMEOUT`) |
| CDP command timeout | 30 seconds | sibling `chrome-extension/src/tools.ts` (`CDP_COMMAND_TIMEOUT`) |
| Post-action screenshot capture | 3 seconds | sibling `chrome-extension/src/tools.ts` (`capturePostActionScreenshot` race) |
| Scroll CDP mouseWheel race | 5 seconds | sibling `chrome-extension/src/tools.ts` |
| Page load wait helper | 15 seconds | sibling `chrome-extension/src/tools.ts` (`waitForTabLoad`) |
| CDP port wait (Chrome launch) | 15 seconds (200ms poll) | `chrome.rs` |
| HTTP client timeout | 30 seconds | `web_tool.rs` |
| Search scrape budget (DDG, Brave — each) | 8 seconds | `web_tool.rs` (`SCRAPE_TIMEOUT`) |
| Options page connection test | 3 seconds | sibling `chrome-extension/src/options.ts` |
| Indicator hide animation | ~350ms | sibling `chrome-extension/src/content/visual-indicator.ts` |
| Agent tab/group cleanup delay | ~200ms | sibling `chrome-extension/src/native.ts` |
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
4. Individual tool 60s timeouts handle truly dead connections
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
   c. chrome.runtime.connectNative("dev.neboai.nebo")
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

### 21.3 Run Lifecycle Integration

The browser tab lifecycle is wired to the agent run lifecycle:

1. **Run start (browser tool use):** `web_tool.rs` sends `show_indicators` with `session_id` before dispatching browser/search/devtools actions. Idempotent — safe on every call.
2. **Run completion:** `chat_dispatch.rs` sends `hide_indicators` with `session_id` before dropping the RunHandle. Covers success, error, and cancellation — all paths converge here.
3. **Stale run cleanup:** The periodic stale-run cleanup task (every 60s, 600s idle threshold) sends `hide_indicators` for each expired session key.

The `ExtensionBridge.send_command()` method broadcasts fire-and-forget commands to all active extension connections. The WS handler serializes these as `{ "type": "<command>", "session_id": "..." }` — exactly the format the extension's `native.ts` expects for `show_indicators` and `hide_indicators`.

### 21.4 Server Restart Recovery

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

### 23.1 Page Load Timing on Complex Pages

**Status: largely fixed extension-side (verified 2026-06-11).** `navigate` waits `waitForTabLoad` (status `complete`, 15s cap) **plus** `waitForDomStable` (MutationObserver, 500ms quiet / 3s cap); departures from restricted origins (chrome://, chrome-extension://) use `waitForNavigationCycle` (loading→complete) to dodge the stale-`complete` race (`tools.ts:148–213, 1386–1404`). Mutation actions additionally get `waitForNetworkIdle` (fetch/XHR monkey-patch, 500ms quiet / 5s cap) + DOM-stable before their post-action snapshot. The residual stale-`complete` race lives ONLY in the `new_tab` session-reuse path (`native.ts:286–294`), which skips all of this (see 23.10). Truly async late renders can still slip through — agents should `wait` + re-read when content looks partial.

### 23.2 Manifest Path Staleness

**Status: Fixed.** The `needs_manifest_update()` function now compares the manifest `path` field against the current binary path on startup and reinstalls if stale. Also validates `allowed_origins` and Windows registry entries.

### 23.3 Extension Disconnection During Tool Execution

If the extension or relay disconnects mid-tool-execution, the pending oneshot channel times out after 60s. The tool may have partially executed (e.g., navigation started but result never returned). The bridge no longer immediately rejects pending requests on disconnect — it lets timeouts handle truly dead connections.

### 23.4 Agent Tab Creation and Empty Pages

The current extension avoids creating `about:blank` agent tabs because extensions cannot reliably access them. If the first action is `navigate`, it creates the session tab directly at the requested URL. Other first actions create a normal new-tab page, which may still produce little useful content until navigation happens.

### 23.5 CDP Debugger Permission Dialog

When the extension first attaches the debugger to a tab, Chrome shows a yellow "debugging this browser" infobar. This can confuse users and may persist if the debugger isn't properly detached.

### 23.6 Content Script Injection Race

The accessibility tree content script is injected at `document_start`, but for newly created tabs (via `chrome.tabs.create`), there's a race — `ensureAgentTab()` creates the tab and immediately proceeds. The content script may not be loaded yet when `read_page` is called. The tool handles this by retrying with manual injection, but this adds latency.

### 23.7 "Cannot access contents of url" Error

This is Chrome's error when trying to execute scripts on restricted pages: `chrome://`, `chrome-extension://`, `about:blank` (sometimes), and URLs blocked by CSP. The extension catches this and returns it as a tool error.

### 23.8 Default Browser Detection Limited to macOS

The `detect_default_browser()` function only works on macOS (reads LSHandlers). On Linux and Windows, it falls back to "unknown", which means tool requests go to any available connection rather than the user's default browser. Not a problem when only one browser runs the extension.

### 23.9 Extension Bugs Found in the 2026-06-11 Audit (extension repo — open)

Verified against `~/workspaces/nebo/chrome-extension` source; host-side halves are fixed in this repo (see 23.11):

| # | Bug | Where | Severity |
|---|-----|-------|----------|
| 1 | ~~`evaluate` returns a BARE JSON string instead of `{text}`~~ **FIXED 2026-06-11** — now wraps in `{text}` (host tolerates both shapes for old extension builds) | `tools.ts` evaluate | critical (was: broke browser search) |
| 2 | ~~`capturePostActionScreenshot` has no size cap~~ **FIXED 2026-06-11** — oversized captures dropped at `MAX_BASE64_CHARS`; relay bound also raised host-side | `tools.ts` | major |
| 3 | ~~Stale element refs silently REBIND to wrong elements~~ **FIXED 2026-06-11** — refs are now STABLE: reverse `WeakMap` (`__neboElementToRef`) keeps the same number per element across rebuilds; counter never resets; dead WeakRefs pruned per rebuild. `resolveRef`'s re-snapshot retry is now sound | `accessibility-tree.ts` | major |
| 4 | ~~Domain-drift security check is a no-op~~ **FIXED 2026-06-11** — origin is recorded per tab at `read_page`/`find` success (`snapshotOrigins`) and compared at action time; agent-initiated transitions (navigate/back/forward/new_tab) clear it until the next read_page | `tools.ts` | major |
| 5 | ~~`getClickablePointViaCDP` reads the map from the wrong JS world~~ **FIXED 2026-06-11** — marks the element with a unique attribute from the isolated world (the DOM is shared), queries it from the main world for the objectId, `DOM.getContentQuads` path now actually runs; marker cleaned up in `finally` | `tools.ts` | major |
| 6 | ~~Post-action screenshots include the agent glow/Stop-button overlay~~ **FIXED 2026-06-11** — inner `SHOW_AFTER_TOOL_USE` removed from `clickTool`; the `handleToolRequest`/`handleBatchRequest` finally restores indicators after the screenshot | `tools.ts` | major |
| 7 | ~~Hi-DPI coordinate clicks from post-action screenshots land wrong~~ **FIXED 2026-06-11** — `capturePostActionScreenshot` now uses the same probe → `calcOptimalDimensions` → clip/scale capture pipeline as the explicit screenshot tool AND updates `screenshotContexts`, so `transformCoords` always has the matching context | `tools.ts` | major |
| 8 | ~~`press` false success~~ **FIXED 2026-06-11** — unmapped multi-char keys and chords with unmapped mains now return errors (single literal characters still insert as text) | `tools.ts` | major |
| 9 | ~~`new_tab` session-reuse path skips navigate's protections~~ **FIXED 2026-06-11** — the reuse path now routes through the `navigate` tool implementation (URL normalization, beforeunload handling, navigation-cycle wait). This was the race behind the live junk-result incident | `native.ts` | major |
| 10 | ~~Options-page relay hijacks routing~~ **MITIGATED host-side 2026-06-11** — `extension_bridge` now routes to the OLDEST matching connection (lowest id) in both `execute` and `batch_execute`, so a transient second relay can never steal requests | `extension_bridge.rs` | minor |
| 11 | ~~`MAC_COMMANDS` chord lookup order-sensitive; no cmd→ctrl on Win/Linux~~ **FIXED 2026-06-11** — `getMacCommands` canonicalizes aliases + modifier order; `pressKeyChord` maps cmd/command/meta/win → ctrl on non-Mac platforms (models freelance "cmd+a" everywhere; Meta+A is a no-op outside macOS) | `tools.ts` | minor |
| 12 | ~~Service-worker restarts lose module state~~ **FIXED 2026-06-11** — `neboGroupId` and `screenshotContexts` persist to `chrome.storage.session` and restore on SW startup (group existence verified before reuse). Console/network buffers intentionally NOT persisted: transient diagnostics, not worth per-event write amplification | `native.ts`, `tools.ts` | minor |
| 13 | ~~Network log wiped by cross-origin subframes; `loadingFailed` fabricates 503~~ **FIXED 2026-06-11** — bucket resets only on main-frame `Page.frameNavigated`; failures record `error: <errorText>` and render as `failed (<reason>)` instead of a fake status | `tools.ts` | minor |

### 23.10 new_tab Reuse Race

See 23.9 #9 — the `chrome.tabs.update(url)` + `waitForTabLoad` reuse path resolves instantly on the OLD page's `complete` status when the session tab sits on `blank.html`. Fix is to route the reuse path through the `navigate` tool implementation.

### 23.11 Host-Side Fixes Landed 2026-06-11

- `search_via_browser` accepts the extension's bare-string `evaluate` result (`v.as_str()` fallback) AND the CDP backend's `{text}` shape — browser-tier search now functional on both tiers.
- `CdpBridge` gained `evaluate` support (was navigate + read_page only — tier-2 browser search could never work).
- Search fallback chain consolidated in `handle_search`: `search_via_browser` returns errors instead of running its own uncapped scrape chain; DDG and Brave scrape requests each get an 8s budget (a hanging DDG no longer eats Brave's chance).
- Screenshot results route to `image_url` via `extract_screenshot_b64` (nested mutation shape AND flat screenshot-tool shape) — previously the explicit `screenshot` action pretty-printed up to 1.4 MB of base64 into model text with no image, and navigate's auto-screenshot never attached.
- Relay extension→server size bound raised 1 MB → 64 MB (Chrome's 1 MB cap is host→extension only; screenshots and outerHTML evaluate results legitimately exceed 1 MB — overflow used to kill the bridge and time out every pending tool).
- Navigate loop-warning text now advertises `wait(ms: 3000)` (matches the schema; `duration` was never forwarded).
- **Browser-tier search engine switched DDG → Brave** (`search_via_browser` navigates `search.brave.com`): live verification showed html.duckduckgo.com serves its bot-block page even to real browsers (extension AND built-in), so every browser-tier search burned 20–45s for nothing before falling back. Brave returns real results even from flagged IPs.
- Extension fixes landed in the sibling repo (`tools.ts`, built to `dist/`, **requires reloading the unpacked extension in Chrome**): evaluate wraps in `{text}`, post-action screenshots capped at `MAX_BASE64_CHARS`, inner indicator-restore removed so screenshots no longer show the agent glow.
- **Human input synthesis in the extension** (`tools.ts`): clicks travel a curved, eased, jittered mouse path from the last pointer position (`humanMouseMove`, 4–18 steps) with 50–110ms press durations (was: teleport + 12ms click); typing has irregular 35–110ms inter-key cadence (compressed 4× for >200-char texts). These timing patterns are primary bot-detection signals — zero-delta keystrokes and mouse teleports are what got the IP flagged.
- **Human-flow search** (`search_via_browser_human`): extension-tier search now lands on the Brave homepage and clicks/types/Enters like a person instead of jumping to a `?q=` results URL; the URL flow remains for the CDP tier and as fallback. Verified live 2026-06-11 (navigate→read_page→batch→wait→evaluate in the log, real results returned).
- **Round 2 (2026-06-11, later)**: `hover` and `drag` routed through the human-input layer (curved approach, eased drag steps, human press timing); `press` errors on unmapped keys instead of false success; `getMacCommands` chord lookup canonicalized; stable element refs + real domain-drift check (see §18.4/§18.5); host `extension_bridge` routes to the oldest matching connection in `execute` AND `batch_execute`.
- **Round 3 (2026-06-11, later still)**: `new_tab` reuse routes through `navigate`; `getClickablePointViaCDP` fixed via the marker-attribute world bridge (Playwright-accurate quads now actually used for ref clicks); post-action screenshots use the explicit-screenshot clip/scale pipeline + update `screenshotContexts` (Hi-DPI coordinate fidelity); `pressKeyChord` maps cmd→ctrl on non-Mac.
- **Round 4 (2026-06-11, final)**: SW-restart persistence (`neboGroupId` + `screenshotContexts` via `chrome.storage.session`); network log resets only on main-frame navigation and reports failures honestly; stale `badge.showConnected(arg)` call fixed. **All 13 findings from the 2026-06-11 audit are now closed.**

### 23.12 DevTools Tool Coverage

**Status: Partially fixed.** `web(action: "console")` now correctly maps to the extension's `read_console_messages` tool (previously mapped to nonexistent `devtools_console`). The `filter` param is translated to `pattern` for backward compatibility. Devtools actions `source`, `storage`, `dom`, `cookies`, and `performance` have no extension implementation yet — the Rust side now returns a clear "not yet available" error instead of the previous "Unknown tool" from the extension.

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
cat ~/Library/Application\ Support/Google/Chrome/NativeMessagingHosts/dev.neboai.nebo.json

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
| "Tool 'X' timed out after 60s" | Relay not running, WS not connected, extension service worker suspended, or extension-side tool hung |
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
| `browser/src/extension_bridge.rs` | 534 | Multi-browser bridge — per-browser channels, default browser routing, session IDs, batch execution, per-session read_page cache, grace period, 60s timeout |
| `browser/src/executor.rs` | 249 | Routes to ExtensionBridge first; per-call transport-failure failover to CdpBridge (tier 2) |
| `browser/src/cdp_bridge.rs` | current | Tier-2 built-in browser — launches bundled Obscura over CDP (chromiumoxide), one page per session, lazy + self-healing |
| `browser/src/manager.rs` | 193 | Manager — owns ExtensionBridge, managed Chrome profiles, sessions |
| `browser/src/native_host.rs` | 550 | Native messaging host — manifest install/update/validate, stdin/stdout protocol, Windows registry, direct path |
| `browser/src/native_types.rs` | 112 | NativeMessage struct — all message types with convenience constructors |
| `browser/src/actions.rs` | 167 | Action option structs (Navigate, Click, Type, Fill, Select, Hover, Press, Scroll, Wait, Screenshot), INTERACTIVE_ROLES |
| `browser/src/snapshot.rs` | 252 | Accessibility tree annotation — sequential [eN] refs and role-based [B1/T2/L3] IDs |
| `browser/src/snapshot_store.rs` | 170 | TTL in-memory cache for annotated snapshots — put/get/lookup/cleanup, 1-hour TTL |
| `browser/src/storage.rs` | 91 | Web storage helpers — StorageKind, StorageState, JS snippet generators |
| `browser/src/audit.rs` | 22 | Security audit logging for sensitive tools (evaluate, screenshot) |
| `tools/src/web_tool.rs` | ~2,700 | WebTool — 4 resources (http, search, browser, devtools), SSRF guard + manual redirect checks, search chain, cross-subagent caches, agent ergonomics |
| `cli/src/main.rs` (~170 lines) | — | Relay bridge — stdin/stdout ↔ WebSocket, browser detection, hello handshake |
| `server/src/handlers/ws.rs` (~95 lines) | — | WS handler for /ws/extension — per-browser connection, tool relay |
| `server/src/lib.rs` (~30 lines) | — | Browser init + manifest install/update |
| sibling `chrome-extension/src/background.ts` | current | Service worker — startup connect, keep-alive, toolbar, stop-agent relay |
| sibling `chrome-extension/src/native.ts` | current | Native messaging — connection, reconnect, session tab groups, batch dispatch, cleanup |
| sibling `chrome-extension/src/tools.ts` | current | Tool implementations — CDP + content scripts |
| sibling `chrome-extension/src/badge.ts` | current | Badge state management |
| sibling `chrome-extension/src/types.ts` | current | Badge type definitions |
| sibling `chrome-extension/src/options.ts` | current | Options page — connection test |
| sibling `chrome-extension/src/content/accessibility-tree.ts` | current | Content script — a11y tree generation |
| sibling `chrome-extension/src/content/visual-indicator.ts` | current | Content script — glow + stop button |
| sibling `chrome-extension/manifest.json` | current | MV3 manifest — permissions, tabGroups, content scripts |

---

## 25. Headless Fallback (Tier 2 — Built-in CDP Browser)

### 25.1 Current Implementation

**File:** `crates/browser/src/cdp_bridge.rs`

Browser automation no longer hard-requires the Nebo Chrome extension. Tier 2 is the bundled **Obscura** headless browser (fork: `localrivet/obscura`, branch `chromiumoxide-cdp-compat` — the bundled binary MUST build from this fork or tier 2 hangs; see memory `obscura-fork-cdp-fixes`). `CdpBridge` launches `obscura serve --stealth` on an ephemeral loopback port and drives it over CDP via `chromiumoxide`.

```
web(action: "click", ref: "ref_1")
    ↓
ExtensionBridge connected?
    YES → Chrome extension path (visible browser, user's session)
        └─ on TRANSPORT failure (disconnect/timeout) → retry same call on CDP
    NO  → CdpBridge if the obscura binary is bundled/configured
    ELSE → "No browser backend available" with install guidance
```

Key properties:
- **Lazy launch**: the Obscura process starts on first use — extension users never pay for it. The process is killed on drop (`kill_on_drop`).
- **One CDP page (tab) per `session_id`** — preserves the 1:1 sub-agent→tab model; `close_session` closes that session's page.
- **Self-healing connection**: a CDP event-loop task flips a liveness flag when the connection dies; a dead core is dropped and relaunched on the next call instead of wedging.
- **Operation timeouts**: `new_page` 30s (recycles the connection on hang), navigation 45s, evaluate 20s — a wedged browser fails fast instead of trapping the tool.
- **ObscuraConfig**: binary path, optional persistent `storage_dir` (cookies/storage; None = ephemeral), `stealth` (anti-detection + tracker blocking), optional `log_path` capturing Obscura's own navigations/CDP errors.
- **Humanized input parity (2026-06-11)**: the CDP tier has full human-input synthesis (`crates/browser/src/human_input.rs`) — a direct port of the extension's `humanMouseMove`/`humanDelay`/click/type primitives onto chromiumoxide. `execute` supports `click` (by CSS `selector` or `coordinate`), `type`, and `press` with curved bezier mouse paths, 50–110ms click holds, and 35–110ms typing cadence, threading the last pointer position per session. Click resolves a selector to its center via JS, moves there along a curve, presses, and explicitly `focus()`es the element (Obscura's synthesized click doesn't move keyboard focus to form fields — a headless quirk; the human motion is still what detection observes). This is what lets `search_via_cdp_human` browse like a person instead of hitting `?q=` URLs. Verified live against the real Obscura binary (`human_click_and_type_fills_input`, run with `--ignored`).

### 25.2 Differences from Extension Path

| Capability | Extension (tier 1) | Built-in CDP (tier 2) |
|-----------|--------------------|------------------------|
| Visible browser | Yes (user sees actions) | No (headless, no window) |
| User's cookies/logins | Yes (user's Chrome profile) | Separate profile (persistent only if `storage_dir` set) |
| Extension install required | Yes | No |
| Requires bundled obscura binary | No | Yes |
| Tab/session model | Per-Nebo-session Chrome tab groups | One CDP page per `session_id` |
| Batching | One round-trip (`execute_batch`) | Sequential per action |
| Humanized input | Curved mouse, click hold, typing cadence | **Same** (`human_input.rs`, 2026-06-11) |
| Element targeting | refs (accessibility tree) + selector/coordinate | selector + coordinate (no ref surface yet) |
| Supported actions | full tool set | navigate, read_page, evaluate, click, type, press |

### 25.3 Known Robustness Gap

Leaked CDP targets + a dead-connection cache can wedge tier-2 `new_page` in very long sessions (`error=cdp new_page:`); bounded in real runs by `close_session`, and the 30s `new_page` timeout now recycles the connection. A fresh restart clears it. (See memory `obscura-tier2-target-leak-wedge`.)
| Performance | Relay chain, but real user browser | Subprocess calls, no native messaging relay |
