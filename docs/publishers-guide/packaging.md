# Packaging & Marketplace

This guide covers the packaging format for all Nebo marketplace artifacts, artifact naming, and the artifact hierarchy.

For artifact-specific specs, see:

- [Skills](skills.md)
- [Plugins](plugins.md)
- [Apps](apps.md)
- [Platform Capabilities](platform-capabilities.md)
- [Workflows](workflows.md)
- [Agents](agents.md)
- [MCP Integrations](mcp.md)

## Hierarchy

```
APP  >  AGENT  >  WORK  >  SKILL
(UI)   (job)   (procedure) (knowledge + actions)
```

This is the **design direction** — the order in which you think about building. Start with knowledge and actions (Skill), chain those into procedures (Workflow), compose procedures into a job (Agent), and add a dedicated UI when chat isn't enough (App). The `>` represents conceptual priority, not a runtime dependency.

Each layer auto-installs its dependencies downward:

- **App** → an Agent with a frontend UI and optional sidecar binary
- **Agent** → installs Workflows and Skills declared in `agent.json`
- **Workflow** → installs Skills declared in `workflow.json` dependencies
- **Skill** → leaf node. No dependencies to auto-install.

**Plugins** are shared native binaries that bundle skills and provide platform capabilities (Google Workspace CLI, browser automation, etc.). They sit alongside the hierarchy — skills and agents declare which plugins they need.

**Platform Capabilities** (storage, network, vision, calendar, email, browser) are provided by Nebo itself — they are infrastructure, not marketplace artifacts. Skills declare which capabilities they need; the platform provides them. See [Platform Capabilities](platform-capabilities.md).

**Design direction:** Start with what the agent needs to *know* and *do* (Skill), chain skills into repeatable procedures (Workflow), compose procedures into a job (Agent), and wrap in a dedicated UI if chat output isn't enough (App). Skills are the universal unit — a folder with a SKILL.md file, optionally bundling scripts, reference docs, and assets. This is the same format used by the broader Agent Skills ecosystem (agentskills.io), so skills from Anthropic, OpenAI, OpenClaw, and other compatible platforms work in Nebo without modification.

---

## Artifact Naming

Every artifact has two identifiers: a **qualified name** (canonical, for engineering) and an **install code** (alias, for sharing).

### Qualified Names

The qualified name is the canonical identifier for every artifact. It is human-readable, org-scoped, type-aware, and versioned.

Format: `@org/type/name@version`

```
@acme/skills/sales-qualification@1.0.0
@acme/skills/crm-lookup@1.0.0
@acme/workflows/lead-qualification@2.1.0
@nebo/agents/chief-of-staff@1.0.0
```

Read left to right: who published it, what kind of artifact it is, what it's called, what version.

| Segment | Description | Rules |
|---------|-------------|-------|
| `@org` | Publisher org (scoped) | Lowercase, alphanumeric + hyphens |
| `type` | Artifact type | `skills`, `workflows`, `agents` (apps use `agents` with `artifact_type: "app"`) |
| `name` | Artifact name | Lowercase, alphanumeric + hyphens |
| `@version` | Semver version (optional) | Omit for latest; supports semver ranges |

**Version resolution:**

| Reference | Resolves To |
|-----------|-------------|
| `@acme/skills/crm-lookup` | Latest published version |
| `@acme/skills/crm-lookup@1.0.0` | Exact version 1.0.0 |
| `@acme/skills/crm-lookup@^1.0.0` | Compatible with 1.0.0 (>=1.0.0 <2.0.0) |
| `@acme/skills/crm-lookup@>=1.0.0` | Any version 1.0.0 or higher |

Semver range semantics follow npm conventions. Exact versions pin to a specific release. Caret (`^`) allows patch and minor updates. Tilde (`~`) allows patch updates only.

**Namespacing:** Two publishers can build artifacts with the same name without collision:

