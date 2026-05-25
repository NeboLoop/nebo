# App Platform — Rust SME Reference

> Definitive reference for the Nebo app primitive. Covers the app manifest
> format, sidecar lifecycle (launch, health check, restart, shutdown),
> gRPC proxy, storage API, agent invocation, SDK surface, and frontend
> integration. Compares apps with skills and plugins.
>
> For the publisher-facing guide, see `docs/publishers-guide/apps.md`.

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [Apps vs Skills vs Plugins — Decision Rule](#2-apps-vs-skills-vs-plugins--decision-rule)
3. [Architecture & Crate Placement](#3-architecture--crate-placement)
4. [Directory Layout](#4-directory-layout)
5. [Manifest Format](#5-manifest-format)
6. [Agent Loader Integration](#6-agent-loader-integration)
7. [Sidecar Runtime](#7-sidecar-runtime)
8. [AppLifecycle & Supervision](#8-applifecycle--supervision)
9. [gRPC Protocol](#9-grpc-protocol)
10. [HTTP Handlers](#10-http-handlers)
11. [Route Definitions](#11-route-definitions)
12. [Storage API](#12-storage-api)
13. [Agent Invocation](#13-agent-invocation)
14. [Janus LLM Gateway](#14-janus-llm-gateway)
15. [HTTP Proxy (CORS-free)](#15-http-proxy-cors-free)
16. [Identity Endpoint](#16-identity-endpoint)
17. [Embedded Chat](#17-embedded-chat)
18. [App SDK (TypeScript)](#18-app-sdk-typescript)
19. [Frontend Integration](#19-frontend-integration)
20. [Sandbox & Security](#20-sandbox--security)
21. [Database Schema](#21-database-schema)
22. [WebSocket Events](#22-websocket-events)
23. [On-Demand Sidecar Launch](#23-on-demand-sidecar-launch)
24. [Sidecar Restore on Server Startup](#24-sidecar-restore-on-server-startup)
25. [App Agent Redaction](#25-app-agent-redaction)
26. [Tool Scope Isolation](#26-tool-scope-isolation)
27. [Edge Cases](#27-edge-cases)
28. [Key Files](#28-key-files)

---

## 1. System Overview

An **App** is an agent with its own UI. It combines a persona (`AGENT.md`), optional workflows and skills (`agent.json`), a static frontend (`ui/`), and an optional native sidecar binary that serves API endpoints over gRPC via a Unix socket.

```
App = Agent + Frontend UI + (optional) Sidecar Binary
```

Apps run in pop-out windows (Tauri WebviewWindow on desktop, browser popup on VPS). The frontend communicates with the sidecar through Nebo's gRPC proxy — the app never connects directly to the socket.

**Key properties:**
- **Pop-out windows.** Apps never render inline. Each app gets its own window with saved size/position.
- **Filesystem-first.** The agent loader scans `~/.nebo/user/agents/` for directories containing `AGENT.md` + `manifest.json` with `artifact_type: "app"`.
- **On-demand sidecar.** Sidecar binaries auto-launch on first API request. No explicit activation step.
- **Supervised process.** Health checks every 15s, auto-restart with exponential backoff (max 5/hour).
- **Sandboxed environment.** Sidecar processes run with a sanitized environment — no access to API keys, database URLs, or secrets.
- **Persistent storage.** Apps get a scoped KV store via the storage API.

---

## 2. Apps vs Skills vs Plugins — Decision Rule

> **No UI, just knowledge → Skill.**
> **Shared binary, no UI → Plugin.**
> **Has a UI → App.**

| Need | Artifact | Why |
|------|----------|-----|
| Teach the agent to draft sales emails | Skill | Knowledge + instructions, no binary |
| Shared Google Workspace CLI | Plugin | One binary, many skills |
| Contact manager with searchable list | App | Needs its own rendered UI |
| Dashboard with charts | App | Visual output beyond chat bubbles |
| Journal with reflection prompts | App | Persistent UI state + custom layout |

Apps are the heaviest artifact type. Use a skill if chat output suffices. Use an app only when the user needs to see and interact with a dedicated UI.

---

## 3. Architecture & Crate Placement

```
launcher.ts (frontend)
  → Tauri WebviewWindow / browser popup
    → serve_app_ui() — static files from ui/
    → proxy_to_sidecar() — gRPC via Unix socket

crates/server/src/handlers/apps.rs     — HTTP handlers
crates/server/src/routes/apps.rs       — Route definitions
crates/server/src/app_lifecycle.rs     — Process management + health checker
crates/napp/src/runtime.rs            — Binary discovery + process launch
crates/napp/src/manifest.rs           — Manifest parsing + validation
crates/napp/src/sandbox.rs            — Environment sanitization + binary validation
crates/napp/src/supervisor.rs         — Restart policy + backoff
crates/proto/                         — gRPC proto definitions (UIService, etc.)
@neboai/app-sdk (npm)                 — TypeScript SDK for app frontends (source: NeboLoop/app-sdk)
```

---

## 4. Directory Layout

```
my-app/
├── AGENT.md              # Required — persona + instructions
├── manifest.json         # Required — identity, permissions, window config
├── agent.json            # Optional — workflows, skills, pricing
├── ui/                   # Required for apps — static frontend
│   ├── index.html        #   Entry point (served at /apps/{id}/ui/index.html)
│   ├── style.css
│   └── app.js
├── sidecar/              # Optional — Rust sidecar project
│   ├── Cargo.toml
│   ├── src/main.rs
│   └── target/release/
│       └── my-app-sidecar   # Compiled binary
├── bin/                  # Alternative binary location
└── data/                 # Auto-created — app-scoped data directory
```

The sidecar is optional. Apps without a sidecar are pure-frontend — they use the SDK's `agents.invoke()` and `janus.complete()` to talk to the LLM, and `storage` for persistence. No native binary needed.

---

## 5. Manifest Format

**File:** `manifest.json`

```json
{
  "id": "journal",
  "name": "@nebo/agents/journal",
  "version": "1.0.0",
  "description": "AI-powered journal with reflection prompts.",
  "artifact_type": "app",
  "permissions": [
    "storage:readwrite",
    "subagent:invoke",
    "network:outbound"
  ],
  "window": {
    "title": "Journal",
    "width": 700,
    "height": 800,
    "min_width": 400,
    "min_height": 500,
    "resizable": true
  }
}
```

### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes* | Unique identifier. Must match agent directory name. *Can be omitted if `name` starts with `@`. |
| `name` | string | yes | Qualified name (`@org/agents/name`) or display name |
| `version` | string | yes | Semantic version |
| `artifact_type` | string | yes | Must be `"app"` for apps |
| `description` | string | no | One-line description |
| `permissions` | string[] | no | Required permissions (see §18) |
| `window` | object | no | Default window dimensions |

### Window Config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `title` | string | app name | Window title bar text |
| `width` | u32 | 1024 | Default width in pixels |
| `height` | u32 | 768 | Default height in pixels |
| `min_width` | u32 | — | Minimum width |
| `min_height` | u32 | — | Minimum height |
| `resizable` | bool | true | Whether the user can resize |

### Permission Prefixes

Permissions use `prefix:scope` format:

| Prefix | Scopes | Description |
|--------|--------|-------------|
| `storage:` | `readwrite`, `read` | App KV storage |
| `network:` | `outbound` | HTTP requests via proxy |
| `subagent:` | `invoke` | Invoke other agents |
| `capability:` | varies | Access platform capabilities |
| `shell:` | `execute` | Run shell commands |
| `filesystem:` | `read`, `write` | File access |
| `memory:` | `read`, `write` | Agent memory |
| `session:` | `read`, `write` | Session data |
| `tool:` | tool name | Use specific tools |
| `oauth:` | provider | OAuth flow |

### Validation Rules

- `id` required unless `name` starts with `@` (qualified name provides identity)
- `version` must be present
- `permissions` must use valid prefixes (see `VALID_PERMISSION_PREFIXES` in `manifest.rs`)
- `startup_timeout` ≤ 120 seconds (default 10)
- Binary ≤ 500 MB

**Source:** `crates/napp/src/manifest.rs`

---

## 6. Agent Loader Integration

The `AgentLoader` scans the filesystem for agents. For apps, it detects:

1. `manifest.json` with `artifact_type: "app"` → sets `is_app = true`
2. `ui/` subdirectory → sets `app_ui_path`
3. Binary in `binary`, `app`, `tmp/`, `bin/`, or `sidecar/target/release/` → sets `app_binary_path`
4. `window` field in manifest → sets `app_window_config`

**Loading priority (lowest to highest):**
1. Embedded bundled agents (compiled into binary)
2. Installed agents (`{NEBO_ROOT}/installed/`)
3. User agents (`{NEBO_ROOT}/user/agents/`)

The loader watches the filesystem for changes and emits `AgentFsEvent::Added`, `Changed`, `Removed` events for hot-reload.

**Source:** `crates/napp/src/agent_loader.rs`

---

## 7. Sidecar Runtime

The `napp::Runtime` manages binary discovery, process spawning, and socket handshake.

### Binary Discovery

`find_binary(tool_dir)` searches in order:

1. `{tool_dir}/binary` — single named binary
2. `{tool_dir}/app` — single named binary
3. `{tool_dir}/tmp/` — first file found
4. `{tool_dir}/bin/` — first file found
5. `{tool_dir}/sidecar/target/release/` — first executable file without extension

### Launch Sequence

```
load manifest.json → validate → find_binary → validate_binary
  → sanitize_env → spawn process → wait_for_socket → health_check
  → set socket permissions (0o600) → write PID file
```

1. **Load manifest** from `{tool_dir}/manifest.json`
2. **Find binary** using search order above
3. **Validate binary**: regular file, not symlink, executable, valid magic bytes (ELF, Mach-O 64, Mach-O 32, Mach-O 32 swapped, Universal/fat binary, PE), ≤ 500MB
4. **Sanitize environment**: clear all env vars, set only:
   - `NEBO_APP_ID`, `NEBO_APP_NAME`, `NEBO_APP_VERSION`
   - `NEBO_APP_DIR`, `NEBO_APP_SOCK`, `NEBO_APP_DATA`
   - `NEBO_API_URL` (callback URL: `http://127.0.0.1:{port}` — sidecar uses this to call Nebo's SDK endpoints)
   - `NEBO_APP_TOKEN` (injected post-sanitize for authenticated sidecar requests)
   - System: `PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL`, `TZ`
5. **Spawn process**: `Command::new(binary)` with process group isolation (`setpgid(0,0)`)
6. **Wait for socket**: polls for `{tool_dir}/{id}.sock` (timeout from manifest, default 10s)
7. **Health check**: try connecting to socket (best-effort, warns on failure)
8. **Set socket permissions**: `0o600` (owner only)
9. **Write PID file**: `{tool_dir}/{id}.pid`

### Process Lifecycle

```rust
pub struct Process {
    pub tool_id: String,
    pub manifest: Manifest,
    pub pid: u32,
    pub sock_path: PathBuf,
    pub binary_path: PathBuf,
    pub app_token: String,
    child: tokio::process::Child,
    binary_mtime: std::time::SystemTime,
}
```

- `is_alive()`: `libc::kill(pid, 0)` — checks process exists without sending a signal
- `stop()`: SIGTERM to process group → 2s wait → force SIGKILL → cleanup socket + PID file
- `grpc_endpoint()`: `unix://{sock_path}`

**Source:** `crates/napp/src/runtime.rs`

---

## 8. AppLifecycle & Supervision

`AppLifecycle` wraps the runtime with health checking and automatic restart.

```rust
pub struct AppLifecycle {
    agent_id: String,
    tool_dir: PathBuf,
    runtime: Arc<napp::Runtime>,
    supervisor: Arc<napp::supervisor::Supervisor>,
    process: Arc<Mutex<Option<napp::Process>>>,
    cancel: CancellationToken,
    hub: Arc<ClientHub>,
    registry: Arc<tools::Registry>,
    skill_loader: Arc<tools::skills::Loader>,
    loaded_skill_names: Vec<String>,
    app_token: Arc<tokio::sync::RwLock<String>>,
    permissions: Arc<tokio::sync::RwLock<Vec<String>>>,
}
```

### Lifecycle Methods

- `new(agent_id, tool_dir, hub, registry, skill_loader)` — creates runtime, supervisor, cancellation token
- `launch()` — cleans stale sockets, launches process, discovers sidecar tools, loads app skills, starts health checker, broadcasts `app_started`
- `shutdown()` — cancels health checker, stops process, unregisters agent tools (`registry.unregister_agent_tools(agent_id)`), unloads skills (`skill_loader.unload_skills()`), broadcasts `app_stopped`

### Health Checker

Spawned as a background tokio task. Runs every 15 seconds:

```
loop:
  is_alive()?
    yes → continue
    no  → broadcast "app_crashed"
          should_restart()?
            no  → continue (limit reached)
            yes → record_restart() → launch()
                  success → broadcast "app_restarted"
                  failure → warn, continue
```

### Restart Policy (Supervisor)

| Parameter | Value |
|-----------|-------|
| Max restarts per window | 5 |
| Window duration | 1 hour |
| Initial backoff | 10 seconds |
| Backoff multiplier | 2x |
| Max backoff | 5 minutes |
| Health check interval | 15 seconds |

After 1 hour of no restarts, the window and backoff reset.

### Sidecar Tool Registration

After launch, `discover_tools()` registers sidecar tools declared in `agent.json` into the global tool registry:

1. `read_tool_defs_from_config(agent_root, agent_id)` parses `agent.json` for tool definitions
2. Each tool definition becomes a `SidecarActionTool` backed by a `GrpcSidecarCaller`
3. `GrpcSidecarCaller` implements the `SidecarCaller` trait — routes tool invocations through `UIService.HandleRequest` over the gRPC Unix socket
4. Tools are registered per-agent via `registry.register_for_agent(agent_id, tool)` (scoped to that agent)
5. On shutdown: `registry.unregister_agent_tools(agent_id)` removes all tools for the agent

### App Skill Management

Apps can bundle skills in their tool directory. `loaded_skill_names: Vec<String>` tracks which skills were loaded for cleanup:

- On launch: `skill_loader.load_app_skills(tool_dir)` loads app-bundled skills
- On shutdown: `skill_loader.unload_skills(loaded_skill_names)` removes them
- Loading priority (lowest to highest): bundled → installed `.napp` → user files → **app skills**

### Binary Hot-Reload Detection

The health checker distinguishes between a crash and a binary change on disk:

```
health_check tick:
  process.binary_changed()?  (compares on-disk mtime vs launch-time mtime)
    yes && alive → hot-reload:
      unregister_agent_tools(agent_id)
      stop process
      re-launch → re-discover tools
      broadcast "app_restarted" { reason: "binary_changed" }
    no && dead → crash:
      broadcast "app_crashed"
      supervisor backoff → re-launch
```

`Process.binary_changed()` follows symlinks (`fs::canonicalize`) so rebuilds in `sidecar/target/release/` are detected even when the tool directory uses a symlink. Exponential backoff applies only to actual crashes; hot-reloads restart immediately.

**Source:** `crates/server/src/app_lifecycle.rs`, `crates/napp/src/supervisor.rs`

---

## 9. gRPC Protocol

Apps communicate through protocol buffer services defined in `proto/apps/v0/`.

### UIService (core)

```protobuf
service UIService {
  rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
  rpc Configure(SettingsMap) returns (Empty);
  rpc HandleRequest(HttpRequest) returns (HttpResponse);
}

message HttpRequest {
  string method = 1;
  string path = 2;
  string query = 3;
  map<string, string> headers = 4;
  bytes body = 5;
}

message HttpResponse {
  int32 status_code = 1;
  map<string, string> headers = 2;
  bytes body = 3;
}
```

The `HandleRequest` RPC is the primary proxy — all HTTP requests to `/apps/{id}/api/*` are converted to gRPC calls and forwarded to the sidecar.

### Additional Services

| Service | Proto File | Purpose |
|---------|-----------|---------|
| `ToolService` | `tool.proto` | Register typed tools (name, schema, execute) |
| `GatewayService` | `gateway.proto` | Custom LLM provider routing |
| `HookService` | `hooks.proto` | Intercept Nebo behavior (pre/post hooks) |
| `ScheduleService` | `schedule.proto` | Replace built-in scheduler |

### Common Types

```protobuf
message HealthCheckResponse {
  bool healthy = 1;
  string version = 2;
  string name = 3;
}

message UserContext {
  string token = 1;
  string user_id = 2;
  string plan = 3;
}
```

**Source:** `crates/proto/`

---

## 10. HTTP Handlers

All handlers are in `crates/server/src/handlers/apps.rs`.

### serve_app_ui / serve_app_ui_root

```
GET /apps/{agent_id}/ui/{*path}
GET /apps/{agent_id}/ui/              (trailing-slash root)
```

Serves static files from the app's `ui/` directory. `serve_app_ui_root` handles the trailing-slash case (no path segment). Sanitizes path (rejects `..`). Falls back to `index.html` for SPA routing. Caches immutable assets; `index.html` is never cached.

### proxy_to_sidecar

```
ANY /apps/{agent_id}/api/{*path}
```

1. Validates agent exists and `is_app == 1`
2. Resolves socket path (from `napp_path` or `app_tool_dir`)
3. Auto-launches sidecar if socket doesn't exist (see §21)
4. Converts HTTP → gRPC `UIService.HandleRequest`
5. Connects via Unix socket using `tonic::Endpoint::connect_with_connector_lazy`
6. Returns gRPC response as HTTP response

Max request body: 10 MB.

### invoke_agent / stream_agent

```
POST /apps/{agent_id}/agents/invoke
POST /apps/{agent_id}/agents/stream
```

Runs a full agent chat loop. Supports `agent` override (invoke a different agent). Returns text + tool calls (invoke) or SSE stream (stream).

### janus_complete / janus_stream

```
POST /apps/{agent_id}/janus/complete
POST /apps/{agent_id}/janus/stream
```

Direct LLM call (bypasses agent). Supports custom model, temperature, max_tokens, system prompt.

### Storage endpoints

```
GET    /apps/{agent_id}/storage         — list all keys
GET    /apps/{agent_id}/storage/{key}   — get value
PUT    /apps/{agent_id}/storage/{key}   — set value
DELETE /apps/{agent_id}/storage/{key}   — delete key
```

### get_identity

```
GET /apps/{agent_id}/identity
```

Returns the agent's identity context for the SDK. Validates `is_app == 1`. Parses frontmatter for model and skills, extracts persona body from `AGENT.md`, reads `input_values` from DB. Response:

```json
{
  "id": "journal",
  "name": "journal",
  "displayName": "Journal",
  "description": "AI-powered journal with reflection prompts.",
  "persona": "You are a thoughtful journaling companion...",
  "model": "claude-sonnet-4-20250514",
  "skills": ["web-search"],
  "inputValues": { "tone": "reflective" }
}
```

Used by `nebo.identity.get()` in the SDK. Response is cached client-side.

### http_proxy

```
POST /apps/{agent_id}/http/proxy
```

CORS-free outbound HTTP proxy. Takes `{url, method, headers, body}`, makes the request server-side.

### SDK bundle

```
GET /sdk/nebo.global.js
```

Serves the prebuilt global SDK IIFE for vanilla/HTMX apps.

---

## 11. Route Definitions

**File:** `crates/server/src/routes/apps.rs`

Routes are defined in `routes/apps.rs` and merged into the `/api/v1` namespace via `api_routes()`. The UI serving route is additionally mounted at the top level (outside `/api/v1`) in `lib.rs` so apps can load at `/apps/{id}/ui/index.html`. The SDK IIFE route is also registered at the top level in `lib.rs`.

```
/apps/{agent_id}/ui/                 → serve_app_ui_root (GET, top-level + routes/apps.rs)
/apps/{agent_id}/ui/{*path}          → serve_app_ui (GET, top-level + routes/apps.rs)
/apps/{agent_id}/api/{*path}         → proxy_to_sidecar (ANY method)
/apps/{agent_id}/agents/invoke       → invoke_agent (POST)
/apps/{agent_id}/agents/stream       → stream_agent (POST)
/apps/{agent_id}/janus/complete      → janus_complete (POST)
/apps/{agent_id}/janus/stream        → janus_stream (POST)
/apps/{agent_id}/storage             → list_storage (GET)
/apps/{agent_id}/storage/{key}       → get/put/delete_storage
/apps/{agent_id}/http/proxy          → http_proxy (POST)
/apps/{agent_id}/identity            → get_identity (GET)
/sdk/nebo.global.js                  → serve_sdk_iife (GET, top-level in lib.rs)
```

---

## 12. Storage API

Apps get a scoped KV store. Internally stored in the `plugin_settings` table with key `app:{agent_id}`.

```
GET    /apps/{agent_id}/storage         → { keys: ["key1", "key2"] }
GET    /apps/{agent_id}/storage/mykey   → { value: <any JSON> }
PUT    /apps/{agent_id}/storage/mykey   → body: { value: <any JSON> }
DELETE /apps/{agent_id}/storage/mykey   → 204 No Content
```

Auto-creates the plugin registry entry on first write. Values are JSON-serialized strings in the DB. Note: `delete_storage` is implemented as writing an empty string (`set_plugin_setting(..., "")`) rather than removing the row.

---

## 13. Agent Invocation

Apps can invoke their own agent or any other agent:

```json
POST /apps/{agent_id}/agents/invoke
{
  "message": "What should I prioritize today?",
  "agent": "optional-other-agent-id",
  "data": { "context": "any structured data" }
}
```

Response:
```json
{
  "text": "Based on your calendar, I'd suggest...",
  "tools": []
}
```

The `data` field is passed as mention context to the agent runner. The streaming variant returns SSE events with `{text, done}` chunks.

---

## 14. Janus LLM Gateway

Direct LLM access without the agent's persona or memory:

```json
POST /apps/{agent_id}/janus/complete
{
  "messages": [{"role": "user", "content": "Summarize this text..."}],
  "model": "claude-sonnet-4-20250514",
  "temperature": 0.3,
  "max_tokens": 1000,
  "system": "You are a summarization expert."
}
```

Response: `{ "text": "...", "usage": {...} }`

---

## 15. HTTP Proxy (CORS-free)

Apps that need to call external APIs can avoid CORS issues by proxying through Nebo:

```json
POST /apps/{agent_id}/http/proxy
{
  "url": "https://api.example.com/data",
  "method": "GET",
  "headers": { "Authorization": "Bearer ..." },
  "body": null
}
```

Response: `{ "status": 200, "headers": {...}, "body": "..." }`

Adds `X-Nebo-App: {agent_id}` header to outbound requests.

---

## 16. Identity Endpoint

Apps need to know "who they are" — the agent's name, persona, skills, model, and user-configured inputs. The identity endpoint exposes this context.

```
GET /apps/{agent_id}/identity
```

**Handler:** `get_identity` in `crates/server/src/handlers/apps.rs`

### Response Shape

```json
{
  "id": "deal-tracker",
  "name": "deal-tracker",
  "displayName": "Deal Tracker",
  "description": "Track real estate deals with AI-powered analysis.",
  "persona": "You are a real estate deal analyst...",
  "model": "claude-sonnet-4-20250514",
  "skills": ["web-search", "calculator"],
  "inputValues": { "market": "commercial", "currency": "USD" }
}
```

### How It Works

1. Fetches agent from DB, validates `is_app == 1`
2. Parses frontmatter for `model` and `skills`
3. Splits `AGENT.md` via `napp::agent::split_frontmatter()` → persona body
4. Computes `displayName`: window title > first `# Heading` in persona > agent name
5. Reads `input_values` from the agent DB record (user-configured values)

### SDK Usage

```typescript
const agent = await nebo.identity.get();
console.log(agent.displayName, agent.skills, agent.inputValues);
```

The SDK caches the response. Call `nebo.identity.invalidate()` to force re-fetch.

---

## 17. Embedded Chat

Apps get the full Nebo chat experience — not just `agents.invoke()` one-shots, but the actual rich chat UI with streaming, slash commands, tool visualization, voice, @mentions, ask widgets, and conversation history.

### Architecture

```
App Window
├── App's own UI (any framework)
└── <div id="chat"></div>
    └── nebo.chat.mount(div) creates:
        └── <iframe src="/app/{agentId}/chat-embed">
            └── SvelteKit page rendering ChatPane
                ├── WebSocket → /ws (main client WS)
                ├── Session key: app:{agentId}:chat
                └── Full features: slash commands, tools, voice, etc.
```

### Why Iframe

The chat UI is ~3000 lines of Svelte components (`ChatPane`, `ChatComposer`, `SlashCommandMenu`, `VoiceButton`, `AskWidget`, etc.) with dependencies on TipTap, marked, DaisyUI, and Svelte stores. An iframe:
- Gets full feature parity immediately — no component rewriting
- Style isolation — DaisyUI theme stays consistent regardless of app framework
- Works in any framework (React, Vue, HTMX, vanilla)

### Chat Embed Page

**File:** `app/src/routes/(embed)/chat-embed/[agentId]/+page.svelte`

**URL:** `/chat-embed/[agentId]`

A minimal SvelteKit page that:
1. Reads `agentId` from URL params
2. Connects to the main Nebo WebSocket (`/ws`)
3. Creates a session with key `agent:{agentId}:app` (or `agent:{agentId}:app:{contextId}` when `contextId` is set — the 4th segment enables per-context memory isolation when `agent.json` has `memory.context_isolated: true`)
4. Renders the existing `ChatPane` component
5. Accepts configuration via URL params (`placeholder`, `theme`, `borderless`, `ctx`, `scope`)
6. Implements the postMessage bridge protocol
7. Loads existing chat history via `api.getSessionMessages(sessionKey)`

### postMessage Bridge Protocol

**Parent → Iframe:**

| Message | Fields | Description |
|---------|--------|-------------|
| `nebo:send` | `message: string` | Programmatic send |
| `nebo:new-thread` | — | Start fresh conversation |
| `nebo:set-context` | `context: ChatContext \| null` | Update app context injected into agent requests |
| `nebo:configure` | `options: { placeholder? }` | Update configuration |

**Iframe → Parent:**

| Message | Fields | Description |
|---------|--------|-------------|
| `nebo:ready` | — | Iframe loaded and connected |
| `nebo:message-sent` | `message: string` | User sent a message |
| `nebo:response-complete` | `text: string` | Agent finished responding |

Note: `nebo:resize` is handled by the SDK's `chat.mount()` module — the SDK listens for content height changes from the iframe and auto-resizes the host `<iframe>` element. The embed page itself does not emit this event.

### Session Management

- Session key: `agent:{agent_id}:app` (or `agent:{agent_id}:app:{contextId}` when a `ctx` URL param is provided)
- Chat embed connects to the main WebSocket (`/ws`) and sends `chat_message` with this session key
- `rotate_chat()` (new thread) preserves old messages, starts fresh conversation
- History loads via `api.getSessionMessages(sessionKey)` on mount

### SDK Usage

```typescript
// Mount
nebo.chat.mount(document.getElementById('chat'), {
  placeholder: 'Ask about your ads...',
  height: '100%',
  borderless: true
});

// Programmatic control
nebo.chat.send('Summarize campaign performance');
nebo.chat.newThread();

// Listen for events
nebo.chat.onMessage((msg) => {
  if (msg.type === 'nebo:response-complete') {
    refreshDashboard(msg.text);
  }
});

// Cleanup
nebo.chat.unmount();
```

---

## 18. App SDK (TypeScript)

**Package:** `@neboai/app-sdk` (npm) — source repo: `NeboLoop/app-sdk`

The SDK provides a singleton `nebo` object with typed APIs for all app capabilities.

### Modules

| Module | Purpose | Source |
|--------|---------|--------|
| `nebo.identity` | Agent context (name, persona, skills, inputs) | `src/identity.ts` |
| `nebo.chat` | Embedded full-featured chat UI via iframe | `src/chat.ts` |
| `nebo.storage` | Async KV store | `src/storage.ts` |
| `nebo.agents` | Agent invocation + streaming | `src/agents.ts` |
| `nebo.janus` | Direct LLM calls | `src/janus.ts` |
| `nebo.fetch` | CORS-free HTTP proxy | `src/fetch.ts` |
| `nebo.WebSocket` | Real-time WebSocket | `src/websocket.ts` |
| `nebo.surfaces` | Agent↔app surface events | `src/surfaces.ts` |
| `nebo.a2ui` | Agent-driven UI | `src/a2ui.ts` |

### Usage

```typescript
import { nebo } from '@neboai/app-sdk';

// Identity — know who you are
const agent = await nebo.identity.get();
console.log(agent.displayName, agent.persona, agent.skills);

// Embedded chat — full Nebo chat UI in your app
nebo.chat.mount(document.getElementById('chat'), {
  placeholder: 'Ask about your deals...',
  height: '400px'
});
nebo.chat.send('Summarize pipeline');
nebo.chat.onMessage((msg) => { ... });
nebo.chat.newThread();
nebo.chat.unmount();

// Storage (async KV)
await nebo.storage.setItem('theme', 'dark');
const theme = await nebo.storage.getItem('theme');

// Agent invocation (one-shot, no conversation history)
const { text, tools } = await nebo.agents.invoke('Draft a follow-up email');

// Streaming
for await (const chunk of nebo.agents.stream('Analyze this data')) {
  console.log(chunk.text, chunk.done);
}

// Direct LLM (Janus)
const summary = await nebo.janus.complete({
  messages: [{ role: 'user', content: 'Summarize...' }],
  temperature: 0.3
});

// HTTP proxy
const resp = await nebo.fetch('/apps/my-app/api/data');

// WebSocket
const ws = new nebo.WebSocket('/apps/my-app/ws');
ws.send('update', { key: 'value' });

// Real-time surfaces (A2UI events)
nebo.surfaces.addEventListener('surface_update', (evt) => { ... });
```

### Global SDK (IIFE)

For vanilla HTML / HTMX apps that can't use ES modules:

```html
<script src="/sdk/nebo.global.js"></script>
<script>
  // nebo is available as a global
  nebo.chat.mount(document.getElementById('chat'));
  nebo.identity.get().then(agent => console.log(agent.name));
  nebo.storage.setItem('key', 'value');
</script>
```

### Surfaces API (`@neboai/app-sdk` — `src/surfaces.ts`)

Enables apps to receive structured agent events without coupling to Nebo's UI framework. The agent pushes events over WebSocket; the app renders however it wants.

**`NeboSurfaces` class:**
- `connect()` — opens WebSocket, starts listening for events
- `disconnect()` — closes connection and stops reconnecting
- `on(eventType, handler)` — subscribe to typed events (returns unsubscribe function)
- `off(eventType, handler)` — remove a listener
- `send(name, payload)` — send an action/event to the agent
- `requestState()` — request a `state_snapshot` from the agent
- `state: Record<string, unknown>` — shared state, auto-updated from `state_snapshot` and `state_delta` events

**Event types:**

| Event | Key Fields | Description |
|-------|-----------|-------------|
| `run_started` | `runId, threadId?` | Agent run began |
| `run_finished` | `runId` | Agent run completed |
| `run_error` | `runId, message, code?` | Agent run failed |
| `text_start` | `messageId` | Streaming text begins |
| `text_content` | `messageId, delta` | Streaming text chunk |
| `text_end` | `messageId` | Streaming text finished |
| `tool_call_start` | `toolCallId, toolName` | Tool execution began |
| `tool_call_end` | `toolCallId, result?` | Tool execution finished |
| `state_snapshot` | `snapshot` | Full state replacement |
| `state_delta` | `delta` (RFC 6902 JSON Patch ops) | Incremental state update |
| `surface_create` | `surfaceId, components, data?` | A2UI component tree created |
| `surface_update` | `surfaceId, components?, data?` | A2UI surface updated |
| `surface_delete` | `surfaceId` | A2UI surface removed |
| `data_update` | `surfaceId?, path?, value` | Partial data model update |
| `custom` | `name, value` | App-specific event |

Wildcard listener `on('*', handler)` receives all events.

### Chat API (`@neboai/app-sdk` — `src/chat.ts`)

Mounts the full Nebo chat UI inside an app via iframe.

**`ChatOptions`:**
- `placeholder?: string` — input placeholder text
- `theme?: 'auto' | 'light' | 'dark'` — color theme
- `height?: string` — iframe height (default `'400px'`)
- `borderless?: boolean` — remove iframe border/radius
- `contextId?: string` — scope session to a specific context (each unique ID gets its own persistent conversation)
- `scope?: string` — tool scope name from `agent.json` to restrict available tools

**`ChatContext`:**
- `projectId?: string` — current project the user is viewing
- `displayedDoc?: { filename, documentId }` — document currently displayed
- `attachedDocuments?: { filename, documentId }[]` — explicitly attached documents
- `route?: string` — current app route/page path
- Arbitrary `[key: string]: unknown` extensions

**Methods:**
- `chat.mount(element, options?)` — creates iframe pointing to `/chat-embed/{appId}`, starts listening for postMessage events
- `chat.unmount()` — removes iframe, cleans up listeners
- `chat.send(message)` — programmatic send via `nebo:send` postMessage
- `chat.onMessage(handler)` — listen for iframe events (returns unsubscribe function)
- `chat.setContext(context | null)` — update app context injected into every agent request (via `nebo:set-context`)
- `chat.newThread()` — start a fresh conversation (via `nebo:new-thread`)

### WebSocket (`@neboai/app-sdk` — `src/websocket.ts`)

**`NeboWebSocket`** — auto-connects to `ws://{base}/ws/app/{appId}` with exponential backoff reconnection.

- Initial reconnect delay: 1 second
- Max reconnect delay: 30 seconds
- Backoff: doubles on each reconnect attempt, resets to 1s on successful connection
- `send(data)` — send string, ArrayBuffer, or Blob
- `close(code?, reason?)` — close and stop reconnecting
- `readyState` — proxies to underlying WebSocket state

---

## 19. Frontend Integration

### Chat Embed Route (`app/src/routes/(embed)/chat-embed/[agentId]/+page.svelte`)

Standalone page that renders `ChatPane` in a bare layout (no sidebar, nav, or header). Used as the iframe source for `nebo.chat.mount()`. Connects to the main WebSocket, uses session key `agent:{agentId}:app` (or `agent:{agentId}:app:{contextId}`), and implements the postMessage bridge protocol. Accepts configuration via URL params (`placeholder`, `theme`, `borderless`, `ctx`, `scope`). Loads existing chat history via `api.getSessionMessages(sessionKey)` on mount.

### Apps Page (`app/src/routes/apps/+page.svelte`)

Lists all agents where `isApp === true`. Renders a card grid with icon, name, description. Click opens the app via `launchApp()`.

### Launcher (`app/src/lib/apps/launcher.ts`)

```typescript
export async function launchApp(
  agentId: string,
  appName: string,
  config?: Partial<AppWindowConfig>
): Promise<void>
```

**Tauri desktop:**
1. Check for existing window by label `app-{agentId}`
2. If exists, focus it. If closed, destroy stale handle.
3. Restore saved window position/size via `get_window_state` Tauri command
4. Create `WebviewWindow` pointing to `{origin}/apps/{agentId}/ui/index.html`

**Browser fallback:**
- Opens `window.open()` popup with app dimensions, centered on screen.

### App Wrapper Route (`app/src/routes/app/[agentId]/+page.svelte`)

Simple redirect — immediately calls `goto(\`/${agentId}/threads\`, { replaceState: true })`. Does not render any UI. Apps are launched in pop-out windows via `launchApp()`, not rendered inline at this route.

---

## 20. Sandbox & Security

### Environment Isolation

Sidecar processes start with a cleared environment. Only these variables are set:

**App-specific:**
- `NEBO_APP_ID` — agent identifier
- `NEBO_APP_NAME` — display name
- `NEBO_APP_VERSION` — manifest version
- `NEBO_APP_DIR` — tool directory path
- `NEBO_APP_SOCK` — Unix socket path
- `NEBO_APP_DATA` — data directory path
- `NEBO_API_URL` — callback URL (`http://127.0.0.1:{port}`) for calling Nebo's SDK endpoints (storage, invoke, janus, http_proxy)
- `NEBO_APP_TOKEN` — authentication token (injected post-sanitize, used as `Authorization: Bearer` for SDK endpoint calls)

**Allowlisted system:**
- `PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL`, `TZ`

**Blocking model:** The environment is cleared and only the above allowlisted variables are set. There is a `_BLOCKED_ENV_VARS` constant in the source for documentation purposes (listing `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_API_KEY`, `JWT_SECRET`, `DATABASE_URL`, `AWS_*`, `GITHUB_TOKEN`, `STRIPE_SECRET_KEY`), but it is not actively checked — blocking is implicit through the allowlist approach.

### Binary Validation

Before launching, binaries are validated:
- Must be a regular file (not symlink)
- Must be executable (Unix mode `& 0o111 != 0`)
- Must have valid magic bytes: ELF (`7f454c46`), Mach-O 64 (`feedfacf`), Mach-O 32 (`feedface`), Mach-O 32 swapped (`cefaedfe`), Universal/fat binary (`cafebabe`), PE (`4d5a`)
- Scripts (shebang `#!`) are rejected
- Size ≤ 500 MB

### Process Isolation

- Process group isolation via `setpgid(0, 0)` — prevents signals from leaking
- Socket permissions `0o600` — owner-only access
- SIGTERM → process group on shutdown (kills all child processes)

**Source:** `crates/napp/src/sandbox.rs`

---

## 21. Database Schema

### Agent table (app-relevant columns)

| Column | Type | Description |
|--------|------|-------------|
| `is_app` | INTEGER | 0 or 1 |
| `app_ui_path` | TEXT | Path to `ui/` directory |
| `app_binary_path` | TEXT | Path to sidecar binary |
| `app_window_config` | TEXT | JSON window config |
| `napp_path` | TEXT | Path to agent directory |

Note: `app_ui_path` and `app_binary_path` are `skip_serializing` — never sent to the frontend.

### App Storage

Uses the `plugin_settings` table:

| Column | Value |
|--------|-------|
| `plugin_name` | `app:{agent_id}` |
| `setting_key` | user-provided key |
| `setting_value` | JSON-serialized value |

---

## 22. WebSocket Events

Apps emit lifecycle events through the WebSocket hub:

| Event | Payload | When |
|-------|---------|------|
| `app_started` | `{ agentId, sockPath }` | Sidecar launched successfully |
| `app_stopped` | `{ agentId }` | Sidecar shut down |
| `app_crashed` | `{ agentId }` | Sidecar process died |
| `app_restarted` | `{ agentId, restartCount }` | Sidecar auto-restarted |

---

## 23. On-Demand Sidecar Launch

Sidecars launch automatically on first API request. No explicit activation step.

**Flow in `proxy_to_sidecar`:**

```
request arrives at /apps/{id}/api/...
  → resolve sock_path
  → if !sock_path.exists():
      → acquire app_lifecycles write lock
      → double-check: if already in map, skip
      → resolve tool_dir from app_tool_dir()
      → create AppLifecycle::new(agent_id, tool_dir, hub, registry, skill_loader)
      → lifecycle.launch().await
      → store in app_lifecycles map
  → connect to socket via tonic gRPC
  → proxy request → response
```

The first API request to a sidecar that is not running triggers automatic launch — no explicit activation step needed.

The `app_lifecycles` map is an `Arc<tokio::sync::RwLock<HashMap<String, AppLifecycle>>>` on `AppState`.

### Tool Directory Resolution

`app_tool_dir(agent)` derives the sidecar directory:
1. If `app_ui_path` set → parent directory (e.g., `~/.nebo/user/agents/journal/` from `journal/ui`)
2. If `app_binary_path` set → parent (or grandparent if parent is `bin/`)
3. Otherwise → `None` (no sidecar possible)

### Socket Path Resolution

`sidecar_sock_path(agent)` determines where the socket should be:
1. If `napp_path` set → `{napp_path}/{agent.id}.sock`
2. Otherwise → `{app_tool_dir}/{agent.id}.sock`

---

## 24. Sidecar Restore on Server Startup

On boot, the server automatically relaunches sidecars for enabled app agents. This prevents user-visible breakage when the server restarts.

**Flow in `start_server()` (`crates/server/src/lib.rs`):**

```
list_agents() → for each agent where is_enabled=1 AND is_app=1:
  → resolve tool_dir via app_tool_dir(agent)
  → create AppLifecycle::new(agent_id, tool_dir, hub, registry, skill_loader)
  → lifecycle.launch().await
  → store in app_lifecycles map
```

The restore runs after plugin initialization and before the comm message handler is registered. Failed launches are logged as warnings but do not block server startup.

---

## 25. App Agent Redaction

For `is_app=true` agents, the `get_agent()` handler strips sensitive publisher IP before returning the response. Only UI-needed fields are included:

**Returned fields:** `id`, `name`, `description`, `isApp`, `isEnabled`, `kind`, `appWindowConfig`, `inputValues`, `installedAt`, `updatedAt`, `pricingModel`, `pricingCost`, `displayName`, `version`, `inputFields`, `views`, `pluginsNeedingAuth`

**Stripped fields:** `persona` (full AGENT.md body), skills content, frontmatter — anything that exposes the publisher's prompt engineering or agent configuration.

Non-app agents continue to return the full response.

**Source:** `crates/server/src/handlers/agents.rs` (`get_agent`)

---

## 26. Tool Scope Isolation

Apps can declare named scopes in `agent.json` to restrict which tools, skills, and plugins are active for a given embed chat session.

### agent.json Declaration

```json
{
  "scopes": {
    "read": {
      "tools": ["search_docs", "get_summary"],
      "skills": ["web-search"],
      "plugins": []
    },
    "write": {
      "tools": ["search_docs", "get_summary", "create_doc", "update_doc"],
      "skills": ["web-search", "drafting"],
      "plugins": ["google-workspace"]
    }
  }
}
```

### How It Works

1. SDK embed mounts chat with `scope` param: `nebo.chat.mount(el, { scope: 'read' })`
2. Chat embed page reads `scope` from URL params and includes it in the `chat_message` payload
3. Dispatch passes `tool_scope` to the runner via `ChatConfig`
4. Runner restricts available tools/skills/plugins to those listed in the named scope

**Struct:** `ToolScope` in `crates/napp/src/agent.rs` — `tools: Vec<String>`, `skills: Vec<String>`, `plugins: Vec<String>`

Each scope is a named subset. When no scope is specified, all tools are available (default behavior).

---

## 27. Edge Cases

### Stale Sockets
`runtime.cleanup_stale(tool_dir)` removes leftover `.sock` and `.pid` files from previous crashes before launching.

### SvelteKit Apps
SvelteKit-based apps must set `paths.relative: true` in `svelte.config.js`. Without this, the client-side router fails to match routes when served from `/apps/{id}/ui/`.

### Manifest ID
The `id` field in `manifest.json` must match the agent directory name and DB agent ID. This ensures the socket filename matches what `proxy_to_sidecar` expects.

### Symlinked Agent Directories
During development, agent directories can be symlinked from a source repo to `~/.nebo/user/agents/`. The agent loader, UI server, and runtime all resolve symlinks correctly.

### No Sidecar Apps
Apps without a sidecar binary (pure frontend) work fine — they use the SDK's `agents.invoke()`, `janus.complete()`, and `storage` APIs. The `proxy_to_sidecar` route simply returns 503 if no binary is found.

---

## 28. Key Files

| Component | File |
|-----------|------|
| Lifecycle management | `crates/server/src/app_lifecycle.rs` |
| HTTP handlers | `crates/server/src/handlers/apps.rs` |
| Route definitions | `crates/server/src/routes/apps.rs` |
| gRPC proto (UIService) | `proto/apps/v0/ui.proto` |
| gRPC proto (Gateway) | `proto/apps/v0/gateway.proto` |
| gRPC proto (Hooks) | `proto/apps/v0/hooks.proto` |
| gRPC proto (Schedule) | `proto/apps/v0/schedule.proto` |
| Manifest parsing | `crates/napp/src/manifest.rs` |
| Runtime / process launch | `crates/napp/src/runtime.rs` |
| Binary validation / sandbox | `crates/napp/src/sandbox.rs` |
| Restart policy | `crates/napp/src/supervisor.rs` |
| Agent loader | `crates/napp/src/agent_loader.rs` |
| Agent config (scopes, tools, memory) | `crates/napp/src/agent.rs` |
| Sidecar tool routing | `crates/tools/src/sidecar_tool.rs` |
| Server startup (sidecar restore) | `crates/server/src/lib.rs` |
| Agent redaction | `crates/server/src/handlers/agents.rs` |
| TypeScript SDK | `@neboai/app-sdk` (npm); IIFE served from `app/node_modules/@neboai/app-sdk/dist/nebo.global.js` |
| SDK — Identity module | `@neboai/app-sdk` — `src/identity.ts` |
| SDK — Chat mount module | `@neboai/app-sdk` — `src/chat.ts` |
| SDK — Surfaces module | `@neboai/app-sdk` — `src/surfaces.ts` |
| SDK — WebSocket module | `@neboai/app-sdk` — `src/websocket.ts` |
| Frontend launcher | `app/src/lib/apps/launcher.ts` |
| Apps listing page | `app/src/routes/apps/+page.svelte` |
| App wrapper route | `app/src/routes/app/[agentId]/+page.svelte` |
| Chat embed page | `app/src/routes/(embed)/chat-embed/[agentId]/+page.svelte` |
