# Permission System - Rust SME Reference

> Definitive reference for the Nebo Rust permission system. Covers global
> settings, capability permissions, per-entity overrides, origin policy, hard
> safeguards, real-time approvals, channel behavior, database schema, API
> endpoints, and frontend components.

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Layer 1: Global Settings](#2-layer-1-global-settings)
3. [Layer 2: Capability Permissions](#3-layer-2-capability-permissions)
4. [Layer 3: Per-Entity Overrides](#4-layer-3-per-entity-overrides)
5. [Layer 4: Origin-Based Policy](#5-layer-4-origin-based-policy)
6. [Hard Safeguards](#6-hard-safeguards)
7. [Tool Execution Pipeline](#7-tool-execution-pipeline)
8. [Capability Mapping](#8-capability-mapping)
9. [Resource Permits](#9-resource-permits)
10. [Permission Resolution Flow](#10-permission-resolution-flow)
11. [Real-Time Approval Flow](#11-real-time-approval-flow)
12. [Full Access](#12-full-access)
13. [REST API Endpoints](#13-rest-api-endpoints)
14. [WebSocket Messages](#14-websocket-messages)
15. [Frontend Components](#15-frontend-components)
16. [Database Schema](#16-database-schema)
17. [Special Callers](#17-special-callers)
18. [File Reference](#18-file-reference)

---

## 1. Architecture Overview

The permission system is layered. Approval prompts are produced by the agent
runner before registry execution; hard denials still happen inside the registry.

```
Layer 1: Global settings
  settings.full_access, auto_install_deps, comm/developer/update toggles

Layer 2: Capability permissions
  user_profiles.tool_permissions JSON and canonical capability metadata

Layer 3: Per-entity overrides
  entity_config.permissions and entity_config.resource_grants

Layer 4: Origin policy
  tools::policy hard-denies unsafe origin/tool combinations

Hard safeguards
  tools::safeguard blocks sudo, protected-path writes, destructive disk ops, etc.
```

Important split:

- `Runner` decides whether a tool call needs an interactive approval prompt.
- `Registry::execute()` enforces hard safeguards, origin policy, capability
  gates, and resource grants before the tool actually runs.
- Full Access bypasses the runner approval prompt gate. It does not bypass hard
  safeguards or origin policy.

---

## 2. Layer 1: Global Settings

**Source:** `crates/db/migrations/0109_full_access.sql`,
`crates/db/migrations/0110_approved_commands.sql`
**Model:** `crates/db/src/models.rs` - `Setting`

The `settings` table is a singleton row (`id = 1`) with global toggles:

| Column | Type | Default | Purpose |
|--------|------|---------|---------|
| `auto_install_deps` | INTEGER bool | `0` | Allow dependency installer flows to run automatically when enabled by caller. |
| `auto_approve_read` | INTEGER bool | `1` | Legacy/low-level read approval setting retained for compatibility. |
| `auto_approve_write` | INTEGER bool | `0` | Legacy/low-level write approval setting retained for compatibility. |
| `auto_approve_bash` | INTEGER bool | `0` | Legacy/low-level shell approval setting retained for compatibility. |
| `heartbeat_interval_minutes` | INTEGER | `30` | Default heartbeat interval. |
| `comm_enabled` | INTEGER bool | `0` | NeboAI communication toggle. |
| `comm_plugin` | TEXT | `''` | Active comm plugin name. |
| `developer_mode` | INTEGER bool | `0` | Developer mode toggle. |
| `auto_update` | INTEGER bool | `1` | Auto-update toggle. |
| `full_access` | INTEGER bool | `0` | Master approval bypass for agent-runner tool approvals. |

```rust
pub struct Setting {
    pub id: i64,
    pub auto_install_deps: i64,
    pub auto_approve_read: i64,
    pub auto_approve_write: i64,
    pub auto_approve_bash: i64,
    pub heartbeat_interval_minutes: i64,
    pub comm_enabled: i64,
    pub comm_plugin: String,
    pub developer_mode: i64,
    pub auto_update: i64,
    pub full_access: i64,
    pub updated_at: i64,
}
```

`full_access` is loaded in `chat_dispatch::resolve_full_access()` and passed into
`RunRequest` by both `run_chat()` and `run_chat_events()`.

---

## 3. Layer 2: Capability Permissions

**Source:** `crates/tools/src/capabilities.rs`
**Storage:** `user_profiles.tool_permissions` JSON string

Global permissions control which capability categories the agent can use. The
canonical categories are served by the backend; the frontend renders that list
instead of maintaining a separate hardcoded taxonomy.

```json
{
  "chat": true,
  "file": true,
  "shell": false,
  "web": true,
  "contacts": false,
  "desktop": false,
  "media": false,
  "system": false
}
```

| Category | Scope |
|----------|-------|
| `chat` | Core conversation and memory behavior. |
| `file` | File reads/writes/edits/searches. |
| `shell` | Shell commands and terminal processes. |
| `web` | Browser, fetch, search, and loop/web-like access. |
| `contacts` | Contacts operations. |
| `desktop` | Window/input/UI automation. |
| `media` | Screenshots, image/media capture, TTS/STT-like media operations. |
| `system` | System integrations such as keychain/search/notifications. |

The `user_profiles` table also stores:

- `terms_accepted_at` - timestamp for Full Access terms acceptance.
- `approved_commands` - JSON array of approved shell command prefixes.

---

## 4. Layer 3: Per-Entity Overrides

**Source:** `crates/db/migrations/0057_entity_config.sql`
**Resolution:** `crates/server/src/entity_config.rs`

Each agent or channel can override global permissions through `entity_config`:

```sql
CREATE TABLE entity_config (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type TEXT NOT NULL CHECK(entity_type IN ('main', 'agent', 'channel')),
    entity_id   TEXT NOT NULL,
    heartbeat_enabled          INTEGER,
    heartbeat_interval_minutes INTEGER,
    heartbeat_content          TEXT,
    heartbeat_window_start     TEXT,
    heartbeat_window_end       TEXT,
    permissions     TEXT,
    resource_grants TEXT,
    model_preference    TEXT,
    personality_snippet TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(entity_type, entity_id)
);
```

`entity_config::resolve()` starts with global user permissions, overlays
entity-specific permissions, overlays entity resource grants, and returns a
`ResolvedEntityConfig` with override metadata for the UI.

`resolve_for_chat()` is the convenience path used by chat dispatch, agents, and
heartbeat. It loads settings, global permissions, HEARTBEAT.md, and any entity
config row, then resolves the final config.

---

## 5. Layer 4: Origin-Based Policy

**Source:** `crates/tools/src/policy.rs`, `crates/tools/src/origin.rs`

Every request carries an `Origin`:

```rust
pub enum Origin {
    User,
    Comm,
    App,
    Skill,
    System,
}
```

Origin policy is a hard deny list. It is checked by `Registry::execute()` and
cannot be overridden by Full Access or an approval response.

| Origin | Shell access | Notes |
|--------|--------------|-------|
| `User` | Allowed subject to other gates | Direct user interaction. |
| `System` | Allowed subject to other gates | Internal system tasks. |
| `Comm` | Denied | `shell` and `system:shell` are blocked. |
| `App` | Denied | `shell` and `system:shell` are blocked. |
| `Skill` | Denied | `shell` and `system:shell` are blocked. |

The deny check supports bare tool names and compound keys such as
`system:shell`.

---

## 6. Hard Safeguards

**Source:** `crates/tools/src/safeguard.rs`

Safeguards are unconditional. They run before tool execution and cannot be
bypassed by settings, capability toggles, Full Access, or approval responses.

Shell safeguards block:

- `sudo` and `su`
- destructive root/system commands such as `rm -rf /`
- disk formatting/wiping commands such as `dd of=/dev/`, `mkfs`, `fdisk`,
  `gdisk`, `parted`, `wipefs`
- fork bombs
- unsafe writes to `/dev/` except safe pseudo-devices

File safeguards block destructive writes/edits to protected system paths and
sensitive user paths such as SSH/GPG/AWS/Kubernetes/Docker credentials.

---

## 7. Tool Execution Pipeline

**Source:** `crates/tools/src/registry.rs` - `Registry::execute()`

Registry execution is the hard enforcement path:

```
Phase 1: Validate + determine resource permit
  1a. safeguard::check_safeguard(tool_name, input)
  1b. policy.is_denied_for_origin(origin, tool_name, resource)
  1c. capability gate via capabilities::gating_capability(...)
      - allow if entity permission is enabled
      - allow if ctx.approved_categories contains the category
      - otherwise reject
  1d. entity resource grant check
  1e. determine physical resource permit

Phase 2: Acquire ResourcePermit for Screen or Browser if required

Phase 3: Re-acquire tool read lock and execute tool.execute_dyn(ctx, input)
```

The runner approval gate runs before Phase 1. When the user approves a non-shell
tool once, the runner adds its category to `ctx.approved_categories` so Phase 1c
accepts that single execution.

---

## 8. Capability Mapping

**Source:** `crates/tools/src/capabilities.rs`

The current capability gate is not the old registry `tool_category()` mapping.
Use `gating_capability(tool_name, input)` for per-call approval/permission
decisions and `whole_tool_capability(tool_name)` only for whole-tool filtering.

Current high-level mapping:

| Tool/input | Capability |
|------------|------------|
| `web` | `web` |
| `loop` | `web` |
| `os` file-management redirect | no runner capability gate |
| `os` file operations | `file` |
| `os` shell operations | `shell` |
| `os` UI/input/window operations | `desktop` |
| `os` capture/media operations | `media` |
| `os` keychain/search/notification/system operations | `system` |
| organizer contacts | `contacts` |
| organizer mail/calendar/reminders | no runner capability gate |
| installed extension tools, `message`, `event` | no runner capability gate |

`register_all_with_permissions()` still filters some whole tools, but
per-execution checks are the authoritative enforcement path.

---

## 9. Resource Permits

**Source:** `crates/tools/src/registry.rs` - `ResourcePermits`

Physical resources are serialized:

```rust
pub enum ResourceKind {
    Screen,
    Browser,
}
```

Tools declare resource needs with `resource_permit(&input)`. The registry
acquires the permit before execution and holds it until the tool completes.

Entity resource grants control per-entity access:

- `"allow"` - entity can use the resource.
- `"deny"` - entity cannot use the resource.
- `"inherit"` - use the inherited default.

---

## 10. Permission Resolution Flow

End-to-end flow for a normal interactive chat:

```
User message -> WebSocket -> chat_dispatch::run_chat()
  -> resolve entity config
  -> resolve settings.full_access
  -> build RunRequest { permissions, resource_grants, full_access, origin, ... }
  -> Runner receives tool calls
  -> Runner approval gate may emit ApprovalRequest
  -> Registry::execute enforces safeguards, origin policy, permissions, grants
```

Channel-triggered runs use the raw event entry point:

```
Inbound channel event -> ChannelDispatchImpl -> chat_dispatch::run_chat_events()
  -> resolve entity config
  -> resolve settings.full_access
  -> build RunRequest with the same full_access flag as run_chat()
  -> stream raw runner events back to ChannelDispatchImpl
```

`ChannelDispatchImpl` collects text and errors for the channel reply. Channel
plugins do not show Nebo app modals. If an `ApprovalRequest` reaches channel
dispatch, dispatch cancels the run and sends a text fallback telling the user to
enable Full Access or continue in the Nebo app.

---

## 11. Real-Time Approval Flow

A tool call asks for approval when all of these are true:

- `capabilities::gating_capability(tool, input)` returns a capability.
- The resolved entity permission for that capability is off.
- `settings.full_access` is off.
- The origin is interactive.
- For shell calls, no approved command prefix matches.
- An approval channel is available.

Flow:

```
1. Runner creates a oneshot sender keyed by tool_call_id.
2. Runner emits StreamEvent::approval_request(tool_call).
3. chat_dispatch::run_chat broadcasts "approval_request" over WebSocket.
4. ApprovalGate shows ApprovalModal in the app shell.
5. WebSocket handler receives approval_response.
6. Handler sends "deny", "once", or "always" through the oneshot.
7. Runner continues, skips, or persists the always decision.
```

`always` behavior:

- shell: persist a command prefix in `user_profiles.approved_commands`
- non-shell: persist a capability grant via the user permissions API path

### Approval Channels

```rust
pub type ApprovalChannels =
    Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>;
```

The string payload is `"deny"`, `"once"`, or `"always"`.

---

## 12. Full Access

When `settings.full_access` is enabled:

- runner approval prompts are bypassed
- hard safeguards still run
- origin policy still runs
- entity/resource registry enforcement still runs for constraints not satisfied
  by the runner approval gate
- the permissions UI visually disables individual toggles under the Full Access
  master state

Turning on Full Access requires the app modal, risk checkbox, and typing
`ENABLE`. Terms acceptance is timestamped in `user_profiles.terms_accepted_at`.

---

## 13. REST API Endpoints

### Global Permissions

```
GET  /api/v1/user/me/permissions
  -> { permissions, capabilities, approvedCommands }

PUT  /api/v1/user/me/permissions
  <- { permissions: { "web": true, "desktop": false, ... } }

PUT  /api/v1/user/me/approved-commands
  <- { commands: ["npm install", "cargo check"] }
```

### Terms Acceptance

```
POST /api/v1/user/me/accept-terms
  -> sets user_profiles.terms_accepted_at
```

### Agent Settings

```
GET  /api/v1/agent/settings
  -> { settings: { fullAccess, autoInstallDeps, autoApproveRead,
       autoApproveWrite, autoApproveBash, heartbeatIntervalMinutes,
       commEnabled, commPlugin, developerMode, autoUpdate } }

PUT  /api/v1/agent/settings
  <- { fullAccess?, autoInstallDeps?, autoApproveRead?,
       autoApproveWrite?, autoApproveBash?, heartbeatIntervalMinutes?,
       commEnabled?, commPlugin?, developerMode?, autoUpdate? }
```

### Entity Config

```
GET    /api/v1/entity-config/{entity_type}/{entity_id}
PUT    /api/v1/entity-config/{entity_type}/{entity_id}
DELETE /api/v1/entity-config/{entity_type}/{entity_id}
```

---

## 14. WebSocket Messages

### Server -> Client

```json
{
  "type": "approval_request",
  "data": {
    "session_id": "session-uuid",
    "request_id": "tool-call-uuid",
    "tool": "os",
    "input": { "action": "read", "path": "/tmp/file.pdf" }
  }
}
```

### Client -> Server

```json
{
  "type": "approval_response",
  "data": {
    "request_id": "tool-call-uuid",
    "approved": true,
    "always": false
  }
}
```

The handler translates the booleans into `"deny"`, `"once"`, or `"always"` for
the runner.

---

## 15. Frontend Components

| Component | File | Role |
|-----------|------|------|
| Permissions page | `app/src/routes/settings/permissions/+page.svelte` | Full Access toggle, capability toggles, approved command prefixes, terms modal. |
| Approval gate | `app/src/lib/components/ApprovalGate.svelte` | App-wide consumer of `approval_request` WebSocket events. |
| Approval modal | `app/src/lib/components/ApprovalModal.svelte` | Deny, Once, Always decision UI. |
| WebSocket listener | `app/src/lib/websocket/listeners.ts` | Leaves approval events to `ApprovalGate`; handles other socket events. |
| Entity config panel | `app/src/lib/components/EntityConfigPanel.svelte` | Per-entity permission/resource override UI. |

The approval modal is app-wide, not chat-component-local. This matters for
background or channel-related activity that still reaches the Nebo app shell.

---

## 16. Database Schema

### settings

```sql
CREATE TABLE settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    auto_install_deps INTEGER NOT NULL DEFAULT 0,
    auto_approve_read INTEGER NOT NULL DEFAULT 1,
    auto_approve_write INTEGER NOT NULL DEFAULT 0,
    auto_approve_bash INTEGER NOT NULL DEFAULT 0,
    heartbeat_interval_minutes INTEGER NOT NULL DEFAULT 30,
    comm_enabled INTEGER NOT NULL DEFAULT 0,
    comm_plugin TEXT NOT NULL DEFAULT '',
    developer_mode INTEGER NOT NULL DEFAULT 0,
    auto_update INTEGER NOT NULL DEFAULT 1,
    full_access INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);
```

### user_profiles additions

```sql
ALTER TABLE user_profiles ADD COLUMN tool_permissions TEXT DEFAULT '{}';
ALTER TABLE user_profiles ADD COLUMN terms_accepted_at INTEGER;
ALTER TABLE user_profiles ADD COLUMN approved_commands TEXT NOT NULL DEFAULT '[]';
```

### entity_config

See [Layer 3](#4-layer-3-per-entity-overrides).

---

## 17. Special Callers

| Caller | Behavior | Source |
|--------|----------|--------|
| Workflows | Orchestrated workflow execution does not use the app approval modal path. | `workflow_manager.rs`, workflow engine |
| MCP server | Auto-resolves runner approval requests for MCP tool calls. | `crates/server/src/handlers/mcp_server.rs` |
| Channel dispatch | Cannot show approval UI; cancels and returns a text fallback if an approval request reaches it. | `crates/server/src/channel_dispatch.rs` |
| Full Access | Bypasses runner approval prompts, not safeguards or origin policy. | `settings.full_access`, `RunRequest.full_access` |

---

## 18. File Reference

| Component | File |
|-----------|------|
| Settings model | `crates/db/src/models.rs` |
| Settings queries | `crates/db/src/queries/settings.rs` |
| Capability mapping | `crates/tools/src/capabilities.rs` |
| Policy and origin | `crates/tools/src/policy.rs`, `crates/tools/src/origin.rs` |
| Hard safeguards | `crates/tools/src/safeguard.rs` |
| Tool registry | `crates/tools/src/registry.rs` |
| Entity config resolution | `crates/server/src/entity_config.rs` |
| Chat dispatch | `crates/server/src/chat_dispatch.rs` |
| Channel dispatch | `crates/server/src/channel_dispatch.rs` |
| Approval channels | `crates/server/src/state.rs` |
| WebSocket approval handler | `crates/server/src/handlers/ws.rs` |
| User permissions API | `crates/server/src/handlers/user.rs` |
| Agent settings API | `crates/server/src/handlers/agent.rs` |
| Runner `RunRequest` and approval gate | `crates/agent/src/runner.rs` |
| Permissions page | `app/src/routes/settings/permissions/+page.svelte` |
| Approval gate | `app/src/lib/components/ApprovalGate.svelte` |
| Approval modal | `app/src/lib/components/ApprovalModal.svelte` |
| Capability migration | `crates/db/migrations/0028_capability_permissions.sql` |
| Full Access migration | `crates/db/migrations/0109_full_access.sql` |
| Approved commands migration | `crates/db/migrations/0110_approved_commands.sql` |
| Entity config migration | `crates/db/migrations/0057_entity_config.sql` |