```
@acme/skills/crm-lookup
@nebo/skills/crm-lookup
```

Different artifacts, no conflict. The org scope prevents name collisions as the marketplace grows.

### Install Codes (Aliases)

Install codes are short, shareable aliases assigned by the marketplace. They exist for one purpose: so a non-technical user can text a code to a friend and have it work.

Format: `PREFIX-XXXX-XXXX` — Crockford Base32 (`0123456789ABCDEFGHJKMNPQRSTVWXYZ`, excludes I, L, O, U), case-insensitive.

| Prefix | Artifact | Example |
|--------|----------|---------|
| `NEBO` | Link bot to NeboLoop account | `NEBO-A1B2-C3D4` |
| `SKIL` | Install a skill | `SKIL-R7KP-2M9V` |
| `WORK` | Install a workflow | `WORK-5TG2-XBJK` |
| `AGNT` | Install an agent | `AGNT-9DCE-4MPA` |
| `APPX` | Install an app | `APPX-3FKT-7WNP` |
| `LOOP` | Join bot to a Loop | `LOOP-7YSR-6WN3` |
| `PLUG` | Install a plugin | `PLUG-4HVT-8KRP` |

Install codes always resolve to `@latest`. They are detected case-insensitively in chat messages and dispatched automatically.

**Install codes are marketing artifacts. Qualified names are engineering identifiers.** Inside `workflow.json`, `agent.json`, dependency declarations, and all structured data — use qualified names. Install codes are for users, not for code.

---

## Package Format

### Skills — Directory Format (Agent Skills Standard)

