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

## How It Works

1. Publisher uploads a native binary to NeboLoop for each platform
2. Publisher creates a skill with `plugins:` in SKILL.md frontmatter
3. User installs the skill (via marketplace or `SKIL-XXXX-XXXX` code)
4. Nebo detects the plugin dependency and downloads the binary silently
5. Binary is stored locally at `<data_dir>/nebo/plugins/<slug>/<version>/`
6. Skill scripts access the binary via `{SLUG}_BIN` environment variable

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
| `name` | string | required | Plugin slug (must match the plugin's registered slug in NeboLoop) |
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

---

## Using Plugin Binaries in Scripts

Plugin binaries are exposed to your scripts as environment variables. The naming convention is `{SLUG}_BIN` where the slug is uppercased and hyphens become underscores.

| Plugin Slug | Environment Variable |
|-------------|---------------------|
| `gws` | `GWS_BIN` |
| `ffmpeg` | `FFMPEG_BIN` |
| `my-tool` | `MY_TOOL_BIN` |

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
  "id": "plugin-uuid-123",
  "slug": "gws",
  "name": "Google Workspace CLI",
  "version": "1.2.3",
  "description": "Google Workspace integration for email, calendar, and drive",
  "author": "NeboLoop Inc.",
  "platforms": {
    "darwin-arm64": {
      "binaryName": "gws",
      "sha256": "a1b2c3...",
      "signature": "base64...",
      "size": 45678900,
      "downloadUrl": "https://cdn.neboloop.com/plugins/gws/1.2.3/darwin-arm64/gws"
    },
    "linux-amd64": {
      "binaryName": "gws",
      "sha256": "d4e5f6...",
      "signature": "base64...",
      "size": 42000000,
      "downloadUrl": "https://cdn.neboloop.com/plugins/gws/1.2.3/linux-amd64/gws"
    }
  },
  "signingKeyId": "key-001",
  "envVar": "",
  "auth": { ... },
  "events": [ ... ]
}
```

### Manifest Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | Yes | NeboLoop artifact ID (assigned on create) |
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

Agents consume plugin events by declaring a watch trigger with an `event` field in `agent.json`:

```json
{
  "email-watcher": {
    "trigger": {
      "type": "watch",
      "plugin": "gws",
      "event": "email.new",
      "restart_delay_secs": 5
    },
    "description": "React to new emails",
    "activities": [...]
  }
}
```

When `event` is set, the watch command is resolved from the plugin's manifest — no need to hardcode CLI args in the agent config. The `{{key}}` placeholders in the manifest command are substituted from the agent's input values.

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

## Publishing a Plugin

### Prerequisites

- A NeboLoop developer account
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

After your plugin is approved, NeboLoop assigns a `PLUG-XXXX-XXXX` install code. Users can paste this code into Nebo's chat to install the plugin directly. However, plugins are typically installed as dependencies of skills — users rarely install plugins standalone.

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
<data_dir>/nebo/plugins/
  gws/
    1.2.3/
      manifest.json    # Package identity
      plugin.json      # Cached PluginManifest
      PLUGIN.md        # Documentation
      gws              # Your binary (chmod 755)
      skills/          # Embedded skills (if bundled)
        gws-gmail/
          SKILL.md
        gws-calendar/
          SKILL.md
```

Multiple versions can coexist. Each skill resolves to the highest installed version matching its semver range.

---

## Security

- **SHA256 verification:** Every binary is hashed on upload. On download, the hash is verified before the binary is written to disk. Any mismatch = download rejected.
- **ED25519 signatures:** Binaries are signed with NeboLoop's ED25519 key. Signatures are verified on download when the signing key is available.
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

Then create a skill with `plugins: [{name: "my-plugin", version: "*"}]` in your `user/skills/` directory. The loader will resolve the plugin locally without contacting NeboLoop.

### Testing Authentication

To test auth locally, create a `plugin.json` in the version directory with your auth config:

```json
{
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

### NeboLoop Qualified Name

```
@org/plugins/name@version
```

Same scoping and version resolution rules as skills. See [Packaging](packaging.md).

### Complete plugin.json Example

```json
{
  "id": "abc-123",
  "slug": "gws",
  "name": "Google Workspace CLI",
  "version": "1.2.3",
  "description": "Google Workspace integration for email, calendar, and drive",
  "author": "NeboLoop Inc.",
  "platforms": {
    "darwin-arm64": {
      "binaryName": "gws",
      "sha256": "a1b2c3...",
      "signature": "base64...",
      "size": 45678900,
      "downloadUrl": "https://cdn.neboloop.com/..."
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
  ]
}
```
