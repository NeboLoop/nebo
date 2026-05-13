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
├── agent.json            # Optional — workflows, skills, pricing
├── ui/                   # Required — static frontend files
│   ├── index.html        #   Entry point
│   ├── style.css
│   └── app.js
├── sidecar/              # Optional — native backend (Rust recommended)
│   ├── Cargo.toml
│   ├── src/main.rs
│   └── target/release/
│       └── my-app-sidecar
└── data/                 # Auto-created at runtime — app storage
```

### What Each File Does

| File | Purpose |
|------|---------|
| `AGENT.md` | The agent's persona. Written in markdown. Determines how the agent responds when invoked from the app. |
| `manifest.json` | Identity, version, permissions, window dimensions. This is what Nebo reads to know it's an app. |
| `agent.json` | Operational wiring — workflows, skills, event bindings, pricing. Same format as any agent. |
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
    "min_width": 480,
    "min_height": 400,
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
| `min_width` | — | Minimum resize width |
| `min_height` | — | Minimum resize height |
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
  borderless: false                       // No border/shadow
});

// Programmatic control
nebo.chat.send('Summarize today\'s performance');
nebo.chat.newThread();

// Listen for events from the chat
const unsub = nebo.chat.onMessage((msg) => {
  if (msg.type === 'nebo:response-complete') {
    updateDashboard(msg.text);
  }
});

// Cleanup
nebo.chat.unmount();
```

The embedded chat uses session key `app:{agentId}:chat` — separate from `nebo.agents.invoke()` which uses `app:{agentId}:api`. Conversations persist across page reloads.

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

### How It Works

1. User opens the app → frontend loads from `ui/`
2. Frontend calls `/apps/{id}/api/deals` (or any path)
3. Nebo converts the HTTP request to a gRPC `HandleRequest` call
4. Forwards to sidecar via Unix socket at `{app_dir}/{id}.sock`
5. Sidecar processes request, returns HTTP response
6. Nebo relays the response back to the frontend

```
App Frontend → /apps/{id}/api/* → Nebo Proxy → Unix Socket → Sidecar gRPC
```

### Sidecar Contract

Your sidecar must:
1. Implement the `UIService.HandleRequest` gRPC method
2. Listen on a Unix socket at `$NEBO_APP_SOCK`
3. Accept the proto format defined in `proto/apps/v0/ui.proto`

### Environment Variables

Your sidecar runs in a sandboxed environment with only these variables:

| Variable | Example | Description |
|----------|---------|-------------|
| `NEBO_APP_ID` | `deal-tracker` | Agent identifier |
| `NEBO_APP_NAME` | `Deal Tracker` | Display name |
| `NEBO_APP_VERSION` | `1.0.0` | Manifest version |
| `NEBO_APP_DIR` | `/Users/me/.nebo/user/agents/deal-tracker` | App root directory |
| `NEBO_APP_SOCK` | `...deal-tracker/deal-tracker.sock` | Unix socket path |
| `NEBO_APP_DATA` | `...deal-tracker/data` | Writable data directory |
| `PATH` | system path | Standard path |
| `HOME` | user home | Home directory |

API keys, database URLs, and secrets are **never** passed to sidecars.

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

Nebo checks your sidecar every 15 seconds. If it crashes:
- Broadcasts `app_crashed` event to the frontend
- Auto-restarts with exponential backoff (10s, 20s, 40s, 80s, 160s, max 5min)
- Maximum 5 restarts per hour
- Broadcasts `app_restarted` after recovery

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
- Sidecar changes: rebuild, reopen the app (sidecar re-launches automatically)
- Agent changes: edit `AGENT.md`, changes are picked up on next invocation

---

## Publishing to NeboLoop

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
