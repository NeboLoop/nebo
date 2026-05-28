# Apps (`@org/agents/name` with `artifact_type: "app"`)

An App is an agent with its own UI. It bundles a persona, an HTML frontend, and an optional native sidecar binary into a standalone application that opens in its own window. Apps are the right choice when chat output isn't enough — dashboards, contact managers, journals, deal trackers.

For packaging format and `manifest.json`, see [Packaging](packaging.md). For deep technical details, see `docs/sme/APPS.md`.

---

## When to Build an App vs. a Skill

| Need | Build | Why |
|------|-------|-----|
| Teach the agent a new skill via instructions | Skill | No UI needed — chat output suffices |
| Visual dashboard with charts and tables | App | Needs rendered UI |
| Contact list with search and edit | App | Interactive CRUD UI |
| Journal with custom layout | App | Persistent visual state |
| Email drafting with templates | Skill | Chat + tool calls handle it |

**Rule of thumb:** If the user needs to *see and interact with* a dedicated interface, build an app. If chat bubbles and tool calls are enough, write a skill.

---

## App Directory Structure

```
my-app/
├── AGENT.md              # Required — persona and instructions
├── manifest.json         # Required — identity, permissions, window config
├── agent.json            # Optional — workflows, skills, user inputs
├── ui/                   # Required — static frontend files
│   ├── index.html        #   Entry point
│   ├── style.css
│   └── app.js
├── skills/               # Optional — skill docs that teach the agent how to use tools
│   ├── workspace-mgmt/
│   │   └── SKILL.md
│   └── document-analysis/
│       └── SKILL.md
├── sidecar/              # Optional — native backend (Rust recommended)
│   ├── Cargo.toml
│   ├── src/main.rs
│   └── target/release/
│       └── my-app-sidecar
└── ~/.nebo/appdata/agents/{id}/  # Auto-created at runtime — persistent data (separate from code)
```

### What Each File Does

| File | Purpose |
|------|---------|
| `AGENT.md` | The agent's persona. Written in markdown. Determines how the agent responds when invoked from the app. |
| `manifest.json` | Identity, version, permissions, window dimensions. This is what Nebo reads to know it's an app. |
| `agent.json` | Operational wiring — workflows, skill references, event bindings, user inputs. Same format as any agent. |
| `skills/` | SKILL.md files that teach the agent *how* and *when* to use the sidecar tools. Loaded automatically at launch. |
| `ui/index.html` | Entry point for the app's frontend. Served at `/apps/{agent_id}/ui/index.html`. |
| `sidecar/` | Optional native binary. Runs as a gRPC service on a Unix socket. Nebo proxies requests to it. |

---

## manifest.json

```json
{
  "id": "deal-tracker",
  "name": "@acme/agents/deal-tracker",
  "version": "1.0.0",
  "description": "Track real estate deals with AI-powered analysis.",
  "artifact_type": "app",
  "permissions": [
    "storage:readwrite",
    "subagent:invoke",
    "network:outbound"
  ],
  "window": {
    "title": "Deal Tracker",
    "width": 1024,
    "height": 768,
    "resizable": true
  }
}
```

### Required Fields

| Field | Description |
|-------|-------------|
| `id` | Unique identifier. Must match the directory name. |
| `name` | Qualified name (`@org/agents/name`) or display name. |
| `version` | Semantic version (`1.0.0`). |
| `artifact_type` | Must be `"app"`. |

### Optional Fields

| Field | Description |
|-------|-------------|
| `description` | One-line description shown in the marketplace and apps page. |
| `permissions` | Array of permission strings (see below). |
| `window` | Default window dimensions and title. |

### Permissions

Permissions use `prefix:scope` format. Declare only what your app needs.

| Permission | What It Grants |
|------------|---------------|
| `storage:readwrite` | Read and write to the app's scoped KV store |
| `subagent:invoke` | Invoke other agents via the SDK |
| `network:outbound` | Make HTTP requests through the proxy |
| `filesystem:read` | Read files from the user's system |
| `shell:execute` | Run shell commands |
| `memory:read` | Read agent memories |
| `oauth:google` | OAuth flow for Google services |

### Window Config

| Field | Default | Description |
|-------|---------|-------------|
| `title` | app name | Window title bar |
| `width` | 1024 | Default width (pixels) |
| `height` | 768 | Default height (pixels) |
| `resizable` | true | Allow user resize |

Nebo remembers window position and size per app — the user's last arrangement is restored on reopen.

---

## AGENT.md — The Persona

The `AGENT.md` defines how the agent behaves when invoked from the app. Same format as any agent persona.

```markdown
# Deal Tracker

You are a real estate deal analyst. When the user asks you to analyze a deal,
examine the financials, compare with market data, and provide a recommendation.

## Capabilities
- Analyze deal financials (cap rate, cash-on-cash, IRR)
- Compare properties against market comps
- Track deal pipeline stages
- Generate investment memos

## Guidelines
- Always show your math
- Flag deals with cap rates below 5% as high-risk
- Use conservative assumptions for projections
```

---

## Frontend (ui/)

The `ui/` directory contains your app's static frontend. Any framework works — React, Vue, Svelte, Solid, HTMX, or vanilla HTML/JS. Nebo serves these files as-is.

