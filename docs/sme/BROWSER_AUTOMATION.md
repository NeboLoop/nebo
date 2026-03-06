# Browser Automation System — Comprehensive SME Reference

Source: `nebo-rs/crates/browser/`, `nebo-rs/crates/tools/src/web_tool.rs`, `nebo-rs/crates/cli/src/main.rs`, `chrome-extension/`

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
9. [WebTool Integration](#9-webtool-integration)
10. [Native Messaging Host Manifest](#10-native-messaging-host-manifest)
11. [Content Scripts](#11-content-scripts)
12. [Native Message Protocol](#12-native-message-protocol)
13. [Timeouts and Error Handling](#13-timeouts-and-error-handling)
14. [Connection Lifecycle](#14-connection-lifecycle)
15. [Audit Logging](#15-audit-logging)
16. [Known Issues and Failure Modes](#16-known-issues-and-failure-modes)
17. [Debugging Guide](#17-debugging-guide)

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
│    │  queues ToolRequest into mpsc channel                                          │
│    │                                                                                │
│    ▼                                                                                │
│  WS Handler (/ws/extension)                  ← Rust, Axum WebSocket                │
│    │  reads from bridge queue, sends JSON over WS                                   │
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

**Key insight:** The `nebo` binary serves double duty:
- Normal mode: full server + agent
- Native messaging mode: lightweight stdin/stdout relay bridge (detected by `chrome-extension://` arg)

Chrome launches the nebo binary as a native messaging host. The binary detects the `chrome-extension://` argument and runs `run_native_messaging()` instead of the full server.

---

## 2. Message Flow

### Request (agent → extension)

```
1. Agent calls web(action: "navigate", url: "https://example.com")
2. WebTool.handle_browser() → manager.executor() → ActionExecutor.execute()
3. ActionExecutor → ExtensionBridge.execute("navigate", {"url": "..."})
4. ExtensionBridge:
   a. Assigns monotonic request ID
   b. Creates oneshot channel for response
   c. Stores (id → sender) in pending HashMap
   d. Sends ToolRequest to mpsc channel
5. WS handler (extension_ws_handler):
   a. recv_request() pulls from mpsc channel
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
**Extension ID (production):** `heaeiepdllbncnnlfniglgmbfmmemkcg`
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
| `src/tools.ts` | Tool implementations. 17 tools using CDP and content scripts. |
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
| `fill` | `chrome.scripting.executeScript` | Sets `.value` + dispatches input/change events |
| `type` | CDP `Input.dispatchKeyEvent` | Character-by-character keyDown/keyUp |
| `select` | `chrome.scripting.executeScript` | Sets `<select>.value` + dispatches change event |
| `screenshot` | CDP `Page.captureScreenshot` | Returns base64 PNG |
| `scroll` | `chrome.scripting.executeScript` | `window.scrollBy(x, y)` |
| `press` | CDP `Input.dispatchKeyEvent` | Mapped key names (Enter, Tab, etc.) to CDP key codes |
| `go_back` | CDP `Page.navigateToHistoryEntry(-1)` | Falls back to `history.back()` |
| `go_forward` | CDP `Page.navigateToHistoryEntry(1)` | Falls back to `history.forward()` |
| `wait` | `setTimeout` | Capped at 10s |
| `evaluate` | CDP `Runtime.evaluate` | Runs arbitrary JS, returns value |
| `new_tab` | `chrome.tabs.create` | Updates agentTabId to the new tab |
| `close_tab` | `chrome.tabs.remove` | Clears agentTabId if closing agent tab |
| `list_tabs` | `chrome.tabs.query({})` | Returns all tabs (no agent tab needed) |

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

**File:** `nebo-rs/crates/cli/src/main.rs`, lines 437-558
**Function:** `run_native_messaging()`

### 4.1 Detection

Before CLI arg parsing (`main()` line 90-100):
```rust
let args: Vec<String> = std::env::args().collect();
let is_native_messaging = args.iter().any(|a| a.starts_with("chrome-extension://"))
    || args.iter().any(|a| a == "native-messaging");
if is_native_messaging {
    return run_native_messaging().await;
}
```

Chrome passes `chrome-extension://EXTENSION_ID/` as a CLI argument when launching the native messaging host. The binary detects this and enters relay mode.

Also available as explicit `nebo native-messaging` subcommand.

### 4.2 Connection

Connects to `ws://127.0.0.1:27895/ws/extension` with exponential backoff:
- 10 attempts max
- Delay: `min(500 * 2^(attempt-1), 5000)` ms → 500, 1000, 2000, 4000, 5000, 5000...
- On exhaustion: `process::exit(1)` — Chrome will relaunch via onDisconnect

### 4.3 Bidirectional Bridge

Two concurrent tokio tasks:

**Task 1: stdin → WS (extension → server)**
- Reads 4-byte LE length prefix + JSON from stdin (Chrome native messaging protocol)
- Handles `hello` locally (responds with `{"type": "connected"}` via stdout) AND forwards to server
- Handles `ping` locally (responds with `{"type": "pong"}` via stdout)
- Everything else forwarded as WS text message

**Task 2: WS → stdout (server → extension)**
- Reads WS text messages
- Writes as native messages (4-byte LE length + JSON) to stdout

### 4.4 Termination

When either task finishes (stdin closes or WS breaks):
- `tokio::select!` returns
- `process::exit(0)` — force exit because tokio's blocking stdin thread prevents clean shutdown
- Chrome's `onDisconnect` fires, extension schedules reconnect after 2s

---

## 5. Server-Side WebSocket Handler

**File:** `nebo-rs/crates/server/src/handlers/ws.rs`, lines 521-605
**Route:** `GET /ws/extension` (registered in `lib.rs` line 481)

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
    let conn_gen = bridge.connect();  // Increment active_connections
    let (ws_tx, ws_rx) = socket.split();

    // Task 1: Bridge queue → WS (tool requests to extension)
    // Task 2: WS → Bridge deliver (tool responses from extension)

    tokio::select! { ... }

    bridge.disconnect(conn_gen);  // Decrement active_connections
}
```

### 5.3 Send Task (bridge → WS → relay → extension)

Loops on `bridge.recv_request()`. For each `ToolRequest`:
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

**File:** `nebo-rs/crates/browser/src/extension_bridge.rs`
**Shared via:** `AppState.extension_bridge` (Arc)

### 6.1 State

| Field | Type | Purpose |
|-------|------|---------|
| `request_tx` | `mpsc::Sender<ToolRequest>` | Outbound queue (capacity 64) |
| `request_rx` | `Arc<Mutex<mpsc::Receiver<ToolRequest>>>` | Consumed by WS handler |
| `pending` | `Arc<Mutex<HashMap<i64, oneshot::Sender<...>>>>` | Response channels keyed by request ID |
| `next_id` | `Arc<AtomicI64>` | Monotonic request ID counter |
| `active_connections` | `Arc<AtomicI64>` | WS connection count (>0 = connected) |

### 6.2 Key Methods

**`execute(tool, args) -> Result<Value, String>`**
1. Check `is_connected()` — returns error if no WS connections
2. Assign ID from `next_id`
3. Create oneshot channel, store sender in `pending`
4. Send `ToolRequest` to `request_tx`
5. **Wait with 30s timeout** on oneshot receiver
6. On timeout: remove from pending, return `Err("Tool 'X' timed out after 30s")`
7. On receive: return the `Result<Value, String>` from the extension

**`connect() -> i64`**: Atomically increment `active_connections`, return generation

**`disconnect(conn_id)`**: Atomically decrement `active_connections`. If count reaches 0: spawn task to reject ALL pending requests with `"Extension disconnected"`

**`recv_request() -> Option<ToolRequest>`**: Blocking receive on mpsc channel (called by WS handler)

**`deliver_result(id, result)`**: Look up oneshot sender in pending map, send result

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

**File:** `nebo-rs/crates/browser/src/executor.rs`

Thin wrapper around `ExtensionBridge`. Used by `WebTool` to execute browser actions.

```rust
pub struct ActionExecutor {
    bridge: Arc<ExtensionBridge>,
}
```

- `is_connected()`: Delegates to `bridge.is_connected()`
- `execute(tool, args) -> Result<Value, BrowserError>`: Logs via audit, delegates to `bridge.execute()`, maps `String` errors to `BrowserError::Other`

---

## 8. Browser Manager

**File:** `nebo-rs/crates/browser/src/manager.rs`

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

**Note:** The `launch()`, `stop()`, `get_or_create_session()` methods are for **managed Chrome profiles** (local CDP mode), NOT used in the extension path.

---

## 9. WebTool Integration

**File:** `nebo-rs/crates/tools/src/web_tool.rs`

The `web` domain tool handles three resource types: `http`, `search`, `browser`.

### 9.1 Browser Action Routing

```
web(action: "navigate", url: "...")
  → infer_resource("navigate") returns "browser"
  → handle_browser(input)
    → Check manager exists (Option<Arc<Manager>>)
    → "status" action works even disconnected
    → Get executor: manager.executor()
    → Check executor.is_connected()
    → handle_browser_via_extension(executor, action, input)
```

### 9.2 Action Name Mapping

WebTool maps user-facing action names to extension tool names:

| Web Tool Action | Extension Tool |
|----------------|---------------|
| `snapshot`, `read_page` | `read_page` |
| `navigate` | `navigate` |
| `click` | `click` |
| `fill` | `fill` |
| `type` | `type` |
| `select` | `select` |
| `screenshot` | `screenshot` |
| `scroll` | `scroll` |
| `press` | `press` |
| `back`, `go_back` | `go_back` |
| `forward`, `go_forward` | `go_forward` |
| `wait` | `wait` |
| `evaluate` | `evaluate` |
| `list_tabs` | `list_tabs` |
| `new_tab` | `new_tab` |
| `close_tab`, `close` | `close_tab` |

### 9.3 Argument Forwarding

`build_extension_args()` selectively forwards parameters per action:

| Action | Forwarded Keys |
|--------|---------------|
| `navigate`, `new_tab` | `url` |
| `click` | `ref`, `selector` |
| `fill` | `ref`, `selector`, `value` |
| `type` | `text` |
| `select` | `ref`, `selector`, `value` |
| `scroll` | `direction`, `amount` |
| `press` | `key` |
| `wait` | `ms` |
| `evaluate` | `expression` |
| `read_page` | `filter`, `depth`, `maxChars`, `refId` |

### 9.4 Error Messages

| Condition | Error |
|-----------|-------|
| No browser manager | "Browser automation is not available. Use web(action: \"fetch\"...)" |
| Extension disconnected | "Browser extension not connected. The Nebo extension must be installed..." |
| Unsupported action | "Browser action 'X' is not supported via extension." |

---

## 10. Native Messaging Host Manifest

**File:** `nebo-rs/crates/browser/src/native_host.rs`, lines 246-346

### 10.1 Installation

Called from `server/src/lib.rs` (line 252) on server startup, only if not already installed:

```rust
if !browser::native_host::is_manifest_installed() {
    let nebo_binary = std::env::current_exe()...;
    let local_ext_id = cfg.browser_extension_id.as_deref().unwrap_or("");
    browser::native_host::install_manifest(&nebo_binary, local_ext_id);
}
```

### 10.2 Manifest Content

Written as `dev.neboloop.nebo.json`:

```json
{
  "name": "dev.neboloop.nebo",
  "description": "Nebo Browser Automation Host",
  "path": "/path/to/nebo",
  "type": "stdio",
  "allowed_origins": [
    "chrome-extension://heaeiepdllbncnnlfniglgmbfmmemkcg/",
    "chrome-extension://LOCAL_EXTENSION_ID/"
  ]
}
```

Production extension ID: `heaeiepdllbncnnlfniglgmbfmmemkcg`
Local dev extension ID: configurable via `cfg.browser_extension_id`

### 10.3 Browser Directories

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

### 10.4 Manifest Verification

`is_manifest_installed()` checks if `dev.neboloop.nebo.json` exists in ANY of the above directories. Returns true if found in at least one.

---

## 11. Content Scripts

### 11.1 Accessibility Tree (`content/accessibility-tree.ts`)

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

### 11.2 Visual Indicator (`content/visual-indicator.ts`)

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

## 12. Native Message Protocol

Chrome native messaging uses a simple binary protocol: **4-byte little-endian length prefix + JSON body**.

Maximum message size: 1 MB (Chrome hard limit).

### 12.1 Message Types

**Rust struct:** `NativeMessage` in `browser/src/native_types.rs`

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

### 12.2 Message Types Table

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

## 13. Timeouts and Error Handling

### 13.1 Timeout Locations

| Component | Timeout | Location |
|-----------|---------|----------|
| ExtensionBridge.execute() | **30 seconds** | `extension_bridge.rs:116` |
| NativeHost.execute_tool() | **30 seconds** | `native_host.rs:179` |
| Relay WS connection backoff | 500ms → 5s (10 attempts) | `main.rs:451-463` |
| Extension reconnect | 2 seconds | `native.ts:224` |
| Extension keep-alive alarm | 24 seconds | `background.ts:42` |
| Navigate page load wait | 1 second (fixed) | `tools.ts:184` |
| Options page connection test | 3 seconds | `options.ts:38` |
| Indicator hide animation | 350ms | `visual-indicator.ts:178` |
| Agent tab close delay | 400ms | `native.ts:201` |

### 13.2 Error Propagation

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

### 13.3 Disconnection Handling

When the relay bridge process dies (WS closes):
1. WS handler's recv task exits
2. `bridge.disconnect(conn_gen)` called
3. If last connection: all pending requests get `Err("Extension disconnected")`
4. Extension's `onDisconnect` fires → schedules 2s reconnect
5. Keep-alive alarm also reconnects if `!isConnected()`

When the extension disconnects (stdin closes):
1. Relay's send task exits (stdin read fails)
2. `tokio::select!` fires, `process::exit(0)`
3. Chrome's `onDisconnect` fires in extension → reconnect

---

## 14. Connection Lifecycle

### 14.1 Startup Sequence

```
1. Server starts (lib.rs run())
   a. Creates BrowserManager → creates ExtensionBridge (not yet connected)
   b. Installs native messaging manifest if not present
   c. Registers /ws/extension route

2. User opens Chrome with Nebo extension
   a. Service worker loads (background.ts)
   b. Calls native.connect()
   c. chrome.runtime.connectNative("dev.neboloop.nebo")
   d. Chrome reads manifest, launches /path/to/nebo with chrome-extension://ID/ arg

3. Relay bridge process starts (run_native_messaging)
   a. Connects WS to ws://127.0.0.1:27895/ws/extension (with backoff)
   b. WS handler calls bridge.connect() → active_connections = 1

4. Extension sends hello via native messaging
   a. Relay handles locally (responds with "connected") AND forwards to server
   b. WS handler receives hello, logs it

5. Bridge is now fully connected
   a. ExtensionBridge.is_connected() returns true
   b. Agent tool calls can flow through
```

### 14.2 Multiple Connection Handling

The bridge supports multiple simultaneous WS connections (`active_connections` > 1). This handles the overlap during reconnection — a new relay can connect before the old one fully disconnects. Pending requests are only rejected when the LAST connection drops (count reaches 0).

### 14.3 Server Restart Recovery

1. Server restarts → WS breaks
2. Relay's recv task sees WS close → `process::exit(0)`
3. Chrome's `onDisconnect` fires → extension schedules 2s reconnect
4. Extension calls `chrome.runtime.connectNative()` → Chrome launches new relay
5. New relay retries WS with backoff → connects when server is ready
6. Bridge reconnected

---

## 15. Audit Logging

**File:** `nebo-rs/crates/browser/src/audit.rs`

Logs every tool request via `tracing`. Sensitive tools (`evaluate`, `screenshot`) get `warn!` level; others get `info!`.

Called from `NativeHost.execute_tool()` only (not from ExtensionBridge — the NativeHost path is the direct desktop path, unused in the current relay architecture).

---

## 16. Known Issues and Failure Modes

### 16.1 30s Timeout Too Short for Complex Pages

**Issue:** Navigation to heavy pages (e.g., SPAs that load async content) may exceed the 30s timeout. The navigate tool in `tools.ts` only waits 1s after `Page.navigate` — but the agent may call `read_page` before the page is fully loaded, getting partial or empty content. The *real* timeout is the 30s on the ExtensionBridge, which is appropriate for the round-trip but doesn't help with page-load timing.

### 16.2 Manifest Path Staleness

**Issue:** The manifest is installed with the path to the current `nebo` binary. If the binary moves (e.g., after a self-update), the manifest points to the old path. The `is_manifest_installed()` check only verifies the file exists, not that the path is correct.

**Fix needed:** Compare manifest `path` field against current binary path on startup.

### 16.3 Extension Disconnection During Tool Execution

If the extension or relay disconnects mid-tool-execution, the pending oneshot channel is resolved with `Err("Extension disconnected")`. However, the tool may have partially executed (e.g., navigation started but result never returned).

### 16.4 Agent Tab and about:blank

The agent tab starts as `about:blank`. The first `navigate` command changes it to the target URL. If `read_page` is called before navigate, it reads `about:blank` — which returns an empty tree.

### 16.5 CDP Debugger Permission Dialog

When the extension first attaches the debugger to a tab, Chrome shows a yellow "debugging this browser" infobar. This can confuse users and may persist if the debugger isn't properly detached.

### 16.6 Content Script Injection Race

The accessibility tree content script is injected at `document_start`, but for newly created tabs (via `chrome.tabs.create`), there's a race — `ensureAgentTab()` creates the tab and immediately proceeds. The content script may not be loaded yet when `read_page` is called. The tool handles this by retrying with manual injection, but this adds latency.

### 16.7 "Cannot access contents of url" Error

This is Chrome's error when trying to execute scripts on restricted pages: `chrome://`, `chrome-extension://`, `about:blank` (sometimes), and URLs blocked by CSP. The extension catches this and returns it as a tool error, which surfaces as the timeout issue's symptom when the underlying cause is a permissions/URL issue.

### 16.8 False Disconnection During Transient Reconnects

**Root cause:** When the relay process exits (WS drop or stdin EOF), `ExtensionBridge.disconnect()` immediately rejects all pending requests and sets active_connections to 0. The extension reconnects in ~2 seconds via its `handleDisconnect` timer + keep-alive alarm. During this window, any new tool call gets "Browser extension not connected" error even though reconnection is imminent.

**Fix:** Track `last_connected` timestamp. On disconnection, don't reject pending requests (let their 30s timeout handle truly dead connections). In WebTool, add a 3-second grace period: if recently connected, poll for reconnection before failing.

---

## 17. Debugging Guide

### 17.1 Check Connection Status

**Agent tool:** `web(action: "status")`
Returns `Browser extension connected: true/false`

**Extension badge:**
- "ON" (yellow) = connected
- Empty = disconnected
- "..." = connecting
- "!" (red) = error

### 17.2 Verify Native Messaging Manifest

```bash
# macOS Chrome
cat ~/Library/Application\ Support/Google/Chrome/NativeMessagingHosts/dev.neboloop.nebo.json

# Check if the binary path is correct
# The "path" field should point to the actual nebo binary
```

### 17.3 Check Relay Process

```bash
# See if relay is running
ps aux | grep nebo | grep chrome-extension

# Check server logs for WS connection
# Look for "extension bridge connected" / "extension bridge connection closed"
```

### 17.4 Extension Console

1. Navigate to `chrome://extensions/`
2. Find "Nebo Browser Relay"
3. Click "Inspect views: service worker"
4. Check console for `[nebo]` prefixed messages:
   - `[nebo] Native host connected`
   - `[nebo] Created agent tab: N`
   - `[nebo] Native host disconnected: ...`

### 17.5 Common Failure Causes

| Symptom | Likely Cause |
|---------|-------------|
| "Chrome extension not connected" | Extension not installed, or native messaging host not found |
| "Tool 'X' timed out after 30s" | Relay not running, WS not connected, or extension service worker suspended |
| "Cannot access contents of url" | Trying to access chrome:// or extension pages |
| "Element ref_N not found" | Page changed since last read_page (WeakRef was GC'd) |
| Badge shows empty (disconnected) | Nebo server not running, or manifest path wrong |
| Extension connects then immediately disconnects | Manifest path points to wrong binary, or binary crashes |

---

## File Index

| File | Lines | Description |
|------|-------|-------------|
| `browser/src/lib.rs` | 93 | Crate root — re-exports, BrowserError enum, ElementRef types |
| `browser/src/extension_bridge.rs` | 143 | Core bridge — mpsc queue, pending map, timeout logic |
| `browser/src/executor.rs` | 47 | Thin wrapper — delegates to ExtensionBridge |
| `browser/src/manager.rs` | 193 | Manager — owns ExtensionBridge, profiles, sessions |
| `browser/src/native_host.rs` | 393 | Native messaging host — manifest install, stdin/stdout protocol, direct path |
| `browser/src/native_types.rs` | 112 | NativeMessage struct — all message types |
| `browser/src/audit.rs` | 22 | Security audit logging for sensitive tools |
| `tools/src/web_tool.rs` | 603 | WebTool — HTTP, search, browser automation |
| `cli/src/main.rs` (437-558) | ~120 | Relay bridge process — stdin/stdout ↔ WebSocket |
| `server/src/handlers/ws.rs` (521-605) | ~85 | WS handler for /ws/extension |
| `server/src/state.rs` (33) | 1 | extension_bridge field in AppState |
| `server/src/lib.rs` (240-261) | ~20 | Browser init + manifest install |
| `chrome-extension/src/background.ts` | 63 | Service worker — startup, keep-alive, toolbar |
| `chrome-extension/src/native.ts` | 255 | Native messaging — connection, agent tab, tool dispatch |
| `chrome-extension/src/tools.ts` | 455 | Tool implementations — CDP + content scripts |
| `chrome-extension/src/badge.ts` | 32 | Badge state management |
| `chrome-extension/src/types.ts` | 18 | Badge type definitions |
| `chrome-extension/src/options.ts` | 69 | Options page — connection test |
| `chrome-extension/src/content/accessibility-tree.ts` | 340 | Content script — a11y tree generation |
| `chrome-extension/src/content/visual-indicator.ts` | 226 | Content script — glow + stop button |
| `chrome-extension/manifest.json` | 51 | MV3 manifest — permissions, content scripts |
