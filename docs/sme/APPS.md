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
24. [Edge Cases](#24-edge-cases)
25. [Key Files](#25-key-files)

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
packages/app-sdk/                     — TypeScript SDK for app frontends
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
├── data/                 # Auto-created — app-scoped data directory
└── views.json            # Optional — A2UI view definitions
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
3. **Validate binary**: regular file, not symlink, executable, valid magic bytes (ELF/Mach-O/PE), ≤ 500MB
4. **Sanitize environment**: clear all env vars, set only:
   - `NEBO_APP_ID`, `NEBO_APP_NAME`, `NEBO_APP_VERSION`
   - `NEBO_APP_DIR`, `NEBO_APP_SOCK`, `NEBO_APP_DATA`
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
    child: tokio::process::Child,
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
}
```

### Lifecycle Methods

- `new(agent_id, tool_dir, hub)` — creates runtime, supervisor, cancellation token
- `launch()` — cleans stale sockets, launches process, starts health checker, broadcasts `app_started`
- `shutdown()` — cancels health checker, stops process, broadcasts `app_stopped`

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

### serve_app_ui

```
GET /apps/{agent_id}/ui/{*path}
```

Serves static files from the app's `ui/` directory. Sanitizes path (rejects `..`). Falls back to `index.html` for SPA routing. Caches immutable assets; `index.html` is never cached.

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

Routes are merged into the `/api/v1` namespace via `api_routes()`. The UI serving route is additionally mounted at the top level (outside `/api/v1`) so apps can load at `/apps/{id}/ui/index.html`.

```
/apps/{agent_id}/ui/{*path}          → serve_app_ui
/apps/{agent_id}/api/{*path}         → proxy_to_sidecar (ANY method)
/apps/{agent_id}/agents/invoke       → invoke_agent (POST)
/apps/{agent_id}/agents/stream       → stream_agent (POST)
/apps/{agent_id}/janus/complete      → janus_complete (POST)
/apps/{agent_id}/janus/stream        → janus_stream (POST)
/apps/{agent_id}/storage             → list_storage (GET)
/apps/{agent_id}/storage/{key}       → get/put/delete_storage
/apps/{agent_id}/http/proxy          → http_proxy (POST)
/apps/{agent_id}/identity            → get_identity (GET)
/sdk/nebo.global.js                  → serve SDK IIFE
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

Auto-creates the plugin registry entry on first write. Values are JSON-serialized strings in the DB.

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

**File:** `app/src/routes/app/[agentId]/chat-embed/+page.svelte`

A minimal SvelteKit page that:
1. Reads `agentId` from URL params
2. Connects to the main Nebo WebSocket (`/ws`)
3. Creates a session with key `app:{agentId}:chat`
4. Renders the existing `ChatPane` component
5. Accepts configuration via URL params (`placeholder`, `theme`, `borderless`)
6. Implements the postMessage bridge protocol

**Layout:** `app/src/routes/app/[agentId]/chat-embed/+layout.svelte` — bare layout, no sidebar/nav/header.

### postMessage Bridge Protocol

**Parent → Iframe:**

| Message | Fields | Description |
|---------|--------|-------------|
| `nebo:send` | `message: string` | Programmatic send |
| `nebo:new-thread` | — | Start fresh conversation |
| `nebo:configure` | `options: { placeholder? }` | Update configuration |

**Iframe → Parent:**

| Message | Fields | Description |
|---------|--------|-------------|
| `nebo:ready` | — | Iframe loaded and connected |
| `nebo:message-sent` | `message: string` | User sent a message |
| `nebo:response-complete` | `text: string` | Agent finished responding |
| `nebo:resize` | `height: number` | Content height changed |

### Session Management

- Session key: `app:{agent_id}:chat` — distinct from `app:{agent_id}:api` (used by `agents.invoke()`)
- Chat embed connects to the main WebSocket (`/ws`) and sends `chat_message` with this session key
- `rotate_chat()` (new thread) preserves old messages, starts fresh conversation
- History loads via existing `getChatMessages` API

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

**Package:** `packages/app-sdk/`

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
| `nebo.surfaces` | A2UI event streaming | `src/surfaces.ts` |
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

---

## 19. Frontend Integration

### Chat Embed Route (`app/src/routes/app/[agentId]/chat-embed/+page.svelte`)

Standalone page that renders `ChatPane` in a bare layout (no sidebar, nav, or header). Used as the iframe source for `nebo.chat.mount()`. Connects to the main WebSocket, uses session key `app:{agentId}:chat`, and implements the postMessage bridge protocol. Accepts configuration via URL params (`placeholder`, `theme`, `borderless`).

Layout: `app/src/routes/app/[agentId]/chat-embed/+layout.svelte` — renders only children, no chrome.

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

Dual-mode page:
- **Normal agent:** renders A2UI views
- **App agent (`isApp`):** renders iframe (`/apps/{agentId}/ui/index.html`) + collapsible chat panel

The collapsible chat panel in the wrapper uses the existing `ChatPane` component connected via WebSocket with session key `agent:{agentId}:web`. This is separate from the embedded chat (`app:{agentId}:chat`) which apps mount via `nebo.chat.mount()` inside their own UI.

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

**Allowlisted system:**
- `PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL`, `TZ`

**Blocked (never passed through):**
- `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_API_KEY`
- `JWT_SECRET`, `DATABASE_URL`, `AWS_*`, `GITHUB_TOKEN`, `STRIPE_SECRET_KEY`

### Binary Validation

Before launching, binaries are validated:
- Must be a regular file (not symlink)
- Must be executable (Unix mode `& 0o111 != 0`)
- Must have valid magic bytes: ELF (`7f454c46`), Mach-O 64 (`feedfacf`), Mach-O 32 (`feedface`), PE (`4d5a`)
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
      → create AppLifecycle::new(agent_id, tool_dir, hub)
      → lifecycle.launch().await
      → store in app_lifecycles map
  → connect to socket via tonic gRPC
  → proxy request → response
```

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

## 24. Edge Cases

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

## 25. Key Files

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
| TypeScript SDK | `packages/app-sdk/src/` |
| SDK — Identity module | `packages/app-sdk/src/identity.ts` |
| SDK — Chat mount module | `packages/app-sdk/src/chat.ts` |
| Frontend launcher | `app/src/lib/apps/launcher.ts` |
| Apps listing page | `app/src/routes/apps/+page.svelte` |
| App wrapper route | `app/src/routes/app/[agentId]/+page.svelte` |
| Chat embed page | `app/src/routes/app/[agentId]/chat-embed/+page.svelte` |
| Chat embed layout | `app/src/routes/app/[agentId]/chat-embed/+layout.svelte` |