### Entry Point

`ui/index.html` is the entry point. In the Tauri desktop app, each app opens in its own window using the `neboapp://` custom protocol:

```
neboapp://{agent_id}/
```

This gives each app its own origin with `/` as the root URL. Your app's assets load from their natural paths (`/style.css`, `/app.js`, etc.) — no base URL configuration needed. Any framework works out of the box.

In the browser fallback, apps are served at `/apps/{agent_id}/ui/index.html`.

### Using the SDK

#### ES Module (React, Vue, Svelte, Solid)

```bash
npm install @neboai/app-sdk
```

```typescript
import { nebo } from '@neboai/app-sdk';

// Persistent storage (KV)
await nebo.storage.setItem('lastView', 'dashboard');
const view = await nebo.storage.getItem('lastView');

// Invoke the app's agent
const { text } = await nebo.agents.invoke('Analyze deal #42');

// Stream a response
for await (const chunk of nebo.agents.stream('Summarize pipeline')) {
  output.textContent += chunk.text;
}

// Direct LLM call (no agent persona)
const summary = await nebo.janus.complete({
  messages: [{ role: 'user', content: 'Summarize: ...' }],
  temperature: 0.3
});
```

#### Global SDK (HTMX, vanilla HTML)

```html
<script src="/sdk/nebo.global.js"></script>
<script>
  // nebo is available as a global
  async function loadData() {
    const data = await nebo.storage.getItem('contacts');
    // ...
  }
</script>
```

### SDK API Reference

#### Identity

Know who you are — fetch the agent's name, persona, skills, and configured inputs.

```typescript
const agent = await nebo.identity.get();
// => { id, name, displayName, description, persona, model, skills, inputValues }

// Clear cached identity (re-fetches on next get())
nebo.identity.invalidate();
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Agent ID |
| `name` | string | Agent name from AGENT.md |
| `displayName` | string | Human-readable name (window title > first heading > name) |
| `description` | string | One-line description |
| `persona` | string | AGENT.md body (markdown after frontmatter) |
| `model` | string | Configured model (e.g. `"claude-sonnet-4-20250514"`) |
| `skills` | string[] | Installed skill names |
| `inputValues` | object | User-configured input values |

#### Chat (Embedded)

Mount the full Nebo chat UI inside your app. The chat renders in an iframe with full feature parity — streaming, slash commands, tool visualization, voice, @mentions, ask widgets, markdown, code blocks. Works in any framework.

```typescript
// Mount chat into a DOM element
nebo.chat.mount(document.getElementById('chat'), {
  placeholder: 'Ask about your ads...',  // Custom placeholder
  theme: 'auto',                          // 'auto' | 'light' | 'dark'
  height: '400px',                        // CSS height
  borderless: false,                      // No border/shadow
  contextId: currentDoc.id                // Scope session per document
});

// Programmatic control
nebo.chat.send('Summarize today\'s performance');
nebo.chat.newThread();

// Set app context — the agent sees this with every message
nebo.chat.setContext({
  projectId: currentProject.id,
  displayedDoc: { filename: 'contract.pdf', documentId: 'doc-123' },
  route: '/projects/abc/documents',
});

// Clear context when user navigates away
nebo.chat.setContext(null);

// Listen for events from the chat
const unsub = nebo.chat.onMessage((msg) => {
  if (msg.type === 'nebo:response-complete') {
    updateDashboard(msg.text);
  }
});

// Cleanup
nebo.chat.unmount();
```

The embedded chat uses session key `agent:{agentId}:app` — separate from `nebo.agents.invoke()` which uses `agent:{agentId}:api`. Conversations persist across page reloads.

##### Document-Scoped Sessions (`contextId`)

By default, all chats share one session per agent. For apps that display different content (documents, projects, records), pass `contextId` to scope each conversation:

```typescript
// Each document gets its own persistent chat history
nebo.chat.mount(chatContainer, {
  contextId: document.id  // → session key: agent:{id}:app:{contextId}
});
```

When the user switches documents, unmount and remount with the new `contextId`. Each context maintains its own conversation — messages from one document don't leak into another.

**Memory isolation with `contextId`:** By default, all contexts share the agent's memory pool. To isolate memories per context (so Client A's facts never appear in Client B's chat), add `"memory": { "context_isolated": true }` to `agent.json`. See [Agents — Memory](agents.md#memory).

##### Chat Context

The embedded chat is an iframe — it has no visibility into your app's state. Without context, when a user asks "tell me about this file," the agent has no idea what "this file" refers to.

`chat.setContext()` solves this by injecting app state as invisible context into every agent request. The context is not rendered in the chat UI — it's added as a system-level message that the LLM sees but the user doesn't.

```typescript
// Call setContext whenever the user's view changes
function onProjectSelected(project: Project) {
  nebo.chat.setContext({
    projectId: project.id,
    displayedDoc: null,
    route: `/projects/${project.id}`,
  });
}

