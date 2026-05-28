# Plugins (`@org/plugins/name`)

A Plugin is a managed native binary that skills depend on. Instead of bundling a heavy binary inside every skill that needs it, publish the binary once as a plugin and let skills declare it as a dependency. Nebo downloads the plugin automatically when a skill that needs it is installed.

For packaging and distribution, see [Packaging](packaging.md).

---

## When to Use a Plugin vs. Embedded Binary

| Pattern | Use When | Example |
|---------|----------|---------|
| **Plugin** | Multiple skills share the same binary, or the binary is large (>5MB) | `gws` (Google Workspace CLI), `ffmpeg` |
| **Embedded binary** | One skill bundles its own small binary | Custom tool specific to a single skill |

You can use both patterns. A skill can embed a binary AND declare plugin dependencies. The embedded binary takes precedence for the skill's own execution; plugin binaries are available to scripts via environment variables.

---

## What a Plugin Can Do

A plugin is a single native binary that can contribute multiple capabilities simultaneously. What makes a plugin a "connector" vs a "provider" vs a "tool plugin" is determined by which capabilities it declares in its manifest. A single plugin can declare all of them.

| Capability | Manifest Field | What It Does | Runtime |
|---|---|---|---|
| **Tools** | `capabilities.tools[]` | Registers typed, schema-validated tools the agent can call | Routed through STRAP `PluginTool` |
| **Hooks** | `capabilities.hooks[]` | Intercepts lifecycle events (tool execution, messages, memory, prompts) | `HookDispatcher` with circuit breaker |
| **Commands** | `capabilities.commands[]` | Registers `/slash` commands that bypass the LLM and execute directly | Chat dispatch, 30s timeout |
| **Routes** | `capabilities.routes[]` | Handles HTTP endpoints (OAuth callbacks, webhooks) | Proxied through catch-all handler |
| **Providers** | `capabilities.providers[]` | Registers as an AI model provider (LLM, speech, image) | `PluginProvider` with NDJSON streaming |
| **Config** | `capabilities.configSchema[]` | Declares user-configurable settings rendered as a form in the UI | Values injected as env vars |
| **Events** | `events[]` | Produces events via long-running watch processes (NDJSON on stdout) | Auto-emitted into EventBus |
| **Auth** | `auth` | Declares an authentication flow (OAuth, API keys) with login/status/logout | HTTP endpoints + WebSocket events |
| **Permissions** | `permissions` | Declares env var access, network needs, and max execution timeout | Enforced at exec time |

### Common Plugin Patterns

**Connector plugin** — wraps a SaaS API. Declares `auth` for OAuth/API key login, `tools[]` for API operations, `events[]` for webhook-driven watches, and `configSchema[]` for user settings.

```json
{
  "id": "asana",
  "slug": "asana",
  "capabilities": {
    "tools": [
      { "name": "asana.list-tasks", "description": "List tasks", "command": "tasks list" }
    ],
    "configSchema": [
      { "key": "WORKSPACE_ID", "label": "Workspace", "fieldType": "string", "required": true }
    ]
  },
  "auth": { "type": "oauth_cli", "label": "Asana account", "commands": { "login": "auth login", "status": "doctor" } },
  "events": [
    { "name": "task.created", "description": "New task created", "command": "watch tasks" }
  ]
}
```

**Provider plugin** — adds an AI model backend. Declares `providers[]` with commands for listing models and streaming chat.

```json
{
  "id": "openrouter",
  "slug": "openrouter",
  "capabilities": {
    "providers": [
      {
        "id": "openrouter",
        "displayName": "OpenRouter",
        "providerType": "model",
        "modelsCommand": "models list",
        "chatCommand": "chat stream",
        "authCommand": "auth setup"
      }
    ]
  }
}
```

**Hook plugin** — intercepts agent lifecycle events. Declares `hooks[]` to filter or observe tool calls, messages, memory, or prompts.

```json
{
  "id": "content-filter",
  "slug": "content-filter",
  "capabilities": {
    "hooks": [
      {
        "hook": "message.pre_send",
        "hookType": "filter",
        "priority": 10,
        "command": "filter message",
        "timeoutMs": 200
      },
      {
        "hook": "tool.pre_execute",
        "hookType": "filter",
        "priority": 50,
        "command": "filter tool-input",
        "timeoutMs": 500
      }
    ]
  }
}
```

**Utility plugin** — shared binary that other plugins or skills depend on. No capabilities of its own — just a binary distributed via env var.

```json
{
  "id": "ffmpeg",
  "slug": "ffmpeg",
  "platforms": {
    "darwin-arm64": { "binaryName": "ffmpeg", "sha256": "...", "size": 50000000 }
  }
}
```

### Available Hook Points

Plugins can subscribe to these lifecycle hooks. Filter hooks can modify payloads; action hooks are fire-and-forget.

| Hook | Default Type | When |
|---|---|---|
| `tool.pre_execute` | filter | Before a tool runs — can modify input or block |
| `tool.post_execute` | filter | After a tool completes — can modify output |
| `message.pre_send` | filter | Before user message is sent to the agent |
| `message.post_receive` | filter | After agent message is received |
| `memory.pre_store` | filter | Before a memory is persisted |
| `memory.pre_recall` | filter | Before memories are retrieved |
| `session.message_append` | action | When a message is added to the session |
| `prompt.system_sections` | filter | During system prompt generation — can inject sections |
| `steering.generate` | filter | During steering signal generation |
| `response.stream` | action | During response streaming |
| `agent.turn` | action | When an agent turn completes |
| `agent.should_continue` | filter | Decision point to continue or halt a turn |

