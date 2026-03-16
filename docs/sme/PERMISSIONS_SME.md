# Permission System — Rust SME Reference

> Definitive reference for the Nebo Rust permission system. Covers the four-layer
> architecture (global settings, capability permissions, per-entity overrides,
> origin-based policy), the tool execution pipeline, safeguards, the real-time
> approval flow, and all related database schema, API endpoints, and frontend
> components.

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Layer 1: Global Settings](#2-layer-1-global-settings)
3. [Layer 2: Capability Permissions](#3-layer-2-capability-permissions)
4. [Layer 3: Per-Entity Overrides](#4-layer-3-per-entity-overrides)
5. [Layer 4: Origin-Based Policy](#5-layer-4-origin-based-policy)
6. [Hard Safeguards](#6-hard-safeguards)
7. [Tool Execution Pipeline](#7-tool-execution-pipeline)
8. [Tool Category Mapping](#8-tool-category-mapping)
9. [Resource Permits](#9-resource-permits)
10. [Permission Resolution Flow](#10-permission-resolution-flow)
11. [Real-Time Approval Flow](#11-real-time-approval-flow)
12. [Autonomous Mode](#12-autonomous-mode)
13. [REST API Endpoints](#13-rest-api-endpoints)
14. [WebSocket Messages](#14-websocket-messages)
15. [Frontend Components](#15-frontend-components)
16. [Database Schema](#16-database-schema)
17. [Special Callers (Auto-Approve)](#17-special-callers-auto-approve)
18. [File Reference](#18-file-reference)

---

## 1. Architecture Overview

The permission system is a **four-layered architecture** with real-time approval
via WebSocket:

```
Layer 1: Global Settings (settings table)
  │  autonomous_mode, auto_approve_read/write/bash
  │
Layer 2: Capability Permissions (user_profiles.tool_permissions JSON)
  │  8 categories: chat, file, shell, web, contacts, desktop, media, system
  │
Layer 3: Per-Entity Overrides (entity_config table)
  │  Per-role/channel permissions + resource_grants, inherits from layers 1-2
  │
Layer 4: Origin-Based Policy (tools::policy)
     Hard deny lists per Origin (Comm/App/Skill cannot use shell)
```

Enforcement happens in `tools::registry::execute()` — a multi-phase pipeline that
checks safeguards, origin policy, entity permissions, and resource grants before
executing any tool.

---

## 2. Layer 1: Global Settings

**Source:** `crates/db/migrations/0030_developer_mode.sql`
**Model:** `crates/db/src/models.rs` — `Setting`

The `settings` table is a singleton row (id=1) with global toggles:

| Column | Type | Default | Purpose |
|--------|------|---------|---------|
| `autonomous_mode` | INTEGER (bool) | 0 | Master switch — bypasses ALL approval prompts |
| `auto_approve_read` | INTEGER (bool) | 1 | Auto-approve file read operations |
| `auto_approve_write` | INTEGER (bool) | 0 | Auto-approve file write operations |
| `auto_approve_bash` | INTEGER (bool) | 0 | Auto-approve shell command execution |
| `heartbeat_interval_minutes` | INTEGER | 30 | Default heartbeat interval |
| `comm_enabled` | INTEGER (bool) | 0 | NeboLoop communication toggle |
| `comm_plugin` | TEXT | '' | Active comm plugin name |
| `developer_mode` | INTEGER (bool) | 0 | Developer mode toggle |
| `auto_update` | INTEGER (bool) | 1 | Auto-update toggle |

```rust
// crates/db/src/models.rs
pub struct Setting {
    pub id: i64,
    pub autonomous_mode: i64,    // 0 or 1
    pub auto_approve_read: i64,
    pub auto_approve_write: i64,
    pub auto_approve_bash: i64,
    pub heartbeat_interval_minutes: i64,
    pub comm_enabled: i64,
    pub comm_plugin: String,
    pub developer_mode: i64,
    pub auto_update: i64,
    pub updated_at: i64,
}
```

---

## 3. Layer 2: Capability Permissions

**Source:** `crates/db/migrations/0028_capability_permissions.sql`
**Storage:** `user_profiles.tool_permissions` — JSON string

Global permissions control which **tool categories** the agent can access.
Stored as a JSON map on the user profile:

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

The 8 capability categories (as defined in the frontend):

| Category | Description | Required? |
|----------|-------------|-----------|
| **Chat & Memory** | Core conversations, memory, scheduled tasks | Always on |
| **File System** | Read, write, edit, search, browse files | Toggleable |
| **Shell & Terminal** | Commands, background processes, scripts | Toggleable |
| **Web Browsing** | Fetch pages, internet searches, browse | Toggleable |
| **Contacts & Calendar** | Calendar, contacts, reminders, mail | Toggleable |
| **Desktop Control** | Window mgmt, accessibility, clipboard | Toggleable |
| **Media & Capture** | Screenshots, image analysis, TTS | Toggleable |
| **System** | Spotlight, keychain, Siri shortcuts, notifications | Toggleable |

The `user_profiles` table also stores:
- `terms_accepted_at` (INTEGER) — UNIX timestamp when user accepted autonomous mode terms

---

## 4. Layer 3: Per-Entity Overrides

**Source:** `crates/db/migrations/0057_entity_config.sql`
**Resolution:** `crates/server/src/entity_config.rs`

Each role or channel can override global permissions via the `entity_config` table:

```sql
CREATE TABLE entity_config (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type TEXT NOT NULL CHECK(entity_type IN ('main', 'role', 'channel')),
    entity_id   TEXT NOT NULL,
    -- Heartbeat
    heartbeat_enabled          INTEGER,  -- NULL=inherit, 0/1
    heartbeat_interval_minutes INTEGER,  -- NULL=inherit from settings
    heartbeat_content          TEXT,     -- NULL=inherit from HEARTBEAT.md
    heartbeat_window_start     TEXT,     -- HH:MM, NULL=no window
    heartbeat_window_end       TEXT,     -- HH:MM, NULL=no window
    -- Permissions (JSON)
    permissions     TEXT,     -- {"web": true, "desktop": false, ...}
    -- Resource grants (JSON)
    resource_grants TEXT,     -- {"screen": "allow"|"deny"|"inherit", ...}
    -- Model + personality
    model_preference    TEXT,
    personality_snippet TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(entity_type, entity_id)
);
```

### Resolution Logic

`entity_config::resolve()` layers entity-specific overrides onto global defaults:

```
1. Start with global permissions (user_profiles.tool_permissions)
2. Overlay entity-specific permissions (entity_config.permissions JSON)
3. Resource grants default to "inherit", overlay entity-specific grants
4. Heartbeat: use entity-specific if exists, else global
5. Model preference: entity-specific if set
6. Personality snippet: entity-specific if set
```

The `ResolvedEntityConfig` struct tracks which fields were overridden (not inherited)
via its `overrides: HashMap<String, bool>` field — used by the frontend to show
inherited vs. custom state.

```rust
// crates/server/src/entity_config.rs
pub struct ResolvedEntityConfig {
    pub entity_type: String,
    pub entity_id: String,
    pub heartbeat_enabled: bool,
    pub heartbeat_interval_minutes: i64,
    pub heartbeat_content: String,
    pub heartbeat_window: Option<(String, String)>,
    pub permissions: HashMap<String, bool>,
    pub resource_grants: HashMap<String, String>,
    pub model_preference: Option<String>,
    pub personality_snippet: Option<String>,
    pub overrides: HashMap<String, bool>,
}
```

### Convenience Resolver

`resolve_for_chat()` is the one-call path used by chat dispatch, roles, and heartbeat:

```rust
pub fn resolve_for_chat(
    store: &db::Store,
    entity_type: &str,
    entity_id: &str,
) -> Option<ResolvedEntityConfig>
```

It loads settings, global permissions, HEARTBEAT.md, and entity config row, then
calls `resolve()`. Returns `None` on failure (best-effort — chat proceeds without
overrides).

---

## 5. Layer 4: Origin-Based Policy

**Source:** `crates/tools/src/policy.rs`, `crates/tools/src/origin.rs`

### Origins

Every request carries an `Origin` that identifies its source:

```rust
pub enum Origin {
    User,    // Direct user interaction (web UI, CLI)
    Comm,    // Inter-agent communication (NeboLoop, loopback)
    App,     // External app binary
    Skill,   // Matched skill template
    System,  // Internal system tasks (heartbeat, cron, recovery)
}
```

### Origin Deny List

Hard restrictions by origin — cannot be overridden by any setting:

| Origin | Shell Access | Notes |
|--------|-------------|-------|
| **User** | Full | No restrictions |
| **System** | Full | No restrictions |
| **Comm** | Denied | `shell`, `system:shell` blocked |
| **App** | Denied | `shell`, `system:shell` blocked |
| **Skill** | Denied | `shell`, `system:shell` blocked |

```rust
fn default_origin_deny_list() -> HashMap<Origin, HashSet<String>> {
    let shell_deny: HashSet<String> = ["shell", "system:shell"]
        .iter().map(|s| s.to_string()).collect();

    let mut deny_list = HashMap::new();
    deny_list.insert(Origin::Comm, shell_deny.clone());
    deny_list.insert(Origin::App, shell_deny.clone());
    deny_list.insert(Origin::Skill, shell_deny);
    deny_list
}
```

The deny check supports both bare tool names (`shell`) and compound keys
(`system:shell`) via `policy.is_denied_for_origin(origin, tool_name, resource)`.

### Policy Levels

```rust
pub enum PolicyLevel {
    Deny,      // Block all dangerous operations
    Allowlist, // Only allow whitelisted commands (default)
    Full,      // Allow all (dangerous!)
}
```

### Ask Mode

```rust
pub enum AskMode {
    Off,     // Never ask
    OnMiss,  // Ask only for non-whitelisted (default)
    Always,  // Always ask
}
```

### Safe Commands (never need approval)

```rust
pub const SAFE_BINS: &[&str] = &[
    "ls", "pwd", "cat", "head", "tail", "grep", "find", "which", "type",
    "jq", "cut", "sort", "uniq", "wc", "echo", "date", "env", "printenv",
    "git status", "git log", "git diff", "git branch", "git show",
    "go version", "node --version", "python --version",
];
```

Allowlist matching is flexible: exact match, first-word match, or two-word match
(e.g., `git status` matches `git status --short`).

### Dangerous Command Detection

`policy::is_dangerous()` flags known-dangerous patterns:

- `rm -rf`, `rm -r`, `rmdir`
- `sudo`, `su`
- `chmod 777`, `chown`
- `dd`, `mkfs`
- `> /dev/`, `eval`, `exec`
- Fork bombs: `:(){ :|:& };:`
- Piped shell execution: `curl ... | sh`, `wget ... | bash`

---

## 6. Hard Safeguards

**Source:** `crates/tools/src/safeguard.rs`

Safeguards are **unconditional** — they cannot be bypassed by any setting, permission,
or approval. They run as the first check in the execution pipeline.

### Shell Safeguards (`check_shell_safeguard`)

- **sudo** — Blocked. Nebo must never run commands with elevated privileges.
  Detection includes: prefix (`sudo rm`), pipes (`ls | sudo rm`), subshells
  (`$(sudo cat)`), chained (`&& sudo`, `; sudo`).
- **su** — Blocked. Cannot run commands as another user.
- **Destructive commands** targeting root/system:
  - `rm -rf /` and variations (with `--no-preserve-root`, `/*`)
  - `dd of=/dev/` (block device writes)
  - `mkfs`, `fdisk`, `gdisk`, `parted`, `wipefs` (disk formatting)
  - Fork bombs
  - Writes to `/dev/` (except `/dev/null`, `/dev/stdout`, `/dev/stderr`)

### File Safeguards (`check_file_safeguard`)

Only guards destructive actions (`write`, `edit`). Resolves symlinks before checking.

**Protected paths (macOS):**
- `/`, `/System`, `/usr/bin`, `/usr/sbin`, `/usr/lib`, `/bin`, `/sbin`, `/etc`

**Protected paths (Linux):**
- `/`, `/bin`, `/sbin`, `/usr/bin`, `/usr/sbin`, `/usr/lib`, `/boot`, `/etc`,
  `/proc`, `/sys`, `/dev`

**Protected paths (Windows):**
- `C:\Windows`, `C:\Program Files`, `C:\Program Files (x86)`

**Protected user paths (all platforms):**
- Nebo data directory (prevents self-harm — deleting own database)
- `~/.ssh`, `~/.gnupg`, `~/.aws/credentials`, `~/.kube/config`, `~/.docker/config.json`

---

## 7. Tool Execution Pipeline

**Source:** `crates/tools/src/registry.rs` — `Registry::execute()`

Three-phase execution with two lock acquisitions to avoid holding the tools
read-lock while waiting for resource permits:

```
Phase 1: Validate + Determine Resource Permit
  ├─ 1a. Hard Safeguard Check
  │      safeguard::check_safeguard(tool_name, input)
  │      Unconditional. Cannot be overridden.
  │
  ├─ 1b. Origin-Based Deny Check
  │      policy.is_denied_for_origin(origin, tool_name, resource)
  │      Hard deny per Origin → error, no approval prompt.
  │
  ├─ 1c. Entity Permission Check (after dropping tools read-lock)
  │      tool_category(name) → ctx.entity_permissions map
  │      If category is false → error.
  │
  ├─ 1d. Entity Resource Grant Check
  │      permit_kind (Screen/Browser) → ctx.resource_grants map
  │      If grant is "deny" → error.
  │
  └─ Determine physical resource permit (Screen or Browser)

Phase 2: Acquire Resource Permit (may block)
  └─ ResourcePermits::acquire(ResourceKind::Screen|Browser)
     Serializes access — max 1 concurrent user per resource.

Phase 3: Re-acquire tools read-lock and execute
  └─ tool.execute_dyn(ctx, input)
     Permit guard stays alive for duration of execution.
```

---

## 8. Tool Category Mapping

**Source:** `crates/tools/src/registry.rs` — `tool_category()`

Maps tool names to capability permission categories:

```rust
fn tool_category(name: &str) -> &str {
    match name {
        "web"   => "web",        // HTTP fetch, search, browser
        "os"    => "desktop",    // GUI automation, keyboard, mouse, apps
        "agent" => "memory",     // Tasks, memory, sessions, advisors
        "skill" => "filesystem", // Skill management
        "work"  => "filesystem", // Workflow management
        "loop"  => "web",        // NeboLoop communications
        _       => "other",      // Catch-all
    }
}
```

### Tool Registration Filtering

`register_all_with_permissions()` filters which tools get registered based on
capability permissions:

| Tool | Category Gate | Always Registered? |
|------|--------------|-------------------|
| `os` (OsTool) | — | Yes (always) |
| `web` (WebTool) | `allowed("web")` | No |
| `agent` (AgentTool) | — | Yes (core) |
| `event` (EventTool) | — | Yes (core) |
| `skill` (SkillTool) | — | Yes (core) |
| `execute` (ExecuteTool) | — | Yes (when loader+tier available) |
| `message` (MessageTool) | — | Yes (core) |
| `work` (WorkTool) | — | Yes (when manager provided) |
| `role` (RoleTool) | — | Yes (always) |
| `loop` (LoopTool) | `allowed("loop")` | No |

Tools registered unconditionally still go through the per-execution entity
permission check in Phase 1c.

---

## 9. Resource Permits

**Source:** `crates/tools/src/registry.rs` — `ResourcePermits`

Physical resources (screen, browser) are serialized — only one tool execution can
hold a resource at a time.

```rust
pub enum ResourceKind {
    Screen,   // GUI automation, screenshots
    Browser,  // Headless browser sessions
}
```

Each tool can declare a resource requirement via `resource_permit(&input)`.
The registry acquires the corresponding mutex before execution and holds it for
the duration.

Entity resource grants control access per entity:
- `"allow"` — entity can use the resource
- `"deny"` — entity cannot use the resource
- `"inherit"` — use global default (default for all resources)

---

## 10. Permission Resolution Flow

End-to-end flow from user request to tool execution:

```
User message → WebSocket → chat_dispatch::run_chat()
  │
  ├─ Load ResolvedEntityConfig (if role/channel)
  │   entity_config::resolve_for_chat(store, entity_type, entity_id)
  │     ├─ Load global settings (settings table)
  │     ├─ Load global permissions (user_profiles.tool_permissions)
  │     ├─ Load entity config row (entity_config table)
  │     └─ Resolve: global + entity overlay → ResolvedEntityConfig
  │
  ├─ Build RunRequest
  │   RunRequest {
  │     permissions: resolved.permissions,
  │     resource_grants: resolved.resource_grants,
  │     model_preference: resolved.model_preference,
  │     personality_snippet: resolved.personality_snippet,
  │     origin: Origin::User | Comm | ...,
  │     ...
  │   }
  │
  ├─ Runner builds ToolContext
  │   ToolContext {
  │     origin: req.origin,
  │     entity_permissions: req.permissions,
  │     resource_grants: req.resource_grants,
  │     ...
  │   }
  │
  └─ Registry::execute(ctx, tool_name, input)
      Phase 1a: safeguard::check_safeguard()
      Phase 1b: policy.is_denied_for_origin()
      Phase 1c: entity_permissions[tool_category(name)]
      Phase 1d: resource_grants[resource_name]
      Phase 2:  ResourcePermits::acquire()
      Phase 3:  tool.execute_dyn()
```

---

## 11. Real-Time Approval Flow

When a tool execution requires user approval (based on `auto_approve_*` settings
and policy), the system uses a WebSocket round-trip:

```
1. Agent Runner
   │  Tool call received from LLM
   │  Emit StreamEvent::approval_request(tool_call)
   │
2. Chat Dispatch (chat_dispatch.rs:246)
   │  Match StreamEventType::ApprovalRequest
   │  Broadcast to all WebSocket clients:
   │    { session_id, request_id, tool, input }
   │
3. Frontend (Chat.svelte)
   │  Queue approval request
   │  Show ApprovalModal (FIFO — one at a time)
   │  User chooses: Deny / Once / Always
   │
4. WebSocket Handler (ws.rs:231)
   │  Receive "approval_response" message
   │  Look up approval channel by request_id
   │  Send boolean result via oneshot::Sender<bool>
   │
5. Agent Runner
   │  Receives approval/denial
   │  Continues or skips tool execution
```

### Approval Channels

```rust
// crates/server/src/state.rs
pub approval_channels: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>
```

Each pending approval creates a `oneshot` channel keyed by `tool_call_id`.
The runner awaits the receiver; the WebSocket handler resolves the sender.

---

## 12. Autonomous Mode

When `autonomous_mode` is enabled in settings:
- ALL tool executions bypass approval prompts
- ALL capability permissions are treated as enabled
- Frontend disables individual permission toggles (shows "Auto" badge)

### Activation Safeguards

Turning on autonomous mode requires:
1. A modal dialog with liability terms
2. User must check "I understand the risks" checkbox
3. User must type **"ENABLE"** (case-sensitive) to confirm
4. Terms include: risk acknowledgment, indemnification clause, full responsibility
5. Acceptance is timestamped: `user_profiles.terms_accepted_at`

### Usage in Code

```rust
// crates/server/src/deps.rs
fn is_autonomous(state: &AppState) -> bool {
    state.store
        .get_settings()
        .ok()
        .flatten()
        .map(|s| s.autonomous_mode == 1)
        .unwrap_or(false)
}
```

---

## 13. REST API Endpoints

### Global Permissions

```
GET  /api/v1/user/me/permissions
  → { permissions: { "web": true, "desktop": false, ... } }

PUT  /api/v1/user/me/permissions
  ← { permissions: { "web": true, "desktop": false, ... } }
```

### Terms Acceptance

```
POST /api/v1/user/me/accept-terms
  → Sets user_profiles.terms_accepted_at to current timestamp
```

### Agent Settings

```
GET  /api/v1/agent/settings
  → { settings: { autonomousMode, autoApproveRead, autoApproveWrite,
      autoApproveBash, heartbeatIntervalMinutes, commEnabled,
      commPlugin, developerMode, autoUpdate } }

PUT  /api/v1/agent/settings
  ← { autonomousMode?, autoApproveRead?, autoApproveWrite?,
      autoApproveBash?, heartbeatIntervalMinutes?, commEnabled?,
      commPlugin?, developerMode? }
```

### Entity Config

```
GET    /api/v1/entity-config/{entity_type}/{entity_id}
  → { config: ResolvedEntityConfig }

PUT    /api/v1/entity-config/{entity_type}/{entity_id}
  ← Partial entity config fields to upsert

DELETE /api/v1/entity-config/{entity_type}/{entity_id}
  → Reset to inherited defaults
```

---

## 14. WebSocket Messages

### Server → Client (broadcast)

```json
{
  "type": "approval_request",
  "data": {
    "session_id": "session-uuid",
    "request_id": "tool-call-uuid",
    "tool": "shell",
    "input": { "command": "npm install" }
  }
}
```

### Client → Server

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

The `always` flag (when true) tells the server to update settings to auto-approve
this tool category going forward.

---

## 15. Frontend Components

### Permissions Settings Page

**File:** `app/src/routes/(app)/settings/permissions/+page.svelte`

Three sections:
1. **Autonomous Mode Toggle** — master kill-switch with terms acceptance modal
2. **Capability Toggles** — 8 categories, disabled when autonomous mode is on
3. **Tool Approval Policy** — auto_approve_read/write/bash toggles

State loaded via `Promise.all([getAgentSettings(), getToolPermissions()])`.
Auto-saves on every change.

### Approval Modal

**File:** `app/src/lib/components/ui/ApprovalModal.svelte`

Props:
```typescript
{
  request: { requestId: string, tool: string, input: Record<string, unknown> } | null,
  onApprove: (requestId: string) => void,
  onApproveAlways: (requestId: string) => void,
  onDeny: (requestId: string) => void,
}
```

Three buttons: **Deny** (red), **Once** (primary), **Always** (green).

Input display logic: shows `input.command` for shell, `input.path` for file ops,
or pretty-printed JSON for everything else.

### Approval Queue (Chat Component)

**File:** `app/src/lib/components/chat/Chat.svelte`

```typescript
let approvalQueue = $state<ApprovalRequest[]>([])
const pendingApproval = $derived(approvalQueue.length > 0 ? approvalQueue[0] : null)
```

FIFO queue — only one modal shown at a time. When user responds, the request is
removed and the next one (if any) automatically shows.

### Entity Config Panel

**File:** `app/src/lib/components/chat/EntityConfigPanel.svelte`

Per-entity permission overrides with inherit/override toggle and reset button.
Shows permission categories (web, desktop, filesystem, shell, memory, calendar, email)
and resource grants (screen, browser).

---

## 16. Database Schema

### settings (migration 0030)

```sql
CREATE TABLE settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    autonomous_mode INTEGER NOT NULL DEFAULT 0,
    auto_approve_read INTEGER NOT NULL DEFAULT 1,
    auto_approve_write INTEGER NOT NULL DEFAULT 0,
    auto_approve_bash INTEGER NOT NULL DEFAULT 0,
    heartbeat_interval_minutes INTEGER NOT NULL DEFAULT 30,
    comm_enabled INTEGER NOT NULL DEFAULT 0,
    comm_plugin TEXT NOT NULL DEFAULT '',
    developer_mode INTEGER NOT NULL DEFAULT 0,
    auto_update INTEGER NOT NULL DEFAULT 1,
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);
```

### user_profiles (migration 0028 additions)

```sql
ALTER TABLE user_profiles ADD COLUMN tool_permissions TEXT DEFAULT '{}';
ALTER TABLE user_profiles ADD COLUMN terms_accepted_at INTEGER;
```

### entity_config (migration 0057)

```sql
CREATE TABLE entity_config (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type TEXT NOT NULL CHECK(entity_type IN ('main', 'role', 'channel')),
    entity_id   TEXT NOT NULL,
    heartbeat_enabled          INTEGER,
    heartbeat_interval_minutes INTEGER,
    heartbeat_content          TEXT,
    heartbeat_window_start     TEXT,
    heartbeat_window_end       TEXT,
    permissions     TEXT,       -- JSON
    resource_grants TEXT,       -- JSON
    model_preference    TEXT,
    personality_snippet TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(entity_type, entity_id)
);
```

### sessions (migration 0024)

```sql
-- Session-level policy overrides
sessions.send_policy TEXT      -- 'allow' or 'deny'
sessions.model_override TEXT
sessions.provider_override TEXT
sessions.auth_profile_override TEXT
```

---

## 17. Special Callers (Auto-Approve)

Some execution contexts bypass the approval flow entirely:

| Caller | Behavior | Source |
|--------|----------|--------|
| **Workflows** | Auto-execute, no approval | `workflow_manager.rs` — `requires_approval() → false` |
| **MCP Server** | Auto-approve all tool calls | `handlers/mcp_server.rs:300` — `tx.send(true)` |
| **Autonomous Mode** | All approvals bypassed | `settings.autonomous_mode == 1` |
| **Dependencies** | Auto-install when autonomous | `deps.rs` — `is_autonomous()` check |

---

## 18. File Reference

| Component | File |
|-----------|------|
| **Settings model** | `crates/db/src/models.rs` — `Setting` |
| **Policy & ask mode** | `crates/tools/src/policy.rs` |
| **Hard safeguards** | `crates/tools/src/safeguard.rs` |
| **Tool registry & execution** | `crates/tools/src/registry.rs` |
| **ToolContext & Origin** | `crates/tools/src/origin.rs` |
| **Entity config resolution** | `crates/server/src/entity_config.rs` |
| **Chat dispatch (permissions → runner)** | `crates/server/src/chat_dispatch.rs` |
| **Approval channels (state)** | `crates/server/src/state.rs` |
| **WS approval handler** | `crates/server/src/handlers/ws.rs` |
| **MCP auto-approve** | `crates/server/src/handlers/mcp_server.rs` |
| **Permissions REST API** | `crates/server/src/handlers/user.rs` |
| **Entity config REST API** | `crates/server/src/handlers/entity_config.rs` |
| **Agent settings REST API** | `crates/server/src/handlers/agent.rs` |
| **RunRequest (permissions field)** | `crates/agent/src/runner.rs` — `RunRequest` |
| **Dependency auto-install** | `crates/server/src/deps.rs` |
| **Permissions settings page** | `app/src/routes/(app)/settings/permissions/+page.svelte` |
| **ApprovalModal** | `app/src/lib/components/ui/ApprovalModal.svelte` |
| **Approval queue (Chat)** | `app/src/lib/components/chat/Chat.svelte` |
| **EntityConfigPanel** | `app/src/lib/components/chat/EntityConfigPanel.svelte` |
| **API types** | `app/src/lib/api/neboComponents.ts` |
| **API calls** | `app/src/lib/api/nebo.ts` |
| **Migration: capability permissions** | `crates/db/migrations/0028_capability_permissions.sql` |
| **Migration: settings table** | `crates/db/migrations/0030_developer_mode.sql` |
| **Migration: entity config** | `crates/db/migrations/0057_entity_config.sql` |
| **Migration: session policies** | `crates/db/migrations/0024_session_policies.sql` |