Skills follow the [Agent Skills standard](https://agentskills.io). A skill is a **directory** containing a `SKILL.md` file and optional supporting resources. No manifest.json is required — the SKILL.md frontmatter is the source of truth.

```
skill-name/
├── SKILL.md           # Required — YAML frontmatter + markdown instructions
├── scripts/           # Optional — executable scripts (Python, TypeScript)
├── references/        # Optional — docs loaded on demand
├── assets/            # Optional — templates, images, fonts
└── examples/          # Optional — sample data, examples
```

Skills from Anthropic, OpenAI, OpenClaw, and other Agent Skills-compatible platforms can be installed directly with no modification. See [Skills](skills.md) for the full format spec.

**Marketplace distribution:** When a skill is submitted to NeboLoop, it is packaged as a signed `.napp` archive for integrity verification. At install time, Nebo reads the SKILL.md frontmatter and auto-generates internal registry metadata. The publisher never writes a manifest.json for skills.

**User/development path:** During development, place a skill directory directly in `user/skills/` and iterate. Hot-reload picks up changes with a 1-second debounce.

### Apps — Agent + Frontend UI

Apps are agents with their own UI. They use the `agents` qualified name type with `artifact_type: "app"` in their manifest. An app is a **directory** containing an `AGENT.md`, a `manifest.json`, and a `ui/` subdirectory with the frontend.

```
my-app/
├── AGENT.md              # Required — persona + instructions
├── manifest.json         # Required — identity, permissions, window config
├── agent.json            # Optional — workflows, skills, pricing
├── ui/                   # Required — static frontend
│   ├── index.html
│   ├── style.css
│   └── app.js
└── sidecar/              # Optional — native backend binary
```

The `manifest.json` extends the standard agent manifest with app-specific fields:

```json
{
  "id": "deal-tracker",
  "name": "@acme/agents/deal-tracker",
  "version": "1.0.0",
  "description": "Track real estate deals with AI-powered analysis.",
  "artifact_type": "app",
  "permissions": ["storage:readwrite", "subagent:invoke", "network:outbound"],
  "window": {
    "title": "Deal Tracker",
    "width": 1024,
    "height": 768,
    "resizable": true
  }
}
```

| Field | Description |
|-------|-------------|
| `artifact_type` | Must be `"app"` — this is what distinguishes apps from regular agents |
| `permissions` | Capabilities the app requires (storage, network, subagent, etc.) |
| `window` | Default window dimensions and title for the desktop app |

Apps use the `@neboai/app-sdk` for storage, agent invocation, identity, embedded chat, and direct LLM calls. See [Apps](apps.md) for the full spec.

**User/development path:** Place app directories in `~/.nebo/user/agents/` and iterate. Hot-reload picks up changes.

**Marketplace distribution:** Apps are packaged as sealed `.napp` archives like agents. The `ui/` directory contents are included in the archive (max 5MB per file).

### Workflows and Agents — .napp Archive

Workflows and agents are distributed as `.napp` files — signed `tar.gz` archives.

```
@acme/workflows/lead-qualification-1.0.0.napp
  → manifest.json
  → workflow.json
  → WORKFLOW.md

@nebo/agents/chief-of-staff-1.0.0.napp
  → manifest.json
  → agent.json
  → AGENT.md
  → skills/             # Optional — bundled SKILL.md files
  →   skills/crm-lookup/SKILL.md
```

### .napp Envelope Format

Every `.napp` file wraps the inner tar.gz in a binary envelope that is verified before the payload is touched:

```
[4B magic "NAPP"] [1B version 0x01] [64B ED25519 signature] [32B SHA256 hash] [payload...]
```

Verification order:
1. **SHA256** — cheap integrity check of the payload bytes.
2. **ED25519** — proves the archive was signed by NeboLoop. The signature covers `hash || payload`.

The NeboLoop public key is embedded at compile time (`neboloop_public_key.bin`) so verification works offline (first launch, air-gapped installs). A `SigningKeyProvider` also fetches the key from `GET /api/v1/apps/signing-key` with a 24-hour cache for key rotation.

### Sealed Archives

For workflows and agents, the `.napp` is **never extracted**. Nebo reads files directly from the archive at runtime. The signed archive is the running artifact — if someone tampers with the file, the next read fails signature verification. This provides continuous integrity, not just point-in-time verification at install.

| Artifact | Storage | Integrity Model |
|----------|---------|-----------------|
| Skill | Directory on disk (marketplace: sealed `.napp`) | Frontmatter is source of truth; marketplace archives are signed |
| Workflow | `.napp` sealed | Archive is the signed artifact — continuous integrity |
| Agent | `.napp` sealed | Archive is the signed artifact — continuous integrity |
| App | Directory (dev) / `.napp` sealed (marketplace) | Same as agent — includes `ui/` directory contents |

### License-Key Sealed Archives

Paid marketplace artifacts use an additional encryption layer on top of the `.napp` envelope. After envelope verification, the inner payload is AES-256-GCM encrypted with a per-artifact license key:

```
.napp envelope  →  unwrap (verify ED25519 + SHA256)  →  sealed payload  →  unseal (AES-256-GCM)  →  plain tar.gz
```

**Key derivation:** `derive_license_key(master_secret, artifact_id)` uses HKDF-SHA256 with the artifact ID as salt and `neboloop-license-v1` as info. The same key works regardless of who holds the license — authorization is server-side, so `.napp` files never need re-download on license transfer.

**Detection:** Plain `.napp` payloads start with gzip magic bytes (`0x1f 0x8b`). Sealed payloads start with a 12-byte random nonce, which won't match. `is_sealed()` checks this.

**Partial extraction:** For sealed skill archives, only executables (`scripts/`, `bin/`, `binary`) and metadata (`manifest.json`, `plugin.json`, `signatures.json`) are extracted to disk. IP-sensitive files (SKILL.md, references, assets) stay inside the sealed `.napp` and are read in memory at runtime — plaintext never touches disk.

**Runtime:** The skill loader holds license keys in memory via `set_license_keys(HashMap<artifact_id, [u8; 32]>)`. Keys are cached from NeboLoop at startup and used transparently when reading sealed entries.

### Versioned Storage

Artifacts are stored by qualified name and version:

```
nebo/                                    # From NeboLoop marketplace
  skills/
    @acme/skills/sales-qualification/
      1.0.0.napp
      1.1.0.napp
  workflows/
    @acme/workflows/lead-qualification/
      1.0.0.napp
  agents/
    @nebo/agents/chief-of-staff/
      1.0.0.napp

user/                                    # User-created (dev/sideload path)
  skills/
    my-custom-skill/
      SKILL.md                           # Loose directory — no archive, no signature
      scripts/
      references/
  workflows/
    my-workflow/
      workflow.json
      WORKFLOW.md
  agents/
    my-agent/
      agent.json
      AGENT.md
      skills/            # Optional — bundled skills
        my-skill/
          SKILL.md
    my-app/              # Apps live alongside agents
      manifest.json      #   artifact_type: "app"
      AGENT.md
      ui/                #   Static frontend
        index.html
      sidecar/           #   Optional native binary
```

**Marketplace artifacts** (`nebo/`) are sealed `.napp` files. Signed, versioned, read from archive at runtime.

**User artifacts** (`user/`) are loose files on disk. No archive, no signatures. This is the development path — edit directly, hot-reload picks up changes.

### Runtime Data — `appdata/`

Artifact runtime data (databases, caches, user files) lives in a completely separate tree:

```
appdata/                                 # Runtime data — NEVER touched by updates
  plugins/
    gws/                                 # Plugin data (survives all version upgrades)
      cache.db
      sidecar.log
  skills/
    my-custom-skill/                     # Skill data (survives reinstalls)
      output.json
  agents/
    deal-tracker/                        # Agent app data (survives upgrades)
      deals.db
      sidecar.log
```

This follows the iOS model: code and data live in physically separate containers. The update system operates on `nebo/` and `user/` but **never touches `appdata/`**. Artifacts own their data and are responsible for their own schema migrations across versions.

Environment variables point to the data directory:
- **Plugins/apps:** `NEBO_APP_DATA` → `~/.nebo/appdata/plugins/<slug>/` or `~/.nebo/appdata/agents/<id>/`
- **Skills:** `${NEBO_DATA_DIR}` template variable → `~/.nebo/appdata/skills/<name>/`

### Version Resolution

When a qualified name with a version range is referenced (e.g., `@acme/skills/sales-qualification@^1.0.0`), the resolution logic is:

1. Scan installed versions under that qualified name
2. Pick the highest version satisfying the semver range
3. If nothing matches, fetch from NeboLoop marketplace
4. If fetch fails and nothing is installed, error

This resolution applies everywhere a versioned qualified name appears — workflow `dependencies`, activity `skills` arrays, agent `workflows` refs, and agent-level `skills` arrays.

### Reading from Archives

Nebo reads `.napp` entries by name at runtime using a thin reader:

```rust
fn read_napp_entry(path: &Path, entry_name: &str) -> Result<Vec<u8>>
```

The skill loader reads `SKILL.md` from marketplace archives. The workflow engine calls `read_napp_entry(path, "workflow.json")`. The agent loader calls `read_napp_entry(path, "agent.json")` and `read_napp_entry(path, "AGENT.md")`. One function, used everywhere.

---

## manifest.json — Workflows, Agents, and Apps

Workflows, agents, and apps ship a `manifest.json` as their marketplace identity envelope. **Skills do not use manifest.json** — their identity comes from SKILL.md frontmatter.

```json
{
  "name": "@acme/workflows/lead-qualification",
  "version": "1.0.0",
  "description": "Qualifies inbound leads through research, scoring, and routing",
  "signature": {
    "algorithm": "ed25519",
    "key_id": "nebo-prod-2026"
  }
}
```

### Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | string | yes | — | Qualified name: `@org/type/artifact-name` |
| `version` | string | yes | — | Semantic version |
| `description` | string | no | `""` | Short description |
| `tags` | string[] | no | `[]` | Categorization tags |
| `signature` | object | no | — | ED25519 signing metadata |

The `name` field is the canonical identifier. The org, type, and artifact name are all parsed from it. There is no separate `id`, `type`, or `author` field — the qualified name carries all of that.

**App manifests** add these fields on top of the standard manifest:

| Field | Type | Description |
|-------|------|-------------|
| `artifact_type` | string | Must be `"app"` to mark this agent as an app |
| `permissions` | string[] | Required capabilities (`storage:readwrite`, `network:outbound`, etc.) |
| `window` | object | Default window config (`title`, `width`, `height`, `resizable`) |

> **Install codes** are not part of the package. They are assigned by NeboLoop when the manifest is submitted and are stored server-side as an alias that resolves to the qualified name. The publisher never sets the code — they submit their package and NeboLoop assigns one.

### Manifest Examples

**Workflow:**

```json
{
  "name": "@acme/workflows/lead-qualification",
  "version": "1.0.0",
  "description": "Qualifies inbound leads through CRM lookup, scoring, and routing"
}
```

**Agent:**

```json
{
  "name": "@nebo/agents/chief-of-staff",
  "version": "1.0.0",
  "description": "Never be blindsided again — morning briefing, day monitoring, evening wrap"
}
```

---

## agent.json — Extended Fields

Beyond workflows, skills, and pricing, `agent.json` supports sidecar tool definitions, named tool scopes, and memory configuration.

### Sidecar Tools

Each entry in the `tools` array becomes a native tool registered for the agent. The LLM sees `list_projects(...)` directly — calls are routed to the sidecar HTTP endpoint.

```json
{
  "tools": [
    {
      "name": "list_projects",
      "description": "List all projects",
      "method": "GET",
      "path": "/projects"
    },
    {
      "name": "get_document",
      "description": "Get a document by ID",
      "method": "GET",
      "path": "/documents/{id}",
      "input_schema": {
        "type": "object",
        "properties": { "id": { "type": "string" } },
        "required": ["id"]
      }
    }
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Action name the LLM calls (e.g., `list_projects`) |
| `description` | string | Human-readable description for the model |
| `method` | string | HTTP method for the sidecar endpoint (`GET`, `POST`, `PUT`, `DELETE`) |
| `path` | string | Sidecar-relative path, optionally with `{param}` placeholders |
| `input_schema` | object | Optional JSON Schema for input parameters |

### Tool Scopes

Named scopes restrict which sidecar tools, skills, and plugins are active when an embed chat is mounted with a specific scope name via the SDK.

```json
{
  "scopes": {
    "editor": {
      "tools": ["get_document", "update_document"],
      "skills": ["skills/document-editing"],
      "plugins": ["gws"]
    },
    "projects": {
      "tools": ["list_projects"]
    }
  }
}
```

Each scope maps to a subset of the agent's capabilities. The embed SDK mounts a chat with `scope: "editor"` to filter down to just those tools and skills.

### Memory Configuration

Controls how memories are isolated and inherited across the 3-tier hierarchy (user / agent / context).

```json
{
  "memory": {
    "inherit_user": true,
    "context_isolated": true
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `inherit_user` | boolean | `false` | When true, agent can READ the user's main memories (read-only, `tacit/preferences` only) |
| `context_isolated` | boolean | `false` | When true, memories are isolated per `contextId` from SDK embed sessions |

---

## Plugin Manifest — Capabilities and Permissions

Plugins declare structured capabilities and permissions in `plugin.json`. These are in addition to the base manifest fields (slug, version, platforms, auth, events, dependencies).

### Capabilities Block

```json
{
  "capabilities": {
    "tools": [
      {
        "name": "gws.gmail.triage",
        "description": "Triage unread emails",
        "command": "gmail +triage",
        "approval": true,
        "timeout_seconds": 120
      }
    ],
    "hooks": [
      {
        "hook": "pre_send",
        "hookType": "filter",
        "priority": 50,
        "command": "hooks pre-send",
        "timeout_ms": 500
      }
    ],
    "commands": [
      {
        "name": "/gmail",
        "description": "Gmail operations",
        "command": "gmail",
        "slash": true
      }
    ],
    "routes": [
      {
        "path": "/gws/oauth/callback",
        "method": "GET",
        "command": "auth callback",
        "auth": "public"
      }
    ],
    "providers": [
      {
        "id": "openrouter",
        "displayName": "OpenRouter",
        "providerType": "model",
        "modelsCommand": "models list",
        "chatCommand": "chat stream"
      }
    ],
    "configSchema": [
      {
        "key": "MAX_RESULTS",
        "label": "Max Results",
        "description": "Maximum number of results to return",
        "fieldType": "number",
        "default": "10",
        "required": false,
        "secret": false
      },
      {
        "key": "API_TOKEN",
        "label": "API Token",
        "fieldType": "string",
        "required": true,
        "secret": true
      }
    ]
  }
}
```

All capabilities are executed out-of-process via the plugin binary CLI. The `configSchema` fields are rendered as a settings form in the UI; values are stored in `plugin_settings` and injected as env vars on execution. Fields with `secret: true` are stored encrypted.

### Permissions Block

```json
{
  "permissions": {
    "envAllow": ["HOME", "PATH", "GWS_BIN"],
    "envDeny": ["AWS_SECRET_ACCESS_KEY"],
    "network": true,
    "maxTimeoutSeconds": 300
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `envAllow` | string[] | `[]` (all allowed) | Env vars the plugin may read |
| `envDeny` | string[] | `[]` | Env vars always stripped before execution |
| `network` | boolean | `false` | Whether the plugin needs network access (informational) |
| `maxTimeoutSeconds` | number | `300` | Maximum execution timeout for any tool call |

---

## Separation of Concerns

Each artifact type has a clear split between identity, domain logic, and prose:

- **Skills** — `SKILL.md` frontmatter (identity + runtime config) + body (knowledge) + optional bundled resources (scripts, references, assets). No manifest.json. Frontmatter is the source of truth. Compatible with the Agent Skills standard.
- **Workflows** — `manifest.json` (marketplace identity) + `workflow.json` (procedure definition) + `WORKFLOW.md` (agent docs). Marketplace identity lives in the manifest; the workflow.json carries its own `id` for the local engine (REST API, run records, the `work` tool). Sealed archive.
- **Agents** — `manifest.json` (marketplace identity) + `agent.json` (event bindings, pricing, defaults, sidecar tools, scopes, memory config) + `AGENT.md` (persona prose) + optional `skills/` directory (bundled skills). Sealed archive.
- **Apps** — everything an Agent has + `artifact_type: "app"` in manifest + `ui/` directory (static frontend) + optional sidecar binary. The `manifest.json` adds `permissions` and `window` config. Apps use the `@neboai/app-sdk` for storage, agent invocation, identity, embedded chat, and direct LLM calls.

---

## Quick Reference

### Timeout Constants

| Operation | Timeout |
|-----------|---------|
| Hook call | 500 ms |
| Hook circuit breaker | 3 consecutive failures |
| Hook circuit breaker recovery | 5 minutes |
| Signing key cache | 24 hours |
| Revocation cache | 1 hour |
| Plugin tool default timeout | 120 seconds |
| Plugin max timeout (permissions) | 300 seconds |

### Qualified Name Format

`@org/type/name@version` — org and name are lowercase alphanumeric + hyphens. Type is one of: `skills`, `workflows`, `agents`. Version follows semver.

### Install Code Format

`PREFIX-XXXX-XXXX` — Crockford Base32, case-insensitive. Always resolves to `@latest`.

Prefixes: `NEBO`, `SKIL`, `WORK`, `AGNT`, `APPX`, `LOOP`, `PLUG`