function onDocumentOpened(doc: Document) {
  nebo.chat.setContext({
    projectId: currentProject.id,
    displayedDoc: { filename: doc.filename, documentId: doc.id },
    attachedDocuments: selectedDocs.map(d => ({
      filename: d.filename,
      documentId: d.id,
    })),
    route: `/projects/${currentProject.id}/documents/${doc.id}`,
  });
}
```

**`ChatContext` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `projectId` | string | Current project the user is viewing |
| `displayedDoc` | `{ filename, documentId }` | Document currently visible in the viewer |
| `attachedDocuments` | `{ filename, documentId }[]` | Documents explicitly selected/attached |
| `route` | string | Current page/route within the app |
| `[key: string]` | unknown | Arbitrary app-specific data |

All fields are optional. Include only what's relevant to your app. The agent receives the context as `App context: { ... }` prepended to the request.

##### Context Merging

Apps can also send context directly with chat messages (e.g. `{ context: "User viewing doc #123" }`). When the message includes both app context from `setContext()` and mention context from `@agent` references, these are merged into a single system message in the agent prompt. The combined context is invisible to the user but provides situational awareness to the agent.

#### Storage

Scoped KV store — persists across app restarts.

```typescript
nebo.storage.setItem(key: string, value: any): Promise<void>
nebo.storage.getItem(key: string): Promise<any | null>
nebo.storage.removeItem(key: string): Promise<void>
nebo.storage.keys(): Promise<string[]>
nebo.storage.clear(): Promise<void>
```

#### Agents

Invoke any agent in the user's workspace.

```typescript
// Synchronous (wait for full response)
nebo.agents.invoke(message: string, options?: {
  agent?: string,    // Override: invoke a different agent
  data?: any         // Structured context passed to the agent
}): Promise<{ text: string; tools: any[] }>

// Streaming
nebo.agents.stream(message: string, options?: {
  agent?: string,
  data?: any
}): AsyncGenerator<{ text: string; done: boolean }>
```

#### Janus (Direct LLM)

Call the LLM directly — no agent persona, no memory, no tool use.

```typescript
nebo.janus.complete(options: {
  messages: Array<{ role: string; content: string }>,
  model?: string,        // e.g. "claude-sonnet-4-20250514"
  temperature?: number,
  max_tokens?: number,
  system?: string
}): Promise<string>

nebo.janus.stream(options): AsyncGenerator<string>
```

#### HTTP Proxy

Make CORS-free HTTP requests through Nebo's server.

```typescript
// Standard fetch — automatically routed through Nebo
const resp = await nebo.fetch('/apps/my-app/api/data');
```

#### WebSocket

Real-time connection to the app's agent.

```typescript
const ws = new nebo.WebSocket('/apps/my-app/ws');
ws.addEventListener('message', (evt) => { ... });
ws.send('event-name', { key: 'value' });
ws.close();
```

---

## App SDK (`@neboai/app-sdk`)

The App SDK package (`@neboai/app-sdk`) provides three standalone integration patterns for apps that need direct control over agent communication, beyond what the `nebo` global object offers.

### Surfaces API

Receives structured agent events without coupling to the Nebo chat UI. Use this when your app renders its own output and needs raw event data.

```typescript
import { NeboSurfaces } from '@neboai/app-sdk';

const surfaces = new NeboSurfaces();
surfaces.connect();

surfaces.on('text_content', (e) => {
  output.textContent += e.delta;
});

surfaces.on('state_snapshot', (e) => {
  appState = e.snapshot;
  rerender();
});

surfaces.on('state_delta', (e) => {
  // RFC 6902 JSON Patch operations
  applyPatch(appState, e.operations);
  rerender();
});

// Send events back to the agent
surfaces.send('button_click', { buttonId: 'analyze' });
```

**Event types:**

| Event | Description |
|-------|-------------|
| `run_started` | Agent run began |
| `text_content` | Incremental text delta from the agent |
| `tool_call_start` | Agent invoked a tool |
| `state_snapshot` | Full state replacement |
| `state_delta` | Incremental state update (RFC 6902 JSON Patch) |
| `surface_create` | New surface requested by the agent |

The SDK auto-maintains a shared `state` object updated from `state_snapshot` and `state_delta` events.

### Chat Embed

Mounts the full-featured Nebo chat UI inside your app via an iframe.

```typescript
import { chat } from '@neboai/app-sdk';

chat.mount(document.getElementById('chat'), {
  placeholder: 'Ask about this document...',
  theme: 'dark',         // 'auto' | 'light' | 'dark'
  height: '400px',
  borderless: false,
  contextId: 'doc-123',
  scope: 'read'
});

// Programmatic control
chat.send('summarize this');
chat.setContext({
  displayedDoc: { documentId: 'doc-123', filename: 'report.pdf' }
});
chat.onMessage((msg) => console.log(msg));
chat.unmount();
```

Options: `placeholder`, `theme`, `height`, `borderless`, `contextId`, `scope`. Context fields: `projectId`, `displayedDoc`, `attachedDocuments`, `route`.

### WebSocket

Direct WebSocket connection with auto-reconnect and exponential backoff (1s–30s).

```typescript
import { NeboWebSocket } from '@neboai/app-sdk';