Hook circuit breaker: 3 consecutive failures disables the hook, auto-recovery after 5 minutes.

---

## How It Works

1. Publisher uploads a native binary to NeboAI for each platform
2. Publisher creates a skill with `plugins:` in SKILL.md frontmatter (or another plugin with `dependencies:` in plugin.json)
3. User installs the skill or plugin (via marketplace or install code)
4. Nebo detects the plugin dependency and downloads the binary silently
5. If the plugin declares its own `dependencies[]`, those are installed recursively
6. Binary is stored locally at `~/.nebo/nebo/plugins/<slug>/<version>/`
7. Plugin runtime data (databases, caches, logs) is stored at `~/.nebo/appdata/plugins/<slug>/` — physically separate from the binary, survives upgrades
8. Skill scripts access the binary via `${plugin.SLUG_BIN}` template variable (e.g. `${plugin.GWS_BIN}`)
8. If the plugin declares `capabilities.tools[]`, typed tools are registered for the agent

```
User installs skill → SKILL.md declares plugins: [{name: "gws", version: ">=1.2.0"}]
  → Nebo downloads gws binary for current platform
  → Skill script runs with GWS_BIN=/path/to/gws
```

---

## Declaring Plugin Dependencies

Add a `plugins` field to your SKILL.md frontmatter:

```yaml
---
name: google-workspace
description: Manage Google Workspace — Gmail, Calendar, Drive
plugins:
  - name: gws
    version: ">=1.2.0"
---
```

### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Plugin slug (must match the plugin's registered slug in NeboAI) |
| `version` | string | `"*"` | Semver version range |
| `optional` | bool | `false` | If true, the skill loads even if this plugin isn't installed |

### Version Ranges

Version ranges follow semver conventions:

| Range | Meaning |
|-------|---------|
| `"*"` | Any version |
| `">=1.2.0"` | 1.2.0 or higher |
| `"^1.0.0"` | Compatible with 1.x.x (>=1.0.0, <2.0.0) |
| `"~1.2.0"` | Patch updates only (>=1.2.0, <1.3.0) |
| `"=1.2.0"` | Exact version |

### Multiple Dependencies

```yaml
---
name: media-processor
description: Process and convert media files
plugins:
  - name: ffmpeg
    version: ">=5.0.0"
  - name: imagemagick
    version: ">=7.0.0"
    optional: true
---
```

The skill loads only if all **required** plugins resolve. Optional plugins are silently skipped if missing.

### Alternative: `requires` Block

Plugin manifests can also declare dependencies using a `requires` block, which supports the same fields:

```yaml
requires:
  plugins:
    - name: gws
      version: ">=1.2.0"
      optional: false
```

- `optional: true` — the skill or plugin loads without this dependency, but features may be degraded.
- Version uses semver range matching (same syntax as the `plugins` frontmatter field).

---

## Using Plugin Binaries in Scripts

Plugin binaries are accessible in two ways depending on context:

### In SKILL.md Body (Template Variables)

Use `${plugin.SLUG_BIN}` syntax. Nebo expands these at skill activation time.

| Plugin Slug | Template Variable | Expands To |
|-------------|-------------------|------------|
| `gws` | `${plugin.GWS_BIN}` | `/Users/me/.nebo/nebo/plugins/gws/1.2.3/gws` |
| `ffmpeg` | `${plugin.FFMPEG_BIN}` | `/Users/me/.nebo/nebo/plugins/ffmpeg/2.0.0/ffmpeg` |
| `my-tool` | `${plugin.MY_TOOL_BIN}` | `/Users/me/.nebo/nebo/plugins/my-tool/1.0.0/my-tool` |

The slug is uppercased with hyphens replaced by underscores.

### In Script Files (Environment Variables)

When a script runs, plugin binaries are injected as environment variables using the `{SLUG}_BIN` naming convention:

| Plugin Slug | Environment Variable |
|-------------|---------------------|
| `gws` | `GWS_BIN` |
| `ffmpeg` | `FFMPEG_BIN` |
| `my-tool` | `MY_TOOL_BIN` |

Additionally, `NEBO_PLUGIN_DATA` is set to `~/.nebo/appdata/plugins/<slug>/` — the persistent data directory for this plugin. Use this for caches, databases, and any state that should survive plugin upgrades.

### Python Example

```python
#!/usr/bin/env python3
import os
import subprocess

gws_bin = os.environ["GWS_BIN"]
result = subprocess.run([gws_bin, "gmail", "list", "--limit", "10"], capture_output=True, text=True)
print(result.stdout)
```

### TypeScript Example

```typescript
import { execSync } from "child_process";

const gwsBin = process.env.GWS_BIN!;
const output = execSync(`${gwsBin} gmail list --limit 10`, { encoding: "utf-8" });
console.log(output);
```

### Shell Example

```bash
#!/bin/bash
$GWS_BIN gmail list --limit 10
```

---

## The plugin.json Manifest

Every plugin has a `plugin.json` manifest that describes the binary, its platforms, and optional capabilities like authentication and events. This file is stored alongside the binary on disk and cached in memory by the PluginStore.

