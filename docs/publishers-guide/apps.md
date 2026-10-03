# Apps (`@org/agents/name` with `type: "app"`)

An App is an agent with its own UI. It bundles a persona, an HTML frontend, and an optional native sidecar binary into a standalone application that opens in its own window. Apps are the right choice when chat output isn't enough — dashboards, contact managers, journals, deal trackers.

For packaging format and `manifest.json`, see [Packaging](packaging.md).

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
└── $NEBO_DATA_DIR/       # Auto-created at runtime under Nebo's appdata — persistent data, separate from code
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
  "type": "app",
  "permissions": [
    "network:api.example.com",
    "subagent:market-analyst"
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

### Optional Fields

| Field | Description |
|-------|-------------|
| `type` | Set to `"app"` — this is how Nebo detects an app agent. The legacy spelling `artifact_type` is also accepted for app detection, but the sidecar runtime's manifest parser reads only `type`, so always use `type`. Defaults to empty when omitted. |
| `description` | One-line description shown in the marketplace and apps page. |
| `permissions` | Array of permission strings (see below). |
| `window` | Default window dimensions and title. |

### Permissions

Permissions use `prefix:scope` format. Declare only what your app needs. Each entry must start with a known prefix (`network:`, `subagent:`, `storage:`, `filesystem:`, `shell:`, `memory:`, `oauth:`, etc.) — an unknown prefix fails manifest validation at launch.

The permissions that are actively enforced at the app API layer:

| Permission | What It Grants |
|------------|---------------|
| `network:{domain}` | Make HTTP requests to that domain through the CORS-free proxy (e.g. `network:api.zillow.com`). Use `network:*` to allow any domain. Checked per-request against the target URL's host. |
| `subagent:{agentId}` | Invoke that specific agent via `nebo.agents.invoke({ agent })`. Declared per target agent. Invoking the app's own agent needs no permission. |
| `device:motion` | The page's `Permissions-Policy` allows `accelerometer` and `gyroscope` for itself, so `devicemotion` and `deviceorientation` fire. Every other app page keeps motion sensors blocked. On iPhone the page must also call `DeviceMotionEvent.requestPermission()` from a tap. |

Other prefixes (`storage:`, `memory:`, `filesystem:`, `shell:`, `oauth:`, …) are accepted by manifest validation but not yet enforced at the app API layer. The storage, agent-invoke, and janus endpoints are instead gated by the app's per-launch auth token (see Environment Variables).

### Window Config

| Field | Default | Description |
|-------|---------|-------------|
| `title` | app name | Window title bar |
| `width` | 1024 | Default width (pixels) |
| `height` | 768 | Default height (pixels) |
| `resizable` | true | Allow user resize |
| `fullscreen` | false | Open over the whole screen. Desktop: a full-screen window (its size is never saved as the windowed size). Phone: no app bar or safe-area padding, system bars hidden, screen kept awake, no pull-to-refresh, the iOS edge swipe off (Android back walks the page's history, then closes), and a Close pill in the top-left corner that fades after 3 seconds and comes back on a touch near the top. |
| `orientation` | `"portrait"` | `"portrait"`, `"landscape"` or `"any"` on the phone. Any other value is refused when the manifest is written; the phone returns to its normal orientations on close. |
| `pull_to_refresh` | false | On the phone, pulling down from the top of the page reloads it. Off unless asked for: an app is used by touch, and a drag that reloads it (a design canvas, a card game) makes it unusable. Set `true` only for a page that reads like a feed. Fullscreen apps never have it. |
| `voice` | false | On the phone, puts the chat's own dictate and voice buttons at the right of the app's bar when the app is opened from its chat, so the owner can direct the employee by talking while looking at the page (a design canvas). Never shown unless asked for; fullscreen apps have no bar and never get them. See **Voice** below. |

There are no `min_width` / `min_height` fields. Nebo remembers window position and size per app: the user's last arrangement is restored on reopen.

A full-screen page should pad itself with `env(safe-area-inset-*)` (and `viewport-fit=cover` in its viewport meta) and keep controls clear of the top-left corner.

**Voice.** Voice lives in the employee's chat, never inside the app: an app runs no call of its own, and nothing is drawn over your page. An app that sets `window.voice: true` gets the chat's two buttons in its phone bar, beside the centred title:

- **Dictate** (microphone) opens the chat's message box in a sheet, already listening; Send posts to the chat the app was opened from.
- **Voice** (waveform) starts that chat's call. While the call is on, the pair becomes **Mute** and **End**. What is said lands in the chat, and the employee changes the page as it talks.

The buttons drive the chat underneath the app, so a page reload never touches the call. An app opened from home (no chat under it) shows neither. On a call the employee speaks its own short updates in its own voice ("On it, swapping the hero photo now.") and never reads out tools, file names or commands.

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

In the browser, and on the phone and web through the bot's tunnel, the same `ui/` is served under a prefix: `/apps/{agent_id}/ui/` and `https://neboai.com/t/{bot_id}/apps/{agent_id}/ui/`.

**Every path your page uses must be relative** (`./app.js`, `./assets/hero.webp`). A root path (`/app.js`, Vite's default `/assets/...`) works in the desktop window, where `ui/` is the origin's root, and fails everywhere else: on the phone it leaves the tunnel prefix and 404s. Build tools need a relative base (Vite: `base: './'`; SvelteKit: `paths.relative: true`). Load the SDK the same way, relative to `ui/index.html`: `<script src="../../../sdk/nebo.global.js"></script>`.

### Serving rules

- **Content types.** Each file is served with its real type: `html htm`, `js mjs`, `css`, `json map`, `txt`, images (`png jpg jpeg gif svg ico webp avif ktx2`), fonts (`woff woff2 ttf otf`), `wasm`, video (`mp4 m4v webm mov`), sound (`mp3 wav ogg oga opus m4a aac flac`) and 3D models (`glb gltf`). Extensions match regardless of case. Anything else is `application/octet-stream`, which the browser will not use as media or a model.
- **Range requests.** Every file except the entry HTML answers a single `Range: bytes=` request with `206` and `Content-Range`, reading only those bytes, and a range starting past the end with `416`; responses carry `Accept-Ranges: bytes`. Multi-range requests get the whole file. WebKit will not play or seek a video whose server ignores ranges.
- **Caching.**
  - A content-hashed name (`<name>-<hash>.<ext>`, the hash 8 to 64 letters and digits of one case with a digit between two letters, such as `main-0a8ksftt.js` or `chunk-5JFTZ4CW.js`) is `public, max-age=31536000, immutable`. Hand-made names like `hero-section2.png` or `shot-20260930.png` are not treated as hashed.
  - Everything else, `index.html` included, is `no-cache` with a strong `ETag`; `If-None-Match` answers `304` with no body. The entry HTML's tag covers the scripts Nebo injects, so a Nebo update counts as a changed page. A range whose `If-Range` names an older file gets the whole new file.
  - With App Developer mode on, every file is `no-store`.
- **Size.** Each page file may be at most 10 MB, the marketplace's per-file limit, so whatever publishes also installs.
- **Video on the phone.** The app view plays media inline without a tap, so `<video muted playsinline autoplay loop>` plays in place and `currentTime` can be set from scroll.

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

The bundle defines one global, `NeboAppSDK`. `NeboAppSDK.nebo` is the same singleton the ES module exports, and every module (`identity`, `storage`, `agents`, `janus`, `decide`, `chat`, `surfaces`) is also on `NeboAppSDK` directly. There is no bare `nebo` global — take it off `NeboAppSDK` first.

```html
<script src="../../../sdk/nebo.global.js"></script>
<script>
  const { nebo } = NeboAppSDK;
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

The embedded chat uses session key `agent:{agentId}:app` — separate from `nebo.agents.invoke()` which uses `app:{agentId}:api`. Conversations persist across page reloads.

##### Document-Scoped Sessions (`contextId`)

By default, all chats share one session per agent. For apps that display different content (documents, projects, records), pass `contextId` to scope each conversation:

```typescript
// Each document gets its own persistent chat history
nebo.chat.mount(chatContainer, {
  contextId: document.id  // → session key: agent:{id}:app:{contextId}
});
```

When the user switches documents, unmount and remount with the new `contextId`. Each context maintains its own conversation — messages from one document don't leak into another.

**Memory isolation with `contextId`:** By default, all contexts share the agent's memory pool. To isolate memories per context (so Client A's facts never appear in Client B's chat), add `"memory": { "mode": "confidential" }` to `agent.json`. See [Agents — Memory](agents.md#memory).

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
nebo.storage.getItem(key: string): Promise<any | null>   // exactly what setItem stored
nebo.storage.removeItem(key: string): Promise<void>
nebo.storage.keys(): Promise<string[]>
nebo.storage.clear(): Promise<void>
nebo.storage.onChange(handler: (change: {
  appId: string;
  keys: string[];
  action: 'set' | 'delete';
  source: 'employee' | 'page';
}) => void): () => void                                  // returns "stop listening"
```

##### Your App's Data

The page's storage and the app's employee share **one store**: the same keys and the same values. The app's employee has a built-in tool for its own app's data: it can read a key, save any JSON value, delete a key, list keys by prefix, and search (field contains text, any case; nested fields like `phone.mobile`; a key holding a list is searched item by item). So a contact the owner adds by talking to the employee shows on the page, and one typed into the page is one the employee can find.

- **Only the app's own employee** reaches the store, and only for its own app. Another employee asks the app's employee for what it needs. Other apps never see it.
- **Every write is announced.** A save or delete by the employee (`source: "employee"`) or by any open window of the app, including this one (`source: "page"`), calls every `onChange` handler. Redraw there:

```typescript
async function load() {
  render((await nebo.storage.getItem('contacts')) ?? []);
}
load();
nebo.storage.onChange((c) => { if (c.keys.includes('contacts')) load(); });
```

- **One record per chat:** a key starting with `chat:` belongs to the conversation that writes it. When the employee writes `chat:design` from the owner's chat `<id>`, it is stored as `chat:<id>:design`, and the page opened from that chat gets `?thread=<id>` in its address (desktop and phone) to read it. So an app that keeps one piece of work per conversation (a design, a draft) needs no ids in its instructions: the employee always writes `chat:design`, and the page reads `chat:${thread}:design`. Keys without `chat:` are the app's, shared by every chat. Outside one of the owner's chats (a scheduled run, a caller) a `chat:` key is refused.

```typescript
const thread = new URLSearchParams(location.search).get('thread');
const key = thread ? `chat:${thread}:design` : 'design';
render(await nebo.storage.getItem(key));
nebo.storage.onChange((c) => { if (c.keys.includes(key)) render(...); });
```

- **Small edits without rewriting:** the employee's `replace` action changes one exact piece of text inside a stored value (a heading inside a 20 KB page) instead of writing the whole value again; it must match exactly one place, or nothing changes. Tell the employee in AGENT.md to prefer it for small edits.
- **Values are JSON, never JSON written inside a string.** The employee's `set` stores what it is given: text that is JSON (an object, a list) is stored as the value it spells, and text that only looks like JSON but does not parse is refused with "Nothing was saved", so a broken value never reaches your page.
- **Pick keys both sides can find:** one key holding a list (`contacts`), or one key per record under a prefix (`contact:42`). Describe the shape in AGENT.md so the employee uses the same keys.
- A string that is itself valid JSON, such as `"42"` or `"true"`, comes back parsed. Wrap it in an object if the type matters.
- There is no quota, but each value is sent in one request; keep it well under 2 MB and move large or relational data to a sidecar.
- `setItem` and `removeItem` do not throw when a write is refused; read back anything that must not be lost.

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
  model?: string,        // optional; Nebo picks one when omitted
  temperature?: number,
  max_tokens?: number,
  system?: string
}): Promise<string>

nebo.janus.stream(options): AsyncGenerator<string>
```

#### Decide (Typed Decisions)

Ask named questions about some data and get each answer with probabilities and a confidence, in one fast call. Nothing is written as text: use it for judgments (is this lead hot, which category, how urgent) and keep counting, dates and thresholds in your own code.

```typescript
nebo.decide(request: {
  state: unknown,  // text or any JSON: a record, a list of records
  questions: Record<string,
    | { type: 'choice', instructions: string, criteria: Record<string, string> } // 2 to 255 options
    | { type: 'score',  instructions: string, criteria: string[] }               // 2 to 10 levels, lowest first
    | { type: 'noul',   instructions: string }                                   // one statement, no criteria
  >
}): Promise<{
  model: string,
  answers: Record<string, {
    type: 'choice' | 'score' | 'noul',
    choice?: string,       // choice: the option picked
    score?: number,        // score: fractional, 0 = the first level
    noul?: number,         // noul: probability the statement holds
    confidence?: number,   // choice and score: 0 to 1
    probabilities: Record<string, number>
  }>,
  usage: { input_tokens: number, output_tokens: number, cost_micro: number }
}>
```

```typescript
const { answers } = await nebo.decide({
  state: { company: 'Example Co', status: 'asked for a quote today' },
  questions: {
    tier: { type: 'choice', instructions: 'How warm is this lead, judging by `status`?',
            criteria: { hot: 'ready to buy', warm: 'interested', cold: 'not now', other: "can't tell" } },
    reply: { type: 'noul', instructions: '`status` asks us for a reply.' }
  }
});
if (answers.tier.choice === 'hot' && answers.tier.confidence > 0.8) flagLead();
```

- The whole question lives in `instructions`; name the state's fields in backticks. The question's name only labels its answer. Add an escape option (`other`) when a choice list is not complete.
- Very long state is shortened in the middle before it is sent; keep it to the fields the questions need.
- It throws an `Error` whose message is the reason; the error has no status code, so compare the message if you need to tell them apart. A malformed question (the route answers 400, with what is wrong); no work left on the owner's account (429, "You've used all the work included in your account. Choose a plan or add credits to continue."; retrying does not help until the account is funded); too many decisions at once (429, "Too many decisions at once. Try again in a moment.", after the bot has retried once); the bot not signed in to NeboAI (503, "Decisions need NeboAI connected. Sign in to NeboAI and try again."); or the decision service failing (502).
- The app's employee has a `decide` tool that takes the same request, so it can judge records it reads from the app's data.
- Billed to the bot owner's NeboAI account like any model call. See pricing at https://neboai.com/pricing.

#### HTTP Proxy

Make CORS-free HTTP requests through Nebo's server.

```typescript
// Standard fetch — automatically routed through Nebo
const resp = await nebo.fetch('/apps/my-app/api/data');
```

#### WebSocket

Real-time connection to the app's agent.

`nebo.WebSocket()` takes no path; it always connects to the app's own channel (`/ws/app/{appId}`).

```typescript
const ws = new nebo.WebSocket();
ws.addEventListener('message', (evt) => { ... });
ws.send('event-name', { key: 'value' });
ws.close();
```

---

## App SDK (`@neboai/app-sdk`)

The App SDK package (`@neboai/app-sdk`) provides three standalone integration patterns for apps that need direct control over agent communication, beyond what the `nebo` global object offers.

### Surfaces API

> **What Nebo sends today.** App pages receive A2UI cards from the app's employee (render them with `nebo.a2ui` after `surfaces.connect()`; the SDK has no renderer, so bundle `@a2ui/web_core` and pass its MessageProcessor to `nebo.a2ui.init()`) and storage changes (`nebo.storage.onChange`). A card reaches only the app it was made for, a click on it goes to that app's employee, and a page opened after a card was sent does not receive it. The typed events below (`text_content`, `state_snapshot`, `state_delta` and the rest) are defined in the SDK but are not sent to app pages yet, and nothing answers `surfaces.send()`. Use `storage.onChange` for live data.

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
  │  {base}/apps/{id}/api/projects
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
- Relative URLs (no scheme) → routed to your sidecar via the proxy route `{base}/apps/{id}/api{path}`
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
2. Read `$NEBO_DATA_DIR` for the writable data directory
3. Bind a Unix socket at the `$NEBO_APP_SOCK` path
4. Serve the `UIService` gRPC service on that socket

Minimal Rust example:

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sock_path = std::env::var("NEBO_APP_SOCK")
        .unwrap_or_else(|_| "/tmp/my-app.sock".into());
    let data_dir = std::env::var("NEBO_DATA_DIR")
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

Your sidecar runs in a sandboxed environment. The injected variables are:

| Variable | Example | Description |
|----------|---------|-------------|
| `NEBO_APP_ID` | `deal-tracker` | Agent identifier |
| `NEBO_APP_NAME` | `Deal Tracker` | Display name |
| `NEBO_APP_VERSION` | `1.0.0` | Manifest version |
| `NEBO_APP_DIR` | `<Nebo data dir>/user/agents/deal-tracker` | App root directory (versioned code dir — read-only in spirit; write to `NEBO_DATA_DIR`) |
| `NEBO_APP_SOCK` | `...deal-tracker/deal-tracker.sock` | Unix socket path |
| `NEBO_DATA_DIR` | `<Nebo appdata>/agents/deal-tracker` | Writable data directory under Nebo's appdata, `appdata/agents/<app id>/` — the app's own, keyed by its id (separate from code — survives upgrades, reinstalls and renames of the app's folder). Always use the env var; never hardcode a path. |
| `NEBO_API_URL` | `http://127.0.0.1:27895` | Local Nebo API base URL (the port is the running server's port, injected at launch) |
| `NEBO_APP_TOKEN` | (per-launch token) | Per-launch auth token for calling the Nebo API |
| `PATH` | system path | Allowlisted system var |
| `HOME` | user home | Allowlisted system var |
| `TMPDIR` | system temp dir | Allowlisted system var |
| `LANG` | `en_US.UTF-8` | Allowlisted system var |
| `LC_ALL` | `en_US.UTF-8` | Allowlisted system var |
| `TZ` | `America/New_York` | Allowlisted system var |

The environment is sanitized — only the variables above are passed through. API keys, database URLs, and secrets are **never** passed to sidecars.

### Data Persistence

The sidecar owns its own data in `$NEBO_DATA_DIR`. Common approaches:

- **JSON file** — simplest. Load at startup, save after mutations. Works well for small datasets (< 10MB).
- **SQLite** — use for structured data, queries, or anything beyond trivial CRUD.
- **File store** — store blobs (uploaded documents, images) as files in the data directory.

The data directory is physically separated from the code directory — it lives under Nebo's appdata area at `appdata/agents/<app id>/` (e.g. `~/Library/Application Support/Nebo/appdata/agents/deal-tracker/` on macOS), not inside the app's code tree. Every app has its own; no two apps share one. Always resolve it via `$NEBO_DATA_DIR` rather than constructing the path yourself. The sidecar's working directory is set to the data directory at launch, so relative paths (`./app.db`) also land in persistent storage. This means you can safely upgrade or reinstall the app binary without touching your data. The data directory survives sidecar restarts, app updates, reinstalls, and Nebo upgrades. It follows the iOS model: the update system physically cannot reach the data container.

### Binary Location

Place your compiled binary in one of these locations (checked in order):

1. `{app_dir}/binary` — single named file
2. `{app_dir}/app` — single named file
3. `{app_dir}/tmp/` — first file in directory
4. `{app_dir}/bin/` — first file in directory
5. `{app_dir}/sidecar/target/release/` — first executable (Rust dev builds)

For production distribution, use `bin/`. For development, `sidecar/target/release/` is detected automatically.

### Startup Timeout

The sidecar must create the Unix socket within the `startup_timeout` (default 10 seconds, max 120). If the socket doesn't appear — or the process exits first — the launch fails and is retried with backoff (see below).

### Health Checking & Auto-Restart

One supervisor per sidecar starts, watches and restarts it — at boot, on the first request, on "Try again", and after every failure:

- **Exit awaited** — the moment the process exits for any reason, Nebo logs its exit status (code or signal) with the last lines of `sidecar.log`, removes its socket, and restarts it.
- **Socket probed** — every 3 seconds Nebo connects to the socket; three failed probes in a row mean the process is no longer serving, and it is stopped and restarted. A request that cannot reach the sidecar triggers the same restart at once (and a GET/HEAD is sent again to the new process).
- **Backoff** — 1s, 2s, 4s, … capped at 60s. The count resets once a sidecar has served for a minute.
- **Failed** — after 5 consecutive failures the sidecar is `failed`, with the reason. Nebo keeps retrying every 60s without the state flapping. A permanent cause — no program, one the system refuses to run, a damaged manifest — is `failed` at once, with "Reinstall" in the message.
- **Binary hot-reload** — if the binary on disk changes (e.g. you rebuild), Nebo gracefully stops the running process and runs the new one, re-registering tools from `agent.json`. This is not a crash — no backoff, no count. The change watcher resolves through symlinks, so a dev setup like `bin/my-app → sidecar/target/release/my-app` is detected when the underlying target is rebuilt. Note that the binary that actually launches must be a **regular file** — validation rejects a symlinked binary at launch.
- **Stopping is not crashing** — Nebo exiting, a hot reload of Nebo, or the app being deactivated stops the sidecar on purpose; nothing restarts it.

An app with no program at all (a UI-only app) has nothing to supervise; its `/api/*` requests answer `404` with `sidecar.state: "none"`.

#### What your app sees

While the sidecar cannot serve, `/api/*` answers with JSON your page can act on — the real reason, never a generic error:

```json
{ "error": "Neighbor Mail is restarting.", "sidecar": { "state": "restarting", "attempt": 2, "retryInMs": 2000, "reason": "exited with code 1; last output: …" } }
```

- `503` + `Retry-After` while `starting` or `restarting` — retry on your own.
- `503` when `failed` — show `error`, offer "Try again" (`POST /api/v1/apps/{id}/sidecar/restart`, which answers with the state it settled in); for `permanent: true`, offer Reinstall.
- `GET /api/v1/apps/{id}/sidecar` returns the current state. Every change is also broadcast on the WebSocket as `sidecar_state` (`{ agentId, state, … }`), alongside `app_started`, `app_crashed`, `app_restarted` and `app_stopped`.

A `502` from your own sidecar (for example when *your* upstream service is down) is passed through unchanged — it means your sidecar is running and answered.

### Lifecycle

Nebo owns the full lifecycle of every sidecar. When Nebo shuts down (SIGTERM, Ctrl+C, or app quit), it sends SIGTERM to every running sidecar and waits for them to exit before the process ends. Sidecars should handle SIGTERM gracefully — flush data, close connections, then exit.

You do not need to manage sidecar lifetime yourself. Nebo handles:
- **Auto-launch** — the first API request to a sidecar starts it if nothing has
- **Restore on server restart** — every enabled app's sidecar is put under supervision when the server starts; a launch that fails at boot is retried, not forgotten
- **Health checks** — exit awaited, socket probed every 3 seconds
- **Crash recovery** — auto-restart with exponential backoff, `failed` with the reason after repeated failures
- **Hot-reload** — restart on binary change (no backoff)
- **Shutdown** — SIGTERM on Nebo exit, never counted as a crash

### Sidecar Tools (declared in `agent.json`)

Tool definitions live in `agent.json`, following the same filesystem-based pattern as skills and plugins — there is no HTTP discovery step. When the sidecar launches, Nebo reads the `tools` array from `agent.json` and registers each entry as an LLM-callable tool — the agent can then call your sidecar's API directly during conversations.

This is optional. If your `agent.json` declares no tools, the agent can still be used via the chat embed and SDK, but it won't be able to call your API endpoints as tools during LLM reasoning.

#### Declaring Tools

Add a `tools` array to your `agent.json`:

```json
{
  "tools": [
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
  ]
}
```

Your sidecar just implements the corresponding routes in `HandleRequest` — no `/_tools` endpoint is needed. (Sidecar paths starting with `_` are reserved for Nebo's internal use and are blocked from external HTTP clients.)

#### Tool Definition Schema

Each entry in the `tools` array has these fields:

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

1. Sidecar starts → Nebo reads the `tools` array from `agent.json`
2. Nebo registers each tool **per-agent** in the global tool registry — the LLM sees `list_projects(...)`, `get_document(id: "...")`, etc. directly
3. For `GET` requests, non-path input parameters are sent as query strings
4. For `POST`/`PUT` requests, input parameters (minus path params) are sent as the JSON body
5. Tools are re-registered automatically when the sidecar restarts (crash recovery or binary hot-reload)
6. On sidecar shutdown, all registered tools are unregistered

Sidecar tools bypass the contextual tool filter — they are always included for their owning agent regardless of conversation context. Tool calls are routed to the sidecar through `GrpcSidecarCaller`, which translates tool invocations into gRPC `HandleRequest` calls on the Unix socket.

**Tools are not enough on their own.** The LLM knows it *can* call `list_projects`, but it doesn't know *when* or *how* to use it effectively. That's what skills are for — see [Skills](#skills) below.

#### Tips

- Keep descriptions clear and concise — the LLM uses them to decide which tool to call
- Include `input_schema` for POST/PUT actions so the LLM knows what parameters to provide
- Path parameter names in `input_schema.properties` should match the `{placeholder}` in the path
- Omit the `tools` array entirely if your sidecar has no tools
- Avoid tool names that collide with Nebo's core domain tools (`agent`, `os`, `web`, `loop`, `message`, `event`, `skill`, `work`)

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
- **tools** — which sidecar tool names are active (subset of the `tools` declared in agent.json)
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

Sidecar stdout and stderr are captured to the app's data directory — `$NEBO_DATA_DIR/sidecar.log` (e.g. `~/Library/Application Support/Nebo/appdata/agents/{app id}/sidecar.log`), in append mode. When the sidecar exits, Nebo's own log records its exit status and the last lines of this file. Check it when debugging startup issues.

### App Agent Redaction

For agents with `isApp=true`, the API automatically strips sensitive fields from responses to prevent leaking agent internals to end users. The persona, skills list, and frontmatter are not exposed to the client. Only UI-needed fields are returned: name, avatar, and status.

This is transparent — no configuration needed. Any API call that returns agent details for an app agent receives the redacted response.

---

## Framework Notes

Every framework needs a relative base, because the page is served under a prefix everywhere except the desktop window (see Entry Point).

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
    }),
    paths: { relative: true }
  }
};