const ws = new NeboWebSocket();
// Connects to ws://{base}/ws/app/{appId}
```

---

## Sidecar Binary (Optional)

The sidecar is a native binary that runs alongside the app. It serves API endpoints over gRPC via a Unix socket. Nebo proxies all requests from `/apps/{id}/api/*` to the sidecar.

### When You Need a Sidecar

| Scenario | Sidecar? | Alternative |
|----------|----------|-------------|
| CRUD API with local data | Yes | — |
| Complex data processing | Yes | — |
| External API integration | Maybe | `nebo.fetch` + HTTP proxy |
| AI-only features | No | `nebo.agents.invoke()` |
| Simple persistence | No | `nebo.storage` |

### How the SDK Reaches the Sidecar

The frontend never talks to the sidecar directly. Every request flows through Nebo's proxy, which converts HTTP to gRPC:

```
Frontend                  Nebo Server                    Sidecar
────────                  ───────────                    ───────
nebo.fetch('/projects')
  │
  ├─ SDK builds URL:
  │  {base}/api/v1/apps/{id}/api/projects
  │
  └──── HTTP GET ────────►  proxy_to_sidecar()
                              │
                              ├─ Extracts method, path,
                              │  query, headers, body
                              │
                              ├─ Connects to Unix socket
                              │  {app_dir}/{id}.sock
                              │
                              └──── gRPC HandleRequest ──►  UIService
                                    HttpRequest {              │
                                      method: "GET"            ├─ Routes path
                                      path: "projects"         │  to handler
                                      query: ""                │
                                      headers: {...}           ├─ Processes
                                      body: []                 │  request
                                    }                          │
                                                               └─ Returns
                              ◄──── HttpResponse ────────────────┘
                              HttpResponse {
                                status_code: 200
                                headers: {"content-type": "application/json"}
                                body: [{"id":"abc","name":"My Project",...}]
                              }
                              │
  ◄──── HTTP 200 ─────────────┘
  JSON body returned to caller
```

**Key detail:** The `path` field the sidecar receives is relative — stripped of the `/apps/{id}/api/` prefix. If the frontend calls `nebo.fetch('/projects/abc')`, the sidecar sees `path: "projects/abc"`.

### Calling the Sidecar from Frontend Code

Use `nebo.fetch()` — it mirrors the native `fetch()` API but auto-routes relative URLs to your sidecar:

```typescript
import { nebo } from '@neboai/app-sdk';

// GET — list resources
const resp = await nebo.fetch('/projects');
const projects = await resp.json();

// POST — create a resource
const resp = await nebo.fetch('/projects', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ name: 'New Project' })
});

// PUT — update a resource
await nebo.fetch('/projects/abc', {
  method: 'PUT',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ name: 'Updated Name' })
});

// DELETE — remove a resource
await nebo.fetch('/projects/abc', { method: 'DELETE' });

// Query strings work naturally
const resp = await nebo.fetch('/documents?project_id=abc&format=pdf');
```

`nebo.fetch()` returns a standard `Response` object — use `.json()`, `.text()`, `.blob()`, check `.ok`, `.status`, etc. exactly as you would with `fetch()`.

**URL routing rules:**
- Relative URLs (no scheme) → routed to your sidecar via `{base}/api/v1/apps/{id}/api{path}`
- Absolute URLs (`https://...`) → routed through Nebo's CORS-free HTTP proxy

### Handling Requests in the Sidecar

Your sidecar receives every proxied request as a gRPC `HandleRequest` call. The `HttpRequest` message contains `method`, `path`, `query`, `headers`, and `body`. You route them however you want.

**Typical pattern** — split path segments and match:

```rust
use proto::{HttpRequest, HttpResponse};

// Inside your UIService::handle_request implementation:
async fn handle_http(&self, method: &str, path: &str, query: &str, body: &[u8]) -> HttpResponse {
    let parts: Vec<&str> = path.trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    match (method, parts.as_slice()) {
        // GET /projects → list all projects
        ("GET", &["projects"]) => {
            let projects = self.state.list_projects().await;
            json_response(200, &projects)
        }
        // POST /projects → create a project
        ("POST", &["projects"]) => {
            let req: CreateRequest = serde_json::from_slice(body)?;
            let project = self.state.create_project(req).await;
            json_response(201, &project)
        }
        // GET /projects/{id} → get one project
        ("GET", &["projects", id]) => {
            match self.state.get_project(id).await {
                Some(p) => json_response(200, &p),
                None => json_response(404, &json!({"error": "not found"})),
            }
        }
        // PUT /projects/{id} → update a project
        ("PUT", &["projects", id]) => {
            let req: UpdateRequest = serde_json::from_slice(body)?;
            let project = self.state.update_project(id, req).await;
            json_response(200, &project)
        }
        // DELETE /projects/{id} → delete a project
        ("DELETE", &["projects", id]) => {
            self.state.delete_project(id).await;
            json_response(204, &json!({}))
        }
        _ => json_response(404, &json!({"error": "not found"}))
    }
}

fn json_response<T: serde::Serialize>(status: i32, data: &T) -> HttpResponse {
    HttpResponse {
        status_code: status,
        headers: HashMap::from([("content-type".into(), "application/json".into())]),
        body: serde_json::to_vec(data).unwrap_or_default(),
    }
}
```

For larger apps, split routes into modules that each try to match and return `Option<HttpResponse>`:

```rust
// routes/projects.rs
pub async fn handle(state: &AppState, method: &str, parts: &[&str], body: &[u8]) -> Option<HttpResponse> {
    match (method, parts) {
        ("GET", &["projects"]) => Some(list(state).await),
        ("POST", &["projects"]) => Some(create(state, body).await),
        ("GET", &["projects", id]) => Some(get(state, id).await),
        _ => None,
    }
}

// main.rs — chain route modules
if let Some(resp) = routes::projects::handle(&self.state, method, &parts, body).await {
    return resp;
}
if let Some(resp) = routes::documents::handle(&self.state, method, &parts, body).await {
    return resp;
}
// ... fallback
json_response(404, &json!({"error": "not found"}))
```

### The gRPC Contract

Your sidecar must implement the `UIService` gRPC service defined in `proto/apps/v0/ui.proto`:

```protobuf
service UIService {
  rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
  rpc Configure(SettingsMap) returns (Empty);
  rpc HandleRequest(HttpRequest) returns (HttpResponse);
}

message HttpRequest {
  string method = 1;               // GET, POST, PUT, DELETE, etc.
  string path = 2;                 // Path relative to /apps/{id}/api/
  string query = 3;                // Raw query string (e.g. "page=1&size=10")
  map<string, string> headers = 4; // Request headers
  bytes body = 5;                  // Request body (empty for GET/HEAD)
}

message HttpResponse {
  int32 status_code = 1;           // HTTP status code (200, 404, 500, etc.)
  map<string, string> headers = 2; // Response headers
  bytes body = 3;                  // Response body
}
```

**Required RPCs:**

| RPC | Purpose |
|-----|---------|
| `HealthCheck` | Return `healthy: true`, `version`, and `name`. Called by Nebo to verify the sidecar is alive. |
| `HandleRequest` | Process an HTTP request and return an HTTP response. This is where all your app logic lives. |
| `Configure` | Receive settings updates. Can be a no-op if your app doesn't use configuration. |

### Sidecar Startup

Your sidecar binary must:
1. Read `$NEBO_APP_SOCK` for the socket path
2. Read `$NEBO_APP_DATA` for the writable data directory
3. Bind a Unix socket at the `$NEBO_APP_SOCK` path
4. Serve the `UIService` gRPC service on that socket

Minimal Rust example:

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sock_path = std::env::var("NEBO_APP_SOCK")
        .unwrap_or_else(|_| "/tmp/my-app.sock".into());
    let data_dir = std::env::var("NEBO_APP_DATA")
        .unwrap_or_else(|_| "data".into());

    // Clean stale socket
    let _ = std::fs::remove_file(&sock_path);

    let service = MyAppService::new(&data_dir);

    let uds = tokio::net::UnixListener::bind(&sock_path)?;
    let uds_stream = tokio_stream::wrappers::UnixListenerStream::new(uds);

    tonic::transport::Server::builder()
        .add_service(UiServiceServer::new(service))
        .serve_with_incoming(uds_stream)
        .await?;

    Ok(())
}
```

### Environment Variables

Your sidecar runs in a sandboxed environment with only these variables:

| Variable | Example | Description |
|----------|---------|-------------|
| `NEBO_APP_ID` | `deal-tracker` | Agent identifier |
| `NEBO_APP_NAME` | `Deal Tracker` | Display name |
| `NEBO_APP_VERSION` | `1.0.0` | Manifest version |
| `NEBO_APP_DIR` | `/Users/me/.nebo/user/agents/deal-tracker` | App root directory |
| `NEBO_APP_SOCK` | `...deal-tracker/deal-tracker.sock` | Unix socket path |
| `NEBO_APP_DATA` | `~/.nebo/appdata/agents/deal-tracker` | Writable data directory (separate from code — survives upgrades) |
| `PATH` | system path | Standard path |
| `HOME` | user home | Home directory |

API keys, database URLs, and secrets are **never** passed to sidecars.

### Data Persistence

The sidecar owns its own data in `$NEBO_APP_DATA`. Common approaches:

- **JSON file** — simplest. Load at startup, save after mutations. Works well for small datasets (< 10MB).
- **SQLite** — use for structured data, queries, or anything beyond trivial CRUD.
- **File store** — store blobs (uploaded documents, images) as files in the data directory.

The data directory is physically separated from the code directory — it lives at `~/.nebo/appdata/agents/{id}/`, not inside the app's code tree. This means you can safely upgrade or reinstall the app binary without touching your data. The data directory survives sidecar restarts, app updates, reinstalls, and Nebo upgrades. It follows the iOS model: the update system physically cannot reach the data container.

### Binary Location

Place your compiled binary in one of these locations (checked in order):

1. `{app_dir}/binary` — single named file
2. `{app_dir}/app` — single named file
3. `{app_dir}/bin/` — first file in directory
4. `{app_dir}/sidecar/target/release/` — first executable (Rust dev builds)

For production distribution, use `bin/`. For development, `sidecar/target/release/` is detected automatically.

### Startup Timeout

The sidecar must create the Unix socket within the `startup_timeout` (default 10 seconds, max 120). If the socket doesn't appear, the launch fails and the next request retries.

### Health Checking & Auto-Restart

Nebo checks your sidecar every 15 seconds:

- **Crash recovery** — if the process dies, Nebo broadcasts `app_crashed` and auto-restarts with exponential backoff (10s, 20s, 40s, 80s, 160s, max 5min). Maximum 5 crash restarts per hour.
- **Binary hot-reload** — if the binary on disk changes (e.g. you rebuild), Nebo gracefully stops the running process, unregisters old tools, restarts the process, and re-discovers tools from the new binary. This is not a crash — no backoff, no limit, immediate restart. Symlinks are followed, so `bin/my-app → sidecar/target/release/my-app` works. Dev workflow: rebuild your binary and the server auto-detects the change for a seamless tool update.
- Broadcasts `app_restarted` after recovery (includes `reason: "binary_changed"` for hot-reloads).

### Lifecycle

Nebo owns the full lifecycle of every sidecar. When Nebo shuts down (SIGTERM, Ctrl+C, or app quit), it sends SIGTERM to every running sidecar and waits for them to exit before the process ends. Sidecars should handle SIGTERM gracefully — flush data, close connections, then exit.

You do not need to manage sidecar lifetime yourself. Nebo handles:
- **Auto-launch** — the first API request to a sidecar triggers launch if it's not already running
- **Restore on server restart** — previously-running sidecars are detected from the `agent_workflows` table and auto-relaunched when the server starts, so there is no user-visible breakage after a server restart
- **Health checks** — every 15 seconds
- **Crash recovery** — auto-restart with exponential backoff
- **Hot-reload** — restart on binary change (no backoff)
- **Shutdown** — SIGTERM on Nebo exit

### Tool Discovery (`GET /_tools`)

When a sidecar starts, Nebo queries `GET /_tools` to discover what API endpoints the sidecar exposes. If the sidecar responds with tool definitions, Nebo registers them as LLM-callable tools — the agent can then call your sidecar's API directly during conversations.

This is optional. If your sidecar doesn't implement `/_tools`, the agent can still be used via the chat embed and SDK, but it won't be able to call your API endpoints as tools during LLM reasoning.

#### Implementing `/_tools`

Add a `/_tools` route to your `HandleRequest` handler that returns a JSON array of tool definitions:

```rust
// In your handle_http match block:
("GET", &["_tools"]) => {
    let tools = serde_json::json!([
        {
            "name": "list_projects",
            "description": "List all projects for the current user",
            "method": "GET",
            "path": "/projects"
        },
        {
            "name": "create_project",
            "description": "Create a new project with a name and optional description",
            "method": "POST",
            "path": "/projects",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Project name" },
                    "description": { "type": "string", "description": "Optional description" }
                },
                "required": ["name"]
            }
        },
        {
            "name": "get_project",
            "description": "Get a single project by ID",
            "method": "GET",
            "path": "/projects/{id}"
        },
        {
            "name": "delete_project",
            "description": "Delete a project by ID",
            "method": "DELETE",
            "path": "/projects/{id}"
        }
    ]);
    json_response(200, &tools)
}
```

#### Tool Definition Schema

Each tool in the array has these fields:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Action name the LLM uses (e.g. `"list_projects"`) |
| `description` | string | Yes | What this action does — shown to the LLM |
| `method` | string | Yes | HTTP method: `GET`, `POST`, `PUT`, `DELETE` |
| `path` | string | Yes | Path relative to sidecar root (e.g. `"/projects"`, `"/projects/{id}"`) |
| `input_schema` | object | No | JSON Schema for the request body. Omit for GET/DELETE. |

#### Path Parameters

Use `{param}` placeholders in the path. When the LLM calls the tool, Nebo extracts matching keys from the input and substitutes them:

```json
{
    "name": "get_document",
    "description": "Get a document by ID",
    "method": "GET",
    "path": "/documents/{id}"
}
```

The LLM calls: `get_document(id: "doc-123")` → Nebo sends `GET /documents/doc-123` to the sidecar.

#### How It Works

1. Sidecar starts → Nebo calls `GET /_tools` via gRPC `HandleRequest`
2. Sidecar returns `200` with JSON array of tool definitions
3. Nebo registers each tool **per-agent** in the global tool registry via `register_for_agent(agent_id, tool)` — the LLM sees `list_projects(...)`, `get_document(id: "...")`, etc. directly
4. For `GET` requests, non-path input parameters are sent as query strings
5. For `POST`/`PUT` requests, input parameters (minus path params) are sent as the JSON body
6. Tool discovery runs again automatically when the sidecar restarts (crash recovery or binary hot-reload)
7. On sidecar shutdown, all registered tools are cleaned up via `unregister_agent_tools(agent_id)`

Sidecar tools bypass the contextual tool filter — they are always included for their owning agent regardless of conversation context. Tool calls are routed to the sidecar through `GrpcSidecarCaller`, which translates tool invocations into gRPC `HandleRequest` calls on the Unix socket.

**Tools are not enough on their own.** The LLM knows it *can* call `list_projects`, but it doesn't know *when* or *how* to use it effectively. That's what skills are for — see [Skills](#skills) below.

#### Tips

- Keep descriptions clear and concise — the LLM uses them to decide which tool to call
- Include `input_schema` for POST/PUT actions so the LLM knows what parameters to provide
- Path parameter names in `input_schema.properties` should match the `{placeholder}` in the path
- Return `404` or an empty array `[]` from `/_tools` if your sidecar has no tools — Nebo handles both gracefully
- Avoid tool names that collide with Nebo's core tools (`agent`, `skill`, `event`, `message`, `web`, `os`)

### Skills

Tools give the agent the *ability* to call sidecar endpoints. Skills teach the agent *when and how* to use them. Without skills, the agent has tools it doesn't understand — it can call `list_projects` but has no idea when to do so, what the results mean, or how to combine multiple tool calls into a workflow.

#### Directory Structure

```
my-app/
├── skills/
│   ├── workspace-management/
│   │   └── SKILL.md
│   ├── document-analysis/
│   │   └── SKILL.md
│   └── data-export/
│       └── SKILL.md
```

Each skill is a directory containing a `SKILL.md` file. The file has YAML frontmatter followed by markdown instructions.

#### SKILL.md Format

```markdown
---
name: workspace-management
description: Manage projects, documents, and folders
triggers:
  - create a project
  - list projects
  - upload document
  - workspace stats
---
# Workspace Management

Tools for managing projects, documents, and folders.

## list_projects
List all projects for the current user.
- **Method:** GET /projects
- Returns: Array of project objects with id, name, description, created_at

## create_project
Create a new project.
- **Method:** POST /projects
- **name** (string, required): Project name
- **description** (string, optional): Project description
- Returns: Created project object

## get_document_text
Get the extracted text content of a document.
- **Method:** GET /documents/{id}/text
- Returns: Full text with [Page N] markers for PDFs
- Use this when the user asks about document content
```

**Key rules:**
- `name:` in frontmatter must be unique and match the reference in `agent.json`
- `triggers:` are keywords that activate the skill when mentioned in conversation
- The markdown body teaches the LLM how each tool works, what parameters to pass, and what to expect back
- Document business logic, not just API schemas — explain *when* to use each tool

#### Referencing Skills in agent.json

```json
{
  "skills": [
    "skills/workspace-management",
    "skills/document-analysis",
    "skills/data-export"
  ]
}
```

Nebo extracts the last path segment (e.g. `"workspace-management"`) and matches it against the `name:` field in each SKILL.md frontmatter.

#### Loading

Skills are loaded automatically when the app sidecar launches via `skill_loader.load_app_skills(tool_dir)`, using the same loader as all Nebo agents. They appear in the system prompt when triggered by conversation context or auto-loaded for the active agent. Skills are unloaded when the sidecar shuts down.

Apps can bundle skills in a `skills/` directory alongside the sidecar. Loading priority (higher overrides lower):

1. Bundled skills (built into Nebo)
2. Installed `.napp` skills (from marketplace)
3. User skill files (manually created)
4. **App skills** (from the app's `skills/` directory)

### Tool Scoping

By default, all sidecar tools and skills are available in every embed chat context. Tool scoping lets you restrict which tools, skills, and plugins are active based on where the chat is mounted.

#### Defining Scopes in agent.json

```json
{
  "skills": ["skills/workspace-management", "skills/document-editing"],
  "requires": { "plugins": ["gws"] },
  "scopes": {
    "editor": {
      "tools": ["get_document", "update_document", "get_comments", "add_comment"],
      "skills": ["skills/document-editing"]
    },
    "projects": {
      "tools": ["list_projects", "create_project", "search_documents"],
      "skills": ["skills/workspace-management"],
      "plugins": ["gws"]
    }
  }
}
```

Each scope declares:
- **tools** — which sidecar tool names are active (subset of what `/_tools` returns)
- **skills** — which skill refs to load into the prompt (subset of top-level `skills` array)
- **plugins** — additional plugins to pre-activate (merged with global `requires.plugins`)

#### Using Scopes from the SDK

```typescript
// Document editing view — only document tools + skills
nebo.chat.mount(container, {
  contextId: doc.id,
  scope: 'editor'
});

// Project overview — different tools + skills + plugins
nebo.chat.mount(container, {
  contextId: 'projects',
  scope: 'projects'
});

// No scope — all tools/skills/plugins available (default)
nebo.chat.mount(container, {
  contextId: 'general'
});
```

#### Behavior

| SDK `scope` | Tools | Skills | Plugins |
|-------------|-------|--------|---------|
| Not set | All sidecar tools | All agent.json skills | All requires.plugins |
| `"editor"` | Only scope.tools | Only scope.skills | requires.plugins + scope.plugins |
| Unknown name | Warning logged, falls back to "not set" | Same | Same |

Core system tools (memory, scheduling, etc.) are always available regardless of scope. The scope only controls your app's sidecar tools.

When a scope is active, the runner limits available tools, skills, and plugins to exactly what the scope definition declares. This is enforced server-side — the SDK `scope` parameter is passed with the chat embed and the runner filters accordingly.

**Use case:** read-only access in a public embed (`scope: "read"`) vs. full access in an authenticated view (`scope: "write"`).

#### Why Scope?

- **Context window** — 18 tools x ~500 tokens each = 9K tokens. Scoping to 4 tools saves ~7K tokens per request
- **LLM accuracy** — fewer, relevant tools = better tool selection decisions
- **Skill relevance** — only inject skill docs for tools that are available
- **Publisher control** — you decide exactly what the agent can do in each context

### Logging

Sidecar stdout and stderr are captured to `{app_dir}/data/sidecar.log` (append mode). Check this file when debugging startup issues.

### App Agent Redaction

For agents with `isApp=true`, the API automatically strips sensitive fields from responses to prevent leaking agent internals to end users. The persona, skills list, and frontmatter are not exposed to the client. Only UI-needed fields are returned: name, avatar, and status.

This is transparent — no configuration needed. Any API call that returns agent details for an app agent receives the redacted response.

---

## Framework Notes

The `neboapp://` custom protocol gives each app its own origin at `/`, so all frameworks work without special base URL configuration.

### SvelteKit

Use `adapter-static` with output to `../ui`:

```javascript
// svelte.config.js
import adapter from '@sveltejs/adapter-static';

const config = {
  kit: {
    adapter: adapter({
      pages: '../ui',
      assets: '../ui',
      fallback: 'index.html',
      strict: false
    })
  }
};

export default config;
```

### HTMX

HTMX apps work natively. The SDK bridge injects `<meta name="htmx-config" content='{"selfRequestsOnly":false}'>` automatically so HTMX can make requests to the Nebo server. Use the global SDK (`/sdk/nebo.global.js`) for storage and agent invocation.

### React / Vue / Solid / Vanilla

No special configuration needed. Build your app normally and place the output in `ui/`.

---

## Complete Example: Journal App

### manifest.json

```json
{
  "id": "journal",
  "name": "@nebo/agents/journal",
  "version": "1.0.0",
  "description": "AI-powered journal with reflection prompts.",
  "artifact_type": "app",
  "permissions": ["storage:readwrite", "subagent:invoke"],
  "window": {
    "title": "Journal",
    "width": 700,
    "height": 800,
    "resizable": true
  }
}
```

### AGENT.md

```markdown
# Journal

You are a thoughtful journaling companion. When the user writes an entry,
read it carefully and offer a brief, insightful reflection. Don't
summarize — add depth. Ask one follow-up question that helps them
think more deeply about what they wrote.
```

### ui/index.html (HTMX + global SDK)

```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8" />
  <meta name="nebo-app-id" content="journal" />
  <script src="/sdk/nebo.global.js"></script>
</head>
<body>
  <h1>Journal</h1>
  <textarea id="entry" placeholder="What's on your mind?"></textarea>
  <button onclick="reflect()">Reflect</button>
  <div id="reflection"></div>

  <!-- Full embedded chat with the journal agent -->
  <div id="chat"></div>

  <script>
    // Mount the embedded chat
    nebo.chat.mount(document.getElementById('chat'), {
      placeholder: 'Reflect on your day...',
      height: '300px'
    });

    // Use identity to personalize the UI
    nebo.identity.get().then(agent => {
      document.querySelector('h1').textContent = agent.displayName;
    });

    async function reflect() {
      const entry = document.getElementById('entry').value;
      const { text } = await nebo.agents.invoke(entry);
      document.getElementById('reflection').textContent = text;
      await nebo.storage.setItem('last-entry', entry);
    }
  </script>
</body>
</html>
```

This is a complete app — no sidecar needed. The agent provides AI reflection, storage persists the last entry, the embedded chat gives the user a full conversational interface, and identity lets the UI adapt to the agent's configuration.

---

## Development Workflow

### 1. Create the directory

```bash
mkdir -p ~/.nebo/user/agents/my-app/ui
```

### 2. Write manifest.json, AGENT.md, and ui/index.html

See examples above.

### 3. Symlink from source (recommended for development)

```bash
# Work from a source repo, symlink into Nebo
ln -s /path/to/my-app ~/.nebo/user/agents/my-app
```

The filesystem watcher detects new symlinks automatically — the app appears in the Apps tab within seconds. Changes to your source directory take effect immediately — no copy step, no restart.

### 4. Build the sidecar (if needed)

```bash
cd my-app/sidecar
cargo build --release
```

The binary at `sidecar/target/release/` is detected automatically.

### 5. Open the app

Navigate to the Apps tab in Nebo and click your app. The sidecar launches on first API request.

### 6. Iterate

- Frontend changes: edit `ui/` files, refresh the app window
- Sidecar changes: rebuild the binary — Nebo detects the changed file and restarts the sidecar automatically (within 15 seconds)
- Agent changes: edit `AGENT.md`, changes are picked up on next invocation

---

## Publishing to NeboAI

### Via MCP Server

```
1. developer(resource: account, action: select, id: "your-dev-account-id")
2. agent(action: create, name: "deal-tracker", manifestContent: "# Deal Tracker\n...")
3. agent(action: binary-token, id: "AGENT_ID")
4. Upload binary via returned curl command (per platform)
5. agent(action: submit, id: "AGENT_ID", version: "1.0.0")
```

### Platforms

Upload a binary for each platform you support:

| Platform | Architecture |
|----------|-------------|
| `darwin-arm64` | macOS Apple Silicon |
| `darwin-amd64` | macOS Intel |
| `linux-arm64` | Linux ARM |
| `linux-amd64` | Linux x86_64 |
| `windows-arm64` | Windows ARM |
| `windows-amd64` | Windows x86_64 |

Apps without a sidecar (pure frontend) don't need binary uploads — just the manifest and UI files.