```json
{
  "id": "gws",
  "slug": "gws",
  "name": "Google Workspace CLI",
  "version": "1.2.3",
  "description": "Google Workspace integration for email, calendar, and drive",
  "author": "NeboAI Inc.",
  "platforms": {
    "darwin-arm64": {
      "binaryName": "gws",
      "sha256": "a1b2c3...",
      "signature": "base64...",
      "size": 45678900,
      "downloadUrl": "https://cdn.neboai.com/plugins/gws/1.2.3/darwin-arm64/gws"
    },
    "linux-amd64": {
      "binaryName": "gws",
      "sha256": "d4e5f6...",
      "signature": "base64...",
      "size": 42000000,
      "downloadUrl": "https://cdn.neboai.com/plugins/gws/1.2.3/linux-amd64/gws"
    }
  },
  "signingKeyId": "key-001",
  "envVar": "",
  "auth": { ... },
  "events": [ ... ],
  "dependencies": [ ... ],
  "capabilities": { ... }
}
```

### Manifest Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | **Yes** | Unique identifier — use the plugin's slug (e.g., `"gws"`). For published plugins, NeboAI may assign its own artifact ID. **Deserialization fails without this field** — the plugin will not be resolvable. |
| `slug` | string | Yes | URL-safe slug. Must match what skills reference in `plugins[].name` |
| `name` | string | Yes | Human-readable display name |
| `version` | string | Yes | Semver version string |
| `description` | string | No | Brief description |
| `author` | string | No | Publisher name |
| `platforms` | object | Yes | Map of platform key to `PlatformBinary` |
| `signingKeyId` | string | No | ED25519 signing key ID |
| `envVar` | string | No | Custom env var name override. If empty, defaults to `{SLUG}_BIN` |
| `auth` | object | No | Authentication configuration. See [Authentication](#authentication) |
| `events` | array | No | Event declarations. See [Plugin Events](#plugin-events) |
| `dependencies` | array | No | Plugin-to-plugin dependencies. See [Plugin Dependencies](#plugin-to-plugin-dependencies) |
| `capabilities` | object | No | Structured capability declarations. See [Structured Capabilities](#structured-capabilities) |

> **Important:** The `id` field is required for all plugins. Without it, `PluginManifest` deserialization fails and the plugin cannot be resolved. Use the slug as the id (e.g., `"id": "gws"`).

### PlatformBinary

Each entry in the `platforms` map describes the binary for one platform:

| Field | Type | Description |
|-------|------|-------------|
| `binaryName` | string | Filename of the binary (e.g., `"gws"` or `"gws.exe"`) |
| `sha256` | string | SHA256 hex hash for integrity verification |
| `signature` | string | ED25519 signature (base64) |
| `size` | number | File size in bytes |
| `downloadUrl` | string | CDN URL or API path to download the binary |

---

## Authentication

Plugins that require user credentials (e.g., Google OAuth, API keys) can declare an `auth` block. Nebo provides HTTP endpoints and WebSocket events to drive the auth flow from the frontend.

```json
{
  "auth": {
    "type": "oauth_cli",
    "label": "Google Account",
    "description": "Authenticate with your Google Workspace account.",
    "commands": {
      "login": "auth login",
      "status": "auth status",
      "logout": "auth logout"
    },
    "env": {
      "GOOGLE_CLIENT_ID": "...",
      "GOOGLE_CLIENT_SECRET": "..."
    }
  }
}
```

### Auth Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | string | Yes | Auth type identifier (e.g., `"oauth_cli"`) |
| `label` | string | Yes | Button label in UI (e.g., `"Google Account"`) |
| `description` | string | No | Description shown to user during auth step |
| `commands.login` | string | Yes | CLI args appended to binary for login (e.g., `"auth login"`) |
| `commands.status` | string | No | CLI args for status check. Exit code 0 = authenticated |
| `commands.logout` | string | No | CLI args to clear credentials |
| `env` | object | No | Environment variables injected when running auth commands |

### Auth HTTP Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/plugins` | GET | Returns `hasAuth` and `authLabel` per plugin |
| `/api/v1/plugins/{slug}/auth/status` | GET | Runs status command. Returns `{ "authenticated": bool }` |
| `/api/v1/plugins/{slug}/auth/login` | POST | Spawns login command in background. Returns `{ "started": true }` |
| `/api/v1/plugins/{slug}/auth/logout` | POST | Runs logout command synchronously |

### Auth WebSocket Events

| Event | Payload | When |
|-------|---------|------|
| `plugin_auth_started` | `{ plugin, label }` | Login command spawned |
| `plugin_auth_url` | `{ plugin, url }` | OAuth URL discovered in output |
| `plugin_auth_complete` | `{ plugin }` | Login succeeded (exit code 0) |
| `plugin_auth_error` | `{ plugin, error }` | Login failed |

The login flow is asynchronous. Nebo spawns the plugin's login command, scans stderr/stdout for OAuth URLs, broadcasts them to the frontend via WebSocket, and also attempts `open::that()` as a server-side fallback.

---

## Plugin Events

Plugins can declare event-producing capabilities. When a long-running watch process outputs NDJSON to stdout, Nebo auto-emits each line into the EventBus. Other agents can subscribe to these events without knowing plugin internals.

```json
{
  "events": [
    {
      "name": "email.new",
      "description": "Fires when a new email arrives in Gmail",
      "command": "gmail +watch --format ndjson --project {{gcp_project}}"
    },
    {
      "name": "calendar.event",
      "description": "Fires on calendar event changes",
      "command": "calendar +watch --format ndjson",
      "multiplexed": true
    }
  ]
}
```

### Event Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Event name. Prefixed with plugin slug at runtime (e.g., `"gws.email.new"`) |
| `description` | string | `""` | Human-readable description of what triggers this event |
| `command` | string | required | CLI args for the watch process. Supports `{{key}}` template substitution from agent input values |
| `multiplexed` | bool | `false` | If true, NDJSON lines may contain an `"event"` field for multiplexing |

### NDJSON Protocol

Watch processes output one JSON object per line to stdout.

**Single event type** (`multiplexed: false`):

The entire line becomes the event payload, emitted under the declared event source.

```
{"messageId": "123", "from": "alice@example.com", "subject": "Hello"}
```

Nebo emits: `Event { source: "gws.email.new", payload: <entire line> }`

**Multiplexed** (`multiplexed: true`):

Each line may contain an `"event"` field that discriminates the event type. The field is stripped from the payload before emission.

```
{"event": "email.new", "messageId": "123", "from": "alice@example.com"}
{"event": "email.read", "messageId": "456"}
```

Nebo emits:
- `Event { source: "gws.email.new", payload: {"messageId": "123", "from": "alice@example.com"} }`
- `Event { source: "gws.email.read", payload: {"messageId": "456"} }`

If a multiplexed line has no `event` field, the declared event name is used as fallback.

### Agent Watch Triggers

Agents consume plugin events by declaring a watch trigger with `event` and `command` in `agent.json`. Always provide both — `event` enables EventBus auto-emission, `command` is the fallback if the event isn't found in the manifest:

```json
{
  "email-watcher": {
    "trigger": {
      "type": "watch",
      "plugin": "gws",
      "event": "email.new",
      "command": "gmail +watch --format ndjson",
      "restart_delay_secs": 5
    },
    "description": "React to new emails",
    "activities": [...]
  }
}
```

When `event` is set, the runtime first attempts to resolve the CLI command from the plugin's manifest. If the event is not found (plugin not installed, manifest missing the event, etc.), the `command` field is used instead. Without either, the watcher is silently skipped. The `{{key}}` placeholders in the command are substituted from the agent's input values.

Watch triggers with `event` set but no activities are also valid. They auto-emit into the EventBus without running any inline workflow, allowing other agents to subscribe to the events.

### Event Discovery Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/plugins/events` | GET | Lists all declared events across all installed plugins |
| `/api/v1/plugins/{slug}/events` | GET | Lists declared events for a specific plugin |

Response format:

```json
{
  "events": [
    {
      "plugin": "gws",
      "name": "email.new",
      "source": "gws.email.new",
      "description": "Fires when a new email arrives in Gmail",
      "multiplexed": false
    }
  ],
  "total": 1
}
```

---

## Plugin-to-Plugin Dependencies

Plugins can depend on other plugins. For example, `digest` needs `ffmpeg` for media extraction, and `nebo-office` may need `nebo-pdf` for shared rendering.

Declare dependencies in your `plugin.json`:

```json
{
  "id": "digest",
  "slug": "digest",
  "version": "1.2.0",
  "dependencies": [
    { "name": "ffmpeg", "version": ">=5.0.0" },
    { "name": "imagemagick", "version": "*", "optional": true }
  ]
}
```

### Dependency Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Dependency plugin slug |
| `version` | string | `"*"` | Semver version range (same syntax as skill plugin deps) |
| `optional` | bool | `false` | If true, the parent plugin installs even without this dep |

### How It Works

1. User installs your plugin (via `PLUG-XXXX-XXXX` code or as a skill dependency)
2. Nebo reads your manifest's `dependencies[]` field
3. Each required dependency is resolved and installed recursively (same cascade as skill→plugin deps)
4. Your plugin binary receives dependency binaries as environment variables: `{DEP_SLUG}_BIN`

```
digest installs → reads dependencies: [{name: "ffmpeg"}]
  → installs ffmpeg → digest runs with DIGEST_BIN + FFMPEG_BIN
```

### Cycle Protection

The dependency cascade uses a visited set. If plugin A depends on B and B depends on A, the second install is skipped (already visited). No infinite loops.

### Accessing Dependency Binaries

Your plugin binary receives its own binary path as `{SLUG}_BIN` and each dependency binary as `{DEP_SLUG}_BIN`:

```bash
#!/bin/bash
# digest plugin — uses ffmpeg for media processing
$FFMPEG_BIN -i "$INPUT" -f wav - | process_audio
```

---

## Structured Capabilities

Plugins can declare structured capabilities in their manifest. These are richer than the generic `plugin` tool — they give the agent typed tools with schemas, lifecycle hooks, slash commands, HTTP routes, AI provider adapters, and user-configurable settings.

A full `capabilities` declaration:

```yaml
capabilities:
  tools:
    - name: search_emails
      description: "Search Gmail emails"
      command: "search --query {query}"
      input_schema: { type: object, properties: { query: { type: string } } }
      approval: false
      timeout_seconds: 30
  hooks:
    - hook: tool.pre_execute
      hook_type: filter
      priority: 10
      command: "hook pre-execute"
      timeout_ms: 500
  commands:
    - name: sync
      description: "Sync all data"
      command: "sync --full"
  routes:
    - path: /oauth/callback
      method: GET
      command: "oauth-callback"
  providers:
    - name: openrouter
      description: "OpenRouter AI provider"
      command: "provider serve"
  config_schema:
    - key: GOOGLE_API_KEY
      label: "Google API Key"
      field_type: string
      required: true
      secret: true
    - key: SYNC_INTERVAL
      label: "Sync Interval"
      field_type: select
      required: false
      options: ["15m", "30m", "1h", "4h"]
```

Each capability type is detailed in the subsections below.

### Tools

Declare tools in `capabilities.tools[]`. Each tool becomes a typed, schema-validated tool available to the agent:

```json
{
  "capabilities": {
    "tools": [
      {
        "name": "gws.gmail.triage",
        "description": "Triage Gmail inbox — categorize, prioritize, and draft responses",
        "command": "gmail +triage",
        "inputSchema": {
          "type": "object",
          "properties": {
            "limit": { "type": "integer", "description": "Max emails to triage" },
            "label": { "type": "string", "description": "Gmail label filter" }
          }
        },
        "approval": true,
        "timeoutSeconds": 120
      }
    ]
  }
}
```

#### Tool Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Tool name exposed to the agent (e.g., `"gws.gmail.triage"`) |
| `description` | string | required | Description for the model to understand when to use this tool |
| `command` | string | required | CLI args appended to the plugin binary (e.g., `"gmail +triage"`) |
| `inputSchema` | object | generic object | JSON Schema for typed input validation |
| `approval` | bool | `false` | Whether this tool requires user approval before execution |
| `timeoutSeconds` | number | `120` | Maximum execution time in seconds |

#### How Typed Tools Work

1. Plugin installs → Nebo reads `capabilities.tools[]`
2. Tool definitions are routed through the consolidated STRAP `PluginTool`
3. Agent sees the tool with its schema and description
4. On execution: Nebo resolves the plugin binary, runs `<binary> <command>` with input as JSON on stdin, returns stdout

The generic `plugin(resource, action, command)` tool still works alongside structured tools — it's the fallback for commands not declared in capabilities.

#### The Command Catalog (v0.10.0+)

The `plugin` tool's description includes a **per-plugin command catalog** automatically — every installed plugin's available commands surface in the agent's tool schema upfront, with one-line descriptions. This means the model never needs a separate "discover commands" round-trip; if a command isn't in the catalog, it doesn't exist.

The catalog is built from two sources, in this order:

1. **`capabilities.tools[]` in `plugin.json`** — if you declare your tools here, their names + descriptions become the catalog entries. This is the preferred, authoritative form.
2. **Skill `SKILL.md` frontmatter** — for each `skills/<name>/SKILL.md` shipped with the plugin, Nebo reads the YAML frontmatter `description:` and renders the skill as a catalog entry. This is the fallback for plugins that ship skill docs but haven't (yet) declared `capabilities.tools[]`.

What this means for plugin authors:

- **Always include a one-line `description:` in every `SKILL.md` frontmatter.** The model reads it; bad/missing descriptions = bad calls.
- **Follow the `<slug>-<service>-<verb>` naming convention** for skill dirs (e.g., `gws-gmail-triage`). Nebo renders that as the command label `gmail +triage` in the catalog.
- **`capabilities.tools[]` overrides skill-derived entries** when both exist for the same name.

#### Removed in v0.10.0

The `search`, `skills`, `services`, and `help` actions on the `plugin` tool were removed — they were a competing pathway with the new upfront catalog. The only supported actions are now `exec` (run a command) and `events` (list declared NDJSON watch events). Calls to the removed actions return a clear error pointing the agent at the catalog.

### Hooks

Declare lifecycle hooks your plugin wants to subscribe to:

```json
{
  "capabilities": {
    "hooks": [
      {
        "hook": "tool.pre_execute",
        "hookType": "filter",
        "priority": 50,
        "command": "hooks tool-pre-execute",
        "timeoutMs": 500
      }
    ]
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `hook` | string | required | Hook point name (e.g., `"tool.pre_execute"`) |
| `hookType` | string | `"action"` | `"filter"` (can modify payload) or `"action"` (fire-and-forget) |
| `priority` | number | `100` | Lower runs first |
| `command` | string | required | CLI subcommand for the hook handler |
| `timeoutMs` | number | `500` | Timeout in milliseconds |

Hook types:

- **`filter`** — can modify or block the operation. The hook receives the payload as JSON on stdin and returns modified JSON on stdout. Returning a non-zero exit code blocks the operation.
- **`action`** — fire-and-forget side-effect. The return value is ignored.

Priority determines execution order: lower numbers run first. Default timeout is 500ms.

### Commands

Declare slash commands or app commands:

```json
{
  "capabilities": {
    "commands": [
      {
        "name": "/gmail",
        "description": "Quick access to Gmail operations",
        "command": "gmail",
        "slash": true
      }
    ]
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Command name (e.g., `"/gmail"`) |
| `description` | string | required | Human-readable description |
| `command` | string | required | CLI subcommand to execute |
| `slash` | bool | `false` | Register as a slash command in chat |

### Routes

Declare HTTP routes your plugin handles (e.g., OAuth callbacks):

```json
{
  "capabilities": {
    "routes": [
      {
        "path": "/gws/oauth/callback",
        "method": "GET",
        "command": "auth callback",
        "auth": "public"
      }
    ]
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | string | required | Route path |
| `method` | string | required | HTTP method (GET, POST, etc.) |
| `command` | string | required | CLI subcommand that handles the request |
| `auth` | string | `"jwt"` | `"public"` or `"jwt"` |

### Providers

Declare AI provider adapters (for custom model backends):

```json
{
  "capabilities": {
    "providers": [
      {
        "id": "openrouter",
        "displayName": "OpenRouter",
        "providerType": "model",
        "modelsCommand": "models list",
        "chatCommand": "chat stream",
        "authCommand": "auth setup"
      }
    ]
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `id` | string | required | Provider ID |
| `displayName` | string | required | Display name |
| `providerType` | string | required | `"model"`, `"speech"`, `"image"`, etc. |
| `modelsCommand` | string | required | CLI subcommand to list available models (JSON output) |
| `chatCommand` | string | required | CLI subcommand for streaming chat (NDJSON on stdout) |
| `authCommand` | string | — | CLI subcommand for auth setup |

### Config Schema (User Settings)

Declare user-configurable settings that render as a form in the UI. Values are stored in `plugin_settings` and injected as environment variables on every plugin execution.

```json
{
  "capabilities": {
    "configSchema": [
      {
        "key": "WORKSPACE_ID",
        "label": "Workspace ID",
        "description": "Your Asana workspace ID",
        "fieldType": "string",
        "required": true
      },
      {
        "key": "MAX_RESULTS",
        "label": "Max Results",
        "fieldType": "number",
        "default": "50"
      },
      {
        "key": "API_KEY",
        "label": "API Key",
        "fieldType": "string",
        "required": true,
        "secret": true
      },
      {
        "key": "LOG_LEVEL",
        "label": "Log Level",
        "fieldType": "select",
        "options": ["debug", "info", "warn", "error"],
        "default": "info"
      }
    ]
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `key` | string | required | Env var name injected on execution (e.g., `"MAX_RESULTS"`) |
| `label` | string | required | Display label in the settings form |
| `description` | string | `""` | Help text |
| `fieldType` | string | `"string"` | `"string"`, `"number"`, `"boolean"`, or `"select"` |
| `default` | string | — | Default value |
| `required` | bool | `false` | Whether the user must set this field |
| `secret` | bool | `false` | If true, value is stored with AES-256-GCM encryption |
| `options` | string[] | — | Available choices for `"select"` type |

> **Activation gate:** A plugin will not activate until all `required: true` config fields have been set by the user. The UI auto-generates a settings form from `configSchema` — users fill it in under Settings > Plugins.

---

## Permissions

Plugins can declare a permissions manifest that controls environment variable access, network needs, and execution limits. These are enforced at exec time.

```json
{
  "permissions": {
    "envAllow": ["HOME", "PATH", "WORKSPACE_ID"],
    "envDeny": ["AWS_SECRET_ACCESS_KEY", "GITHUB_TOKEN"],
    "network": true,
    "maxTimeoutSeconds": 300
  }
}
```

Or in YAML:

```yaml
permissions:
  env_allow: ["HOME", "PATH"]
  env_deny: ["AWS_SECRET_ACCESS_KEY"]
  network: true
  max_timeout_seconds: 300
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `envAllow` | string[] | `[]` (all allowed) | Env vars the plugin may read. Empty means all are allowed. |
| `envDeny` | string[] | `[]` | Env vars always stripped before execution (security blocklist) |
| `network` | bool | `false` | Whether the plugin needs network access |
| `maxTimeoutSeconds` | number | `300` | Hard cap on any single execution in seconds |

---

## Manifest Validation

Nebo validates `plugin.json` during installation. Invalid manifests are rejected before the binary is written to disk.

### Validation Rules

| Field | Rule |
|-------|------|
| `slug` | Required. Lowercase alphanumeric + hyphens only. No leading/trailing hyphens. Max 64 characters. |
| `version` | Required. Must be valid semver (e.g., `"1.2.3"`, not `"latest"`). |
| `platforms` | At least one platform entry required. |
| `binaryName` | No path separators (`/`, `\`). No `..`. Cannot be empty. |
| `auth.commands.login` | Must be non-empty if `auth` is present. |
| `events[].name` | Must be non-empty. No path separators. |
| `events[].command` | Must be non-empty. |

### Common Validation Errors

```
"slug is required"
"slug 'My Plugin!' contains invalid characters — use lowercase alphanumeric and hyphens"
"slug must not start or end with a hyphen"
"version 'latest' is not valid semver"
"platforms must have at least one entry"
"binary_name '../evil' contains path separator"
"auth.commands.login must be non-empty when auth is present"
```

---

## Plugin Runtime Environment

Plugins are **spawned on-demand** — each tool call, hook invocation, or command execution spawns a fresh process with the CLI arguments from the capability definition. There is no persistent plugin process between invocations. The exception is **watch triggers**, which spawn a long-running process that emits NDJSON events continuously.

Each invocation runs in a sandboxed environment with a controlled set of environment variables.

### Environment Variables

| Variable | Example | Description |
|----------|---------|-------------|
| `NEBO_APP_ID` | `gws` | Plugin identifier (from manifest) |
| `NEBO_APP_NAME` | `Google Workspace CLI` | Display name |
| `NEBO_APP_VERSION` | `1.2.3` | Manifest version |
| `NEBO_APP_DIR` | `~/.nebo/nebo/plugins/gws/1.2.3` | Plugin binary directory (code — replaceable) |
| `NEBO_APP_SOCK` | `~/.nebo/nebo/plugins/gws/1.2.3/gws.sock` | Unix socket path for gRPC |
| `NEBO_APP_DATA` | `~/.nebo/appdata/plugins/gws` | Persistent data directory (separate from code — survives upgrades) |
| `PATH` | system path | Standard path |
| `HOME` | user home | Home directory |
| `TMPDIR` | temp directory | Temporary files |
| `LANG` | locale | System locale |
| `TZ` | timezone | System timezone |

All other environment variables (API keys, secrets, database URLs) are **stripped** — your plugin must not depend on the user's shell environment.

### Data Persistence

Store all persistent data in `$NEBO_APP_DATA`. This directory is physically separated from the code directory — it lives at `~/.nebo/appdata/plugins/<slug>/`, not inside the version directory. This means:

- Upgrading the plugin binary never touches your data
- Your plugin is responsible for its own schema migrations across versions
- Data survives reinstalls, version upgrades, and Nebo updates

Common storage patterns:
- **SQLite** — for structured data and queries
- **JSON files** — for simple configuration state
- **File store** — for cached downloads, processed outputs

### Blocked Variables

These environment variables are always stripped for security:

`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_API_KEY`, `JWT_SECRET`, `DATABASE_URL`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `GITHUB_TOKEN`, `STRIPE_SECRET_KEY`

---

## Publishing a Plugin

### Prerequisites

- A NeboAI developer account
- Your binary compiled for at least one target platform
- Binary must be a single executable file (no runtime dependencies)

### Supported Platforms

| Platform Key | OS | Architecture |
|-------------|-----|--------------|
| `darwin-arm64` | macOS | Apple Silicon |
| `darwin-amd64` | macOS | Intel |
| `linux-arm64` | Linux | ARM64 |
| `linux-amd64` | Linux | x86_64 |
| `windows-arm64` | Windows | ARM64 |
| `windows-amd64` | Windows | x86_64 |

Publish for as many platforms as you support. At minimum, target `darwin-arm64` and `linux-amd64`.

### Step-by-Step

1. **Select your developer account:**

   ```
   developer(resource: account, action: select, id: "<your-dev-account-id>")
   ```

2. **Create the plugin artifact:**

   ```
   plugin(action: create, name: "gws", category: "connectors")
   ```

3. **Get an upload token (per platform):**

   ```
   plugin(action: binary-token, id: "<PLUGIN_ID>")
   ```

   This returns a curl command with a 5-minute expiry.

4. **Upload binary + manifest per platform:**

   Use the returned curl command via the command line, replacing the file path and platform:

   ```bash
   curl -X PUT "<upload-url>" \
     -F "binary=@./build/gws-darwin-arm64" \
     -F "platform=darwin-arm64" \
     -F "manifest=@./PLUGIN.md"
   ```

   Repeat for each platform you support.

5. **Submit for review:**

   ```
   plugin(action: submit, id: "<PLUGIN_ID>", version: "1.0.0")
   ```

### Plugin Tool Actions

| Action | Description |
|--------|-------------|
| `plugin(action: list)` | List all your plugins |
| `plugin(action: get, id: "...")` | Get plugin details |
| `plugin(action: create, name: "...", category: "...")` | Create a new plugin |
| `plugin(action: update, id: "...")` | Update plugin metadata |
| `plugin(action: delete, id: "...")` | Delete a plugin |
| `plugin(action: submit, id: "...", version: "...")` | Submit for review |
| `plugin(action: list-binaries, id: "...")` | List uploaded binaries |
| `plugin(action: binary-token, id: "...")` | Generate upload token |
| `plugin(action: delete-binary, id: "...")` | Delete a binary |

### Install Codes

After your plugin is approved, NeboAI assigns a `PLUG-XXXX-XXXX` install code. Users can paste this code into Nebo's chat to install the plugin directly. However, plugins are typically installed as dependencies of skills — users rarely install plugins standalone.

---

## Bundling Skills with Your Plugin

Plugins can embed skills inside a `.napp` archive. This is the preferred distribution method — when the plugin installs, its skills install automatically.

### .napp Archive Structure

```
gws.napp
├── manifest.json       # Package identity
├── plugin.json         # PluginManifest (the full manifest)
├── PLUGIN.md           # Plugin documentation
├── gws                 # Native binary (platform-specific)
└── skills/             # Embedded skills (optional)
    ├── gws-gmail/
    │   └── SKILL.md
    ├── gws-calendar/
    │   └── SKILL.md
    └── gws-drive/
        └── SKILL.md
```

After installation, embedded skills are discovered by the skill loader automatically. They appear as Tier 2.5 — between marketplace-installed skills and user skills.

---

## Directory Structure

What the user sees on disk after install:

```
~/.nebo/
  nebo/plugins/                          # CODE — replaceable on upgrade
    gws/
      1.2.3/
        manifest.json                    # Package identity
        plugin.json                      # Cached PluginManifest
        PLUGIN.md                        # Documentation
        gws                              # Your binary (chmod 755)
        skills/                          # Embedded skills (if bundled)
          gws-gmail/
            SKILL.md
          gws-calendar/
            SKILL.md

  appdata/plugins/                       # DATA — never touched by updates
    gws/
      sidecar.log                        # Process output
      cache.db                           # Your plugin's persistent data
      credentials.json                   # Whatever your plugin stores
```

Code and data are physically separated. The update system operates on `nebo/plugins/` but **never touches `appdata/plugins/`**. Your plugin's databases, caches, and user files survive all version upgrades and reinstalls — same model as iOS apps.

Multiple versions can coexist. Each skill resolves to the highest installed version matching its semver range.

---

## Security

- **SHA256 verification:** Every binary is hashed on upload. On download, the hash is verified before the binary is written to disk. Any mismatch = download rejected.
- **ED25519 signatures:** Binaries are signed with NeboAI's ED25519 key. Signatures are verified on download when the signing key is available.
- **Quarantine:** If a plugin is revoked (security issue, policy violation), Nebo deletes the binary and writes a `.quarantined` marker. The plugin becomes unresolvable, and any skills depending on it are dropped from the loaded set.
- **No network required after install:** Once downloaded, `resolve()` is fully local. Works offline.

---

## Versioning and Updates

- Multiple versions of the same plugin can coexist on disk (e.g., `gws/1.2.0/` and `gws/1.3.0/`)
- Each skill resolves to the highest installed version matching its semver range
- When you publish a new version, users get it automatically the next time Nebo resolves the dependency
- Old versions are cleaned up by garbage collection when no skill references them

### Garbage Collection

Nebo periodically checks which plugin slugs are referenced by loaded skills. Unreferenced plugin directories are removed. This is deferred (not eager) — plugins aren't deleted the moment a skill is uninstalled.

---

## Testing During Development

During development, you can manually place a plugin binary in the expected directory structure:

```bash
mkdir -p ~/Library/Application\ Support/nebo/plugins/my-plugin/0.1.0/
cp ./build/my-plugin ~/Library/Application\ Support/nebo/plugins/my-plugin/0.1.0/
chmod 755 ~/Library/Application\ Support/nebo/plugins/my-plugin/0.1.0/my-plugin
```

Then create a skill with `plugins: [{name: "my-plugin", version: "*"}]` in your `user/skills/` directory. The loader will resolve the plugin locally without contacting NeboAI.

### Testing Authentication

To test auth locally, create a `plugin.json` in the version directory with your auth config:

```json
{
  "id": "my-plugin",
  "slug": "my-plugin",
  "name": "My Plugin",
  "version": "0.1.0",
  "platforms": {},
  "auth": {
    "type": "oauth_cli",
    "label": "My Service",
    "description": "Connect to My Service",
    "commands": {
      "login": "auth login",
      "status": "auth status",
      "logout": "auth logout"
    },
    "env": {}
  }
}
```

The auth endpoints (`/api/v1/plugins/{slug}/auth/*`) read from this cached manifest.

### Testing Events

Declare events in your local `plugin.json`:

```json
{
  "id": "my-plugin",
  "slug": "my-plugin",
  "name": "My Plugin",
  "version": "0.1.0",
  "platforms": {},
  "events": [
    {
      "name": "item.created",
      "description": "Fires when a new item is created",
      "command": "watch --format ndjson"
    }
  ]
}
```

Then create an agent with a watch trigger referencing `event: "item.created"`. The watch process should output NDJSON to stdout.

---

## WebSocket Events

Events broadcast during plugin operations:

| Event | Payload | When |
|-------|---------|------|
| `plugin_installing` | `{ plugin, platform }` | Before download starts |
| `plugin_installed` | `{ plugin }` | After successful install |
| `plugin_error` | `{ plugin, error }` | On download/verify failure |
| `plugin_auth_started` | `{ plugin, label }` | Auth login begins |
| `plugin_auth_url` | `{ plugin, url }` | OAuth URL discovered in output |
| `plugin_auth_complete` | `{ plugin }` | Auth login succeeded |
| `plugin_auth_error` | `{ plugin, error }` | Auth login failed |

---

## Quick Reference

### Environment Variable Naming

`{SLUG}_BIN` — slug uppercased, hyphens become underscores.

### Install Code Prefix

`PLUG-XXXX-XXXX` — Crockford Base32, case-insensitive.

### SKILL.md Frontmatter

```yaml
plugins:
  - name: <slug>          # Required
    version: "<range>"    # Optional, default "*"
    optional: <bool>      # Optional, default false
```

### NeboAI Qualified Name

```
@org/plugins/name@version
```

Same scoping and version resolution rules as skills. See [Packaging](packaging.md).

### Complete plugin.json Example

```json
{
  "id": "gws",
  "slug": "gws",
  "name": "Google Workspace CLI",
  "version": "1.2.3",
  "description": "Google Workspace integration for email, calendar, and drive",
  "author": "NeboAI Inc.",
  "platforms": {
    "darwin-arm64": {
      "binaryName": "gws",
      "sha256": "a1b2c3...",
      "signature": "base64...",
      "size": 45678900,
      "downloadUrl": "https://cdn.neboai.com/..."
    }
  },
  "signingKeyId": "key-001",
  "envVar": "",
  "auth": {
    "type": "oauth_cli",
    "label": "Google Account",
    "description": "Authenticate with your Google Workspace account.",
    "commands": {
      "login": "auth login",
      "status": "auth status",
      "logout": "auth logout"
    },
    "env": {}
  },
  "events": [
    {
      "name": "email.new",
      "description": "Fires when a new email arrives",
      "command": "gmail +watch --format ndjson",
      "multiplexed": false
    },
    {
      "name": "calendar.event",
      "description": "Fires on calendar event changes",
      "command": "calendar +watch --format ndjson",
      "multiplexed": true
    }
  ],
  "dependencies": [
    {
      "name": "ffmpeg",
      "version": ">=5.0.0"
    }
  ],
  "capabilities": {
    "tools": [
      {
        "name": "gws.gmail.triage",
        "description": "Triage Gmail inbox — categorize, prioritize, and draft responses",
        "command": "gmail +triage",
        "inputSchema": {
          "type": "object",
          "properties": {
            "limit": { "type": "integer", "description": "Max emails to process" },
            "label": { "type": "string", "description": "Gmail label to filter" }
          }
        },
        "approval": true,
        "timeoutSeconds": 120
      },
      {
        "name": "gws.calendar.create",
        "description": "Create a Google Calendar event",
        "command": "calendar +create",
        "approval": true,
        "timeoutSeconds": 30
      }
    ],
    "hooks": [
      {
        "hook": "tool.pre_execute",
        "hookType": "filter",
        "priority": 50,
        "command": "hooks tool-pre-execute",
        "timeoutMs": 500
      }
    ],
    "commands": [
      {
        "name": "/gmail",
        "description": "Quick access to Gmail operations",
        "command": "gmail",
        "slash": true
      }
    ]
  }
}
```