export default config;
```

### HTMX

HTMX apps work natively. The SDK bridge injects `<meta name="htmx-config" content='{"selfRequestsOnly":false}'>` automatically so HTMX can make requests to the Nebo server. Use the global SDK (`../../../sdk/nebo.global.js` from `ui/index.html`) for storage and agent invocation.

### React / Vue / Solid / Vanilla

Build with a relative base and write the output into `ui/` (Vite: `base: './'`, `build.outDir` pointing at `ui/`). Keep the project (package.json, `src/`, `node_modules/`) beside `ui/`, never inside it.

---

## Complete Example: Journal App

### manifest.json

```json
{
  "id": "journal",
  "name": "@nebo/agents/journal",
  "version": "1.0.0",
  "description": "AI-powered journal with reflection prompts.",
  "type": "app",
  "permissions": [],
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

You are a thoughtful journaling partner. When the user writes an entry,
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
  <script src="../../../sdk/nebo.global.js"></script>
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

App directories live in the platform-native `user/agents/` directory (macOS: `~/Library/Application Support/Nebo/user/agents/`, Linux: `~/.local/share/nebo/user/agents/`, Windows: `%APPDATA%\Nebo\user\agents\`; `NEBO_HOME` overrides the root):

```bash
mkdir -p "$HOME/Library/Application Support/Nebo/user/agents/my-app/ui"
```

### 2. Write manifest.json, AGENT.md, and ui/index.html

See examples above.

### 3. Symlink from source (recommended for development)

```bash
# Work from a source repo, symlink into Nebo
ln -s /path/to/my-app "$HOME/Library/Application Support/Nebo/user/agents/my-app"
```

The filesystem watcher detects new symlinks automatically — the app appears in the Apps tab within seconds, with no copy step. Note that the watcher does not follow symlinks *into* the target directory, so edits made in your source repo often won't fire a reload on their own. Touch the symlink, call `POST /agents/{id}/reload`, or restart Nebo to pick them up. (UI files under `ui/` are read per request, so frontend edits always show on refresh — this affects `AGENT.md` and `agent.json` changes.)

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

## Building and Publishing from Nebo

An employee can build an app two ways, chosen by what the owner says:

- "Build me an app for X" creates a **new app employee** (`create_employee` with `app` and `ui`).
- "You are the app" or "build yourself" turns the **current employee into the app** (`update_employee` on its own name with `app` and `ui`). It keeps its chat, memory and persona. An employee hired in conversation gets its own package folder on that first update and stays itself; no second employee is created.
- If it is unclear, the employee asks once.

The built-in `app-studio` skill (App Studio) covers every app, from a tracker to a designed game: where files go, TypeScript and JSX compiled by Nebo on write (`.ts`, `.tsx` and `.jsx` files in `ui` become `.js`, sources kept in `src/`) or a Vite build where the bot has node, the verify loop, and the design method. Loading it turns App Developer mode on.

An app the owner made on this bot (not installed from the marketplace) always has the developer pack for itself: `app_reload`, `app_status`, `app_console`, `app_screenshot`, `app_listing`, `app_submit`. Its page always carries the reload listener and console capture, so reload and console work with no setting. App Developer mode (Bot settings, Developer) opens the pack to teammates on any of the owner's apps, adds the floating console, and serves files `no-store`. Apps installed from the marketplace never get developer tooling: nothing is injected into their pages, their console routes return 404, and the tools refuse them, mode or not.

**Publishing yourself.** "Publish yourself" means `app_listing`, then `app_screenshot`, then `app_submit`. The owner can also start it with **Publish** in the app's chat, on the phone's app screen (top bar, or a pill beside Close when full screen), or with **Publish This App…** on desktop (macOS File menu while an app window has focus; an **App** menu on the app window on Windows and Linux). The listing needs a name, a short description of 10 to 500 characters, a semver version, 1 to 10 screenshots and a category. Nothing is submitted until the owner answers **Submit for review** on the card in the chat; voice can answer it. The package carries AGENT.md, agent.json, manifest.json, `ui/` and the employee's own skills under `skills/<name>/` (its package's `skills/` folders plus plain-named skills its agent.json lists from the user's skills). Marketplace skill references and learned skills are not included. A file over 10 MB or a total over 50 MB is refused before anything is sent.

---

## Publishing to NeboAI

### Page-only app (no sidecar)

```
1. developer(resource: account, action: select, id: "your-dev-account-id")
2. agent(action: create, name: "deal-tracker", manifestContent: "<AGENT.md>")
   The AGENT.md frontmatter must say artifact_type: app, and any value
   containing ": " must be quoted.
3. agent(action: bundle-token, id: "AGENT_ID")
4. Run the returned curl with a .zip of AGENT.md, agent.json, manifest.json,
   ui/ and any skills/<name>/ folders. The token lasts 5 minutes.
5. agent(action: submit, id: "AGENT_ID", version: "1.0.0")
```

What the marketplace does with the bundle:

- `ui/**` becomes the app's page. Allowed types are the skill file types plus sound, video, `wasm`, `glb`/`gltf`, `mjs`, `avif`, `jsx`, `tsx` and `map`. Dot files, dot folders and `node_modules/` are dropped; build folders like `dist/` are kept inside `ui/`.
- The root `manifest.json` is kept as the app's manifest (window and permissions reach the installed package); it must be valid JSON.
- Files under `skills/<name>/` follow the skill rules inside their folder: `SKILL.md` is kept, `scripts/` and `bin/` may hold any type, everything else keeps the allowlist.
- Limits: a file over 10 MB is skipped and counted in `filesSkipped`; over 50 MB in total is refused. The result also reports `uiFilesStored`.
- For a private or loop app the installable package is rebuilt on every upload and Nebo tells the bots that installed the app: one that is online puts the rebuilt package in place right away, one that is offline when it next connects. No version bump is needed, and the owner's settings, schedules and data for the app are kept. Public, unlisted and invite-only apps keep their approved package until a new version passes review; once it is approved, installed bots offer it in Settings → Updates for the owner's yes, or apply it right away when automatic updates are on for that app.
- An update never grants new permissions: a permission the new version adds is asked for when the app first needs it.
- If NeboAI withdraws an app from the marketplace, every bot that installed it turns it off and tells the owner: "<name> is turned off. NeboAI withdrew <name> from the marketplace, so it is turned off here. Everything it saved is kept." Nothing is deleted.

### App with a sidecar

`agent(action: binary-token, id)` returns a curl for `POST /api/v1/developer/apps/{id}/binaries` with `file` (the sidecar for one `platform`, packed under `bin/`) and `ui` (a tar.gz of the built page, packed under `ui/`). Repeat per platform. An app that declares a sidecar needs at least one platform binary for its version before submit.

Apps without a sidecar need no binary upload.
