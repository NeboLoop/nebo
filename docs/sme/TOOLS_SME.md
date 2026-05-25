# Tools System — SME Reference

Comprehensive Subject Matter Expert document covering the Nebo tool system:
registry, domain tools (STRAP pattern), deferred loading, policy/safeguards,
skills, events, process management, and integration points.

**Status:** Current (Rust implementation) | **Last updated:** 2026-05-25

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Registry](#2-registry)
3. [Domain Tools (STRAP)](#3-domain-tools-strap)
4. [Tool Search & Deferred Loading](#4-tool-search--deferred-loading)
5. [Policy & Safeguards](#5-policy--safeguards)
6. [Tool Context & Origins](#6-tool-context--origins)
7. [Two-Phase Execution](#7-two-phase-execution)
8. [Resource Permits](#8-resource-permits)
9. [Skills System](#9-skills-system)
10. [Events & EventBus](#10-events--eventbus)
11. [Process Management](#11-process-management)
12. [MCP Integration](#12-mcp-integration)
13. [Runner Integration](#13-runner-integration)
14. [Workflow Integration](#14-workflow-integration)
15. [Prompt Assembly](#15-prompt-assembly)
16. [Entity Permissions](#16-entity-permissions)
17. [Tool Corrections](#17-tool-corrections)
18. [Sidecar Tool System](#18-sidecar-tool-system)
19. [Agent-Scoped Tool Tracking](#19-agent-scoped-tool-tracking)
20. [Tool Concurrency Safety](#20-tool-concurrency-safety)
21. [File Manifest](#21-file-manifest)

---

## 1. Architecture Overview

```
Server Startup
├── Registry::new(policy)
├── register_all_with_permissions()     ← 14 parameters, domain tools
│   ├── Always: web, agent, event, skill, message, persona, loop (stub), tool_search
│   ├── Deferred: os (keyword), execute, work, publisher, plugin
│   └── Conditional: loop (real tool replaces stub when NeboLoop connects)
├── register(ToolSearchTool)            ← meta-tool for deferred discovery
├── register(McpTool)                   ← STRAP tool for MCP servers
├── register_for_agent(sidecar tools)   ← per-agent sidecar endpoint tools
└── MCP Bridge → register_proxy()       ← deferred proxy tools per server

Per Chat Turn (run_loop)
├── extract_discovered_deferred_tools() ← message-window scanning (Claude Code pattern)
├── list_active(&activated)             ← tool defs sent to LLM
├── filter_tools_with_context()         ← contextual filtering + agent sidecar bypass
├── list_deferred_stubs(&activated)     ← compact stubs in system prompt
├── build system prompt (STRAP docs + deferred listing)
├── LLM responds with tool_calls
├── For each tool_call:
│   ├── Safeguard → Policy → Entity perms → Resource permit
│   └── execute_dyn()
└── Results injected back into conversation

Workflow Activities
├── Pre-resolved tools passed to engine
├── emit + exit tools injected per activity
└── Same two-phase execution model
```

**Crate:** `crates/tools/src/`
**Key exports:** `Registry`, `DynTool`, `Tool`, `ToolResult`, `Policy`, `Origin`, `ToolContext`, `EventBus`, `Skill`, `Loader`

---

## 2. Registry

**File:** `crates/tools/src/registry.rs`

```rust
pub struct Registry {
    tools: Arc<RwLock<HashMap<String, Box<dyn DynTool>>>>,
    deferred: Arc<RwLock<HashSet<String>>>,
    agent_tools: Arc<RwLock<HashMap<String, HashSet<String>>>>,  // agent_id → tool names
    policy: Arc<RwLock<Policy>>,
    process_registry: Arc<ProcessRegistry>,
    bridge: std::sync::RwLock<Option<Arc<mcp::Bridge>>>,
    plugin_store: std::sync::RwLock<Option<Arc<napp::plugin::PluginStore>>>,
    agent_loader: std::sync::RwLock<Option<Arc<napp::AgentLoader>>>,
    resource_permits: ResourcePermits,
}
```

### Tool Trait

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> String;
    fn schema(&self) -> serde_json::Value;
    fn requires_approval(&self) -> bool;
    fn requires_approval_for(&self, input: &Value) -> bool { self.requires_approval() }
    async fn execute(&self, ctx: &ToolContext, input: Value) -> ToolResult;
}

pub trait DynTool: Send + Sync {
    fn resource_permit(&self, input: &Value) -> Option<ResourceKind> { None }
    fn is_concurrent_safe(&self, input: &Value) -> bool { false }
    fn execute_dyn(&self, ctx: &ToolContext, input: Value) -> Pin<Box<dyn Future<Output = ToolResult>>>;
}
```

### Key Methods

| Method | Purpose |
|--------|---------|
| `register(tool)` | Register in active set |
| `register_deferred(tool)` | Register as deferred (not sent to LLM until activated) |
| `register_for_agent(agent_id, tool)` | Register tool owned by an agent's sidecar |
| `agent_tool_names(agent_id)` | Get tool names for an agent's sidecar |
| `unregister(name)` | Remove tool |
| `unregister_agent_tools(agent_id)` | Remove all sidecar tools for an agent |
| `is_deferred(name)` | Check if a tool is deferred |
| `get_deferred_names()` | Get names of all deferred tools |
| `get_tool_description(name)` | Get full description of a specific tool |
| `is_concurrent_safe(name, input)` | Query whether a tool call is safe to run concurrently |
| `list_active(&activated)` | Non-deferred + activated deferred → `Vec<ToolDefinition>` |
| `list_deferred_stubs(&activated)` | `Vec<(name, short_desc)>` for non-activated deferred |
| `list_with_permissions(perms)` | Filter tools by per-entity permission categories |
| `execute(ctx, name, input)` | Two-phase: validate → acquire permit → execute |
| `register_all_with_permissions(...)` | Register complete domain tool set (14 parameters) |

### ToolResult

```rust
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    pub image_url: Option<String>,
}
```

---

## 3. Domain Tools (STRAP)

All domain tools follow the **resource/action** pattern:

```
tool_name(resource: "resource_type", action: "action_name", ...params)
```

### Tool Inventory

| Tool | Name | File | Status | Resources |
|------|------|------|--------|-----------|
| OS | `os` | `os_tool.rs` | **Deferred** | file, shell, capture, window, clipboard, app, dock, settings, music, keychain, search, mail, calendar, contacts, reminders, notification, alert, tts |
| System | `system` | `system_tool.rs` | Not registered by default | file, shell (lighter alternative to OS tool) |
| Web | `web` | `web_tool.rs` | Always | http, search, browser |
| Agent | `agent` | `bot_tool.rs` | Always | memory, task, session, context, advisors, ask, vision, run |
| Event | `event` | `event_tool.rs` | Always | (flat) create, list, delete, pause, resume, run, history |
| Skill | `skill` | `skill_tool.rs` | Always | (flat) catalog, help, load, install, configure, discover, featured, popular, reviews, browse, read_resource, secrets |
| Message | `message` | `message_tool.rs` | Always | owner, notify, sms |
| Persona | `persona` | `agent_tool.rs` | Always | (agent management, registry) |
| Loop | `loop` | `loop_tool.rs` | Always (stub) | dm, channel, group, topic |
| Plugin | `plugin` | `plugin_tool.rs` | Deferred | per-plugin slug as resource; actions: `exec`, `events`. Full command catalog (per-plugin) auto-emitted into the tool description from `capabilities.tools[]` or skill SKILL.md frontmatter — no separate discovery action (v0.10.0+) |
| Work | `work` | `workflows/work_tool.rs` | Deferred | (flat) list, create, run, pause, resume, delete, history |
| Execute | `execute` | `execute_tool.rs` | Deferred | (flat) run, list |
| Publisher | `publisher` | `publisher_tool.rs` | Deferred | (agent/skill publishing) |
| Tool Search | `tool_search` | `tool_search.rs` | Always | (meta-tool) |
| MCP | `mcp` | `mcp_tool.rs` | Always | per-server resource routing |
| Sidecar | (per-endpoint) | `sidecar_tool.rs` | Per-agent | native tools from sidecar `GET /_tools` |
| Emit | `emit` | `emit_tool.rs` | Injected | (workflow only) |
| Exit | `exit` | `exit_tool.rs` | Injected | (workflow only) |

### OS Tool Detail

The `os` tool consolidates 25+ resources across file system, shell, desktop, apps, settings, media, credentials, search, and PIM.

**Now deferred** — discovered through `tool_search` or a direct deferred-tool call in the message window. Contextual keywords can activate OS STRAP sub-docs once the tool is present, but keyword-only deferred activation was removed. Saves ~8-10K tokens on requests that don't involve OS operations.

**Auto-approve (no confirmation):** file, shell, clipboard, capture, search, notification, tts, dock
**Requires approval:** window, app, settings, music, keychain, mail, calendar, contacts, reminders

Sub-tools: `FileTool`, `ShellTool`, `DesktopTool`, `AppTool`, `SettingsTool`, `MusicTool`, `KeychainTool`, `SpotlightTool`, `organizer/*` (platform-specific mail/calendar/contacts/reminders)

### Web Tool Detail

- `http` — fetch, get, post, put, delete, head, sanitize (reqwest, 30s timeout, 5 redirect limit)
- `search` — web search (Brave Search API)
- `browser` — CDP automation: navigate, snapshot, read_page, click, fill, type, screenshot, evaluate, scroll, tabs, etc.
- Large output spilling: responses >8000 chars spilled to file

### Agent Tool Detail

- `memory` — store, recall, search, list (FTS5 + vector hybrid search)
- `task` — spawn, spawn_parallel, orchestrate, status, cancel, create, update, delete
- `session` — list, get, switch, rotate_chat, clear
- `context` — list, get (user preferences)
- `advisors` — deliberate (external advisor execution)
- `ask` — user (UI prompt, blocks until response)
- `vision` — describe, extract
- `run` — list, status, cancel, logs (uses `RunQuerierHandle` to query active runs across the global run registry)

### Plugin Tool Detail

Each installed plugin slug becomes a resource. Actions:
- `exec` — run plugin CLI command (auto-auth retry on OAuth failure, 120s timeout)
- `help` — show plugin skill docs
- `services` — list available services/topics

---

## 4. Tool Search & Deferred Loading

**File:** `crates/tools/src/tool_search.rs`

### Deferred Tools

Tools registered as deferred are not sent to the LLM in the initial tool list. Instead:
1. Compact stubs (name + 1-line description) included in system prompt
2. LLM calls `tool_search(query: "...")` to discover and activate
3. Runner intercepts results, adds to `activated_deferred` set
4. Full tool definition injected on next turn

**Default deferred:** os, execute, work, publisher, plugin, MCP proxy tools

### Search Modes

| Mode | Syntax | Example |
|------|--------|---------|
| Direct select | `select:name1,name2` | `select:work,execute` |
| Required prefix | `+keyword terms` | `+slack send message` |
| Free search | `keyword1 keyword2` | `workflow automation` |

**Scoring:** Name match = +10, description match = +2

### Auto-Activation (tool_filter.rs)

**File:** `crates/agent/src/tool_filter.rs`

```rust
pub fn extract_discovered_deferred_tools(
    messages: &[ChatMessage],
    deferred_names: &HashSet<String>,
) -> HashSet<String>
```

Follows Claude Code's `extractDiscoveredToolNames(messages)` pattern:
1. Scans assistant `tool_calls` — any deferred tool that was directly called → discovered
2. Scans tool result messages for `tool_search` responses — extracts `matches` array entries

**Key property:** when the sliding message window evicts messages, any `tool_search` results or tool calls in those messages disappear, so the tool naturally unloads. Tools come and go with the message window.

Keyword-based deferred activation was removed. The model must explicitly call `tool_search` to discover deferred tools.

### Contextual Tool Filtering (tool_filter.rs)

```rust
pub fn filter_tools_with_context(
    all_tools, messages, called_tools, agent_tool_names
) -> (Vec<ToolDefinition>, Vec<String>)
```

All tools remain registered but the schema list sent to the LLM is filtered by context:
- **Always included:** agent, skill, event, message, tool_search
- **Context-activated:** web, loop, work, execute, emit (via keyword matching in recent messages)
- **OS sub-contexts:** desktop, app, music, settings, keychain, spotlight, organizer (activate `os` tool + inject STRAP sub-docs)
- **Agent sidecar tools:** always included for their agent (bypass filter via `agent_tool_names` parameter)

**Keyword groups** map conversation context → STRAP sub-doc injection:
- "workflow", "automate" → `work`
- "run script", "python", "node" → `execute`
- "neboloop", "channel", "dm" → `loop`
- "click", "mouse", "screenshot" → `desktop` (os sub-context)
- etc.

### LoopStubTool

**File:** `crates/tools/src/registry.rs`

Fallback tool registered when NeboLoop is not yet connected. Returns a helpful error (`"NeboLoop is not connected..."`) instead of crashing. Ensures the `loop` tool appears in the tool list (10/10) even before NeboLoop connects. Replaced by the real `LoopTool` when `comm_plugin` is available.

---

## 5. Policy & Safeguards

### Policy (`policy.rs`)

```rust
pub enum PolicyLevel {
    Deny,       // Deny all dangerous ops
    Allowlist,  // Allow whitelisted only (default)
    Full,       // Allow all
}

pub enum AskMode {
    Off,        // Never ask
    OnMiss,     // Ask for non-whitelisted (default)
    Always,     // Always ask
}

pub struct Policy {
    pub level: PolicyLevel,
    pub ask_mode: AskMode,
    pub allowlist: HashSet<String>,
    pub origin_deny_list: HashMap<Origin, HashSet<String>>,
}
```

**SAFE_BINS** (auto-approve): `ls`, `cat`, `head`, `grep`, `find`, `jq`, `git status/log/diff/branch`, etc.

**`is_dangerous(cmd)`** heuristic: `rm -rf`, `sudo`, `eval`, pipe to `sh`, etc.

### Safeguard (`safeguard.rs`)

**Unconditional checks** — cannot be overridden by policy:

```rust
pub fn check_safeguard(tool_name: &str, input: &Value) -> Option<String>
pub fn check_path_scope(tool_name: &str, input: &Value, allowed_paths: &[String]) -> Option<String>
```

- File writes/edits/deletes outside `allowed_paths` → blocked
- Shell commands outside `allowed_paths` → blocked
- Reads always allowed

### Sandbox Policy (`sandbox_policy.rs`)

Maps skill capabilities to OS-level sandbox permissions:

| Capability | Permission |
|------------|-----------|
| `storage` | Write to data dir |
| `network` | DNS + HTTP to package registries + skill-declared domains |
| (none) | Only stdout, stderr, work dir |

**Deny-read list:** `~/.ssh`, `~/.gnupg`, `~/.aws/credentials`, `~/.config/gcloud`

---

## 6. Tool Context & Origins

**File:** `crates/tools/src/origin.rs`

```rust
pub enum Origin {
    User,    // Web UI, CLI
    Comm,    // NeboLoop, loopback
    App,     // External app binary
    Skill,   // Matched skill template
    System,  // Heartbeat, cron, recovery
    Mcp,     // Claude Desktop, Cursor, etc.
}

pub struct ToolContext {
    pub origin: Origin,
    pub session_key: String,
    pub session_id: String,
    pub user_id: String,
    pub entity_permissions: Option<HashMap<String, bool>>,
    pub resource_grants: Option<HashMap<String, String>>,
    pub allowed_paths: Vec<String>,
    pub cancel_token: CancellationToken,
    pub stream_tx: Option<mpsc::Sender<StreamEvent>>,
    pub run_id: Option<String>,
    pub ask_channels: Option<AskChannels>,
}
```

### Origin-Based Restrictions

| Origin | Default restrictions |
|--------|---------------------|
| `User`, `System`, `Mcp` | All tools allowed (subject to entity perms) |
| `Comm`, `App`, `Skill` | Shell denied by default |

---

## 7. Two-Phase Execution

**File:** `crates/tools/src/registry.rs` (`execute` method)

```
Phase 1: Validate (read-lock)
├── Find tool by name
├── check_safeguard() — unconditional safety
├── check_path_scope() — file/shell scope restrictions
├── origin_deny_list check
├── entity_permissions check (by category)
├── entity resource_grants check (screen/browser)
├── Determine resource_permit needed
└── Drop read-lock

Phase 2: Acquire Permit (async)
├── Wait for ResourceKind mutex (Screen or Browser)
└── Guard stays alive for execution duration

Phase 3: Execute (read-lock)
├── Re-acquire tool reference
├── Call execute_dyn(ctx, input)
└── Return ToolResult
```

---

## 8. Resource Permits

Physical resource serialization — prevents concurrent tool calls from fighting over hardware:

```rust
pub enum ResourceKind {
    Screen,   // Mouse, keyboard, screenshots, app control
    Browser,  // CDP session automation
}

pub struct ResourcePermits {
    screen: Mutex<()>,
    browser: Mutex<()>,
}
```

Only one tool can hold a Screen or Browser permit at a time. Other tools queue on the mutex.

---

## 9. Skills System

**Directory:** `crates/tools/src/skills/`

### Skill (`skill.rs`)

```rust
pub struct Skill {
    pub name: String,
    pub description: String,
    pub version: String,
    pub enabled: bool,
    pub source: SkillSource,          // Installed | User
    pub triggers: Vec<String>,
    pub capabilities: Vec<String>,    // ["python", "network", "storage"]
    pub dependencies: Vec<SkillRequirement>,
    pub plugins: Vec<PluginDependency>,
    pub secrets: Vec<SecretDeclaration>,
    pub source_path: Option<PathBuf>,
    // ...
}
```

### Loader (`loader.rs`)

```rust
pub struct Loader {
    user_dir: PathBuf,
    installed_dir: PathBuf,
    skills: Arc<RwLock<HashMap<String, Skill>>>,
    plugin_store: Option<Arc<napp::plugin::PluginStore>>,
    watcher_paused: Arc<AtomicBool>,
    bundled_raw: HashMap<String, &'static str>,
    cached_catalog: Arc<RwLock<String>>,
    license_keys: Arc<RwLock<HashMap<String, [u8; 32]>>>,
}
```

**Loading order (later overrides earlier):**
1. Embedded bundled skills (frontmatter only, template lazy-loaded)
2. Installed skills from `.napp` archives (`nebo/skills/`)
3. User skills from loose files (`user/skills/`)
4. App skills from agent tool directories (`~/.nebo/agents/<agent-id>/skills/`)

### Warm-Start Manifest

The loader uses a two-phase load strategy:
- **Warm start (<50ms):** Reads a skill manifest index (`.skill-manifest.json`) instead of walking the filesystem.
- **Cold start:** Full filesystem scan + parallel YAML parsing via rayon, then writes manifest for next time.

The `cached_catalog` field holds a pre-built compact catalog string, rebuilt on `load_all()` or watcher reload.

### Loader Methods

| Method | Purpose |
|--------|---------|
| `pause_watcher()` | Prevent premature reloads during skill/plugin extraction |
| `resume_watcher()` | Re-enable filesystem watcher after extraction |
| `with_plugin_store(ps)` | Verify plugin dependencies during load |
| `set_license_keys(keys)` | Set license keys for sealed `.napp` decryption (keyed by artifact_id) |
| `load_app_skills(&app_dir)` | Load skills from an app's directory (e.g. `<tool_dir>/skills/`), returns names |

### Template Expansion (`expand.rs`)

```rust
pub fn expand_variables(body: &str, ctx: &SkillContext) -> String
```

**Variables:**
- `${NEBO_SKILL_DIR}`, `${NEBO_DATA_DIR}`, `${NEBO_USER_NAME}`, `${NEBO_OS}`, `${NEBO_ARCH}`
- `${plugin.GWS_BIN}` → resolved plugin binary path
- `${secret.BRAVE_API_KEY}` → decrypted secret value

---

## 10. Events & EventBus

**File:** `crates/tools/src/events.rs`

```rust
pub struct Event {
    pub source: String,         // "gws.email.new", "chief.morning.complete"
    pub payload: serde_json::Value,
    pub origin: String,         // "workflow:email-triage:run-550e"
    pub timestamp: u64,
}

pub struct EventBus {
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
}
```

**EmitTool** (`emit_tool.rs`) — injected into workflow activities, emits events into EventBus.
**ExitTool** (`exit_tool.rs`) — injected into workflow activities, clean early termination via sentinel `"__WORKFLOW_EXIT__:"`.

Events flow: `EmitTool` → `EventBus` → `EventDispatcher` → `WorkflowManager.run_inline()`

---

## 11. Process Management

**File:** `crates/tools/src/process.rs`

```rust
pub struct ProcessRegistry {
    running: Arc<Mutex<HashMap<String, Arc<BackgroundSession>>>>,
    finished: Arc<Mutex<HashMap<String, Arc<BackgroundSession>>>>,
}
```

**Methods:**
- `spawn_background(command, cwd, env)` → session_id
- `get_any_session(id)` → running or finished
- `write_stdin(id, data)` → send input to running process
- `kill_session(id)` → terminate

**Environment sanitization:** Filters `LD_PRELOAD`, `DYLD_INSERT_LIBRARIES`, `PYTHONSTARTUP`, etc. to prevent code injection via environment.

---

## 12. MCP Integration

**File:** `crates/tools/src/mcp_tool.rs`, `crates/tools/src/registry.rs`

### McpTool (STRAP tool)

```
mcp(server: "monument.sh", resource: "project", action: "list")
```

Routes calls through `mcp::Bridge` to connected MCP servers. Handles OAuth token refresh with 60s buffer.

### Proxy Tools

When MCP servers connect, the bridge calls `Registry::register_proxy()`:
1. Creates `McpProxyTool` (implements `DynTool`)
2. Registered as **deferred**
3. Full name: `mcp__server-name__tool-name`
4. On execute: proxies to `bridge.call_tool(integration_id, original_name, input)`

### MCP Prefix Stripping

```rust
fn strip_mcp_prefix(name: &str) -> &str
// "mcp__nebo-agent__web" → "web"
```

External MCP clients (Claude Desktop, Cursor) can call STRAP tools directly via their short names through the MCP server endpoint.

---

## 13. Runner Integration

**File:** `crates/agent/src/runner.rs`

### Tool List Assembly (per turn)

```rust
// 1. Extract discovered deferred tools from message window
// (tools auto-unload when their discovery messages are evicted)
let activated_deferred = tool_filter::extract_discovered_deferred_tools(
    &messages, &deferred_names,
);

// 2. Get tool definitions for LLM
let all_tool_defs = tools.list_active(&activated_deferred).await;

// 3. Get compact stubs for unactivated deferred tools
let deferred_stubs = tools.list_deferred_stubs(&activated_deferred).await;

// 4. Filter by context + agent sidecar tools
let agent_tools = tools.agent_tool_names(&agent_id).await;
let (filtered, contexts) = tool_filter::filter_tools_with_context(
    &all_tool_defs, &messages, &called_tools, &agent_tools,
);
```

### Tool Execution (parallel)

```rust
for tc in tool_calls {
    futures.push(async move {
        let _permit = concurrency.acquire_tool_permit().await;
        tokio::time::timeout(
            TOOL_EXECUTION_TIMEOUT,
            tools.execute(&ctx, &tc.name, tc.input.clone()),
        ).await
    });
}
```

### Hook System

Pre/post-execute hooks can intercept tool calls:
- `tool.pre_execute` — can block based on `(tool_name, input)` payload
- `tool.post_execute` — fires after execution with result

---

## 14. Workflow Integration

**File:** `crates/workflow/src/engine.rs`

### Tool Resolution

Workflows receive pre-resolved tools from the runner. Per activity:

```rust
// All resolved tools available to every activity
let mut activity_tools: Vec<&Box<dyn DynTool>> = resolved_tools.iter().collect();

// Inject emit tool (always, no declaration needed)
activity_tools.push(&emit_tool);

// Inject exit tool (always)
activity_tools.push(&exit_tool);
```

### Input Passing

Event payload flows into activity prompts via the `inputs` map. Internal keys filtered:

```rust
let skip_keys = ["_emit"];  // Only skip operational keys
let user_inputs = map.iter().filter(|(k, _)| !skip_keys.contains(&k.as_str()));
```

Event keys (`_event_source`, `_event_payload`, `_event_origin`) are included in the prompt so activities can access the triggering event data.

---

## 15. Prompt Assembly

**File:** `crates/agent/src/prompt.rs`

### System Prompt Structure

```
1. Static prefix (personality, rules)
2. Cache boundary marker (provider prompt caching)
3. STRAP section (tool documentation per active context)
4. Tools list (comma-separated tool names)
5. Deferred listing ("Call tool_search to activate: loop, work, ...")
6. Dynamic suffix (session state, memories)
```

### STRAP Section

```rust
pub fn build_strap_section(tool_names: &[String], active_contexts: &[String]) -> String
```

Injects full documentation for:
- MCP proxy tools (from tool descriptions)
- OS sub-contexts based on conversation keywords (desktop, music, organizer, etc.)

### Deferred Listing

```
You have additional tools available that aren't loaded by default:
- loop: NeboLoop communication
- work: Workflow automation
- ...
Call tool_search(query: "...", max_results: 5) to find and activate them.
```

---

## 16. Entity Permissions

### Permission Categories

| Category | Tools |
|----------|-------|
| `web` | web |
| `desktop` | os (file, shell, window, app, settings, music, keychain, search) |
| `memory` | agent (memory) |
| `filesystem` | skill, work |
| `other` | (default for unmapped tools) |

### Resource Grants

Per-entity overrides for physical resources:

```rust
// "screen" -> "allow" | "deny" | "inherit"
// "browser" -> "allow" | "deny" | "inherit"
```

Checked during Phase 1 of two-phase execution, before resource permit acquisition.

---

## 17. Tool Corrections

**File:** `crates/tools/src/registry.rs` (`tool_correction` function)

When the LLM calls a non-existent tool, the registry returns a correction hint:

| Hallucinated Call | Correction |
|-------------------|------------|
| `websearch` | `web(action: "search", query: "...")` |
| `read` | `os(resource: "file", action: "read", path: "...")` |
| `bash` | `os(resource: "shell", action: "exec", command: "...")` |
| `system` | "system is now os" |
| `bot` | "bot is now agent" |
| `desktop` | "desktop is now under os" |
| `project`/`todo` | "use mcp(server: \"monument.sh\", ...)" |

20+ corrections total. Prevents common LLM hallucinations from producing errors.

---

## 18. Sidecar Tool System

**File:** `crates/tools/src/sidecar_tool.rs` (161 lines)

Each sidecar HTTP endpoint becomes a native LLM tool — the LLM sees `list_projects(...)` directly, not `brief(action: "list_projects")`.

### Discovery

Sidecars expose their tools via `GET /_tools`, which returns a JSON array of `SidecarToolDef`:

```rust
pub struct SidecarToolDef {
    pub name: String,           // e.g. "list_projects"
    pub description: String,
    pub method: String,         // GET, POST, PUT, DELETE
    pub path: String,           // "/projects", "/documents/{id}"
    pub input_schema: Option<serde_json::Value>,
}
```

### Execution

`SidecarActionTool` implements `DynTool` directly (not wrapped in a domain tool):
1. Resolves path parameters from input (`/documents/{id}` + `{id: "123"}` → `/documents/123`)
2. Builds request by HTTP method: GET → query params, POST/PUT → JSON body (path param keys stripped)
3. Calls sidecar via the `SidecarCaller` trait (abstracts gRPC connection — implemented in `crates/server`)
4. Returns `ToolResult::ok` or `ToolResult::error` based on HTTP status code

### Integration

- Sidecar tools are registered via `Registry::register_for_agent(agent_id, tool)` — not deferred
- Always included for their agent (bypass contextual tool filter via `agent_tool_names`)
- Cleaned up on agent shutdown via `Registry::unregister_agent_tools(agent_id)`

**Full reference:** `docs/sme/SIDECAR_TOOLS_SME.md`

---

## 19. Agent-Scoped Tool Tracking

**File:** `crates/tools/src/registry.rs`

The registry tracks which tools belong to which agent's sidecar:

```rust
agent_tools: Arc<RwLock<HashMap<String, HashSet<String>>>>  // agent_id → tool names
```

### Methods

| Method | Purpose |
|--------|---------|
| `register_for_agent(agent_id, tool)` | Register tool + record ownership |
| `agent_tool_names(agent_id)` | Get `HashSet<String>` of tool names for an agent |
| `unregister_agent_tools(agent_id)` | Remove all tools owned by an agent |

### Use Cases

- **Clean shutdown:** When an app agent stops, its sidecar tools are removed in one call
- **Hot restart:** Unregister old tools, re-discover via `GET /_tools`, register new set
- **Tool filter bypass:** `filter_tools_with_context()` accepts `agent_tool_names` — sidecar tools always pass through the contextual filter
- **SDK whitelisting:** Agent's own tools are always visible regardless of permission categories

---

## 20. Tool Concurrency Safety

**File:** `crates/tools/src/registry.rs`

The `DynTool` trait includes a method for declaring whether a tool call is safe to run concurrently:

```rust
fn is_concurrent_safe(&self, input: &Value) -> bool { false }  // default: assume writes
```

Read-only operations return `true` and can run in parallel. Write operations default to `false` (serial execution after all concurrent tools finish).

### Registry Integration

```rust
pub async fn is_concurrent_safe(&self, tool_name: &str, input: &Value) -> bool
```

Queries the tool by name (with MCP prefix fallback). Returns `false` for unknown tools (conservative default).

### Per-Tool Overrides

**Skill tool** (`skill_tool.rs`) — marks these actions as concurrent-safe:
`catalog`, `discover`, `help`, `browse`, `read_resource`, `featured`, `popular`, `reviews`, `secrets`

Write actions (`load`, `unload`, `create`, `update`, `delete`, `install`, `configure`) remain serial.

Other tools default to `false` (serial) unless they override `is_concurrent_safe`.

---

## 21. File Manifest

### Domain Tools

| File | Tool Name | Purpose |
|------|-----------|---------|
| `os_tool.rs` | `os` | Unified OS operations (25 resources) |
| `web_tool.rs` | `web` | HTTP + search + browser automation |
| `bot_tool.rs` | `agent` | Agent self-management (memory, tasks, sessions, runs via `RunQuerierHandle`) |
| `event_tool.rs` | `event` | Scheduling & reminders |
| `skill_tool.rs` | `skill` | Skill catalog, loading, marketplace |
| `message_tool.rs` | `message` | Outbound notifications & SMS |
| `agent_tool.rs` | `persona` | Agent registry & validation |
| `sidecar_tool.rs` | (per-endpoint) | Sidecar HTTP endpoint → native LLM tool |
| `system_tool.rs` | `system` | Lighter OS alternative (file + shell only) |
| `loop_tool.rs` | `loop` | NeboLoop communication |
| `plugin_tool.rs` | `plugin` | Plugin binary execution |
| `workflows/work_tool.rs` | `work` | Workflow lifecycle |
| `execute_tool.rs` | `execute` | Script execution (sandboxed) |
| `publisher_tool.rs` | `publisher` | Agent/skill publishing |
| `mcp_tool.rs` | `mcp` | MCP server integration |
| `emit_tool.rs` | `emit` | Event emission (workflow-injected) |
| `exit_tool.rs` | `exit` | Workflow early exit (workflow-injected) |
| `tool_search.rs` | `tool_search` | Deferred tool discovery |
| `a2ui_tool.rs` | `a2ui` | A2UI accessibility framework |

### OS Sub-Tools

| File | Used By | Purpose |
|------|---------|---------|
| `file_tool.rs` | `os`, `system` | File system operations |
| `shell_tool.rs` | `os`, `system` | Shell execution |
| `desktop_tool.rs` | `os` | Desktop automation (capture, window, clipboard) |
| `app_tool.rs` | `os` | App launching, dock management |
| `settings_tool.rs` | `os` | System settings (volume, brightness) |
| `music_tool.rs` | `os` | Media player control |
| `keychain_tool.rs` | `os` | macOS Keychain access |
| `spotlight_tool.rs` | `os` | macOS Spotlight search |
| `organizer/` | `os` | Platform-specific PIM (mail, calendar, contacts, reminders) |

### Infrastructure

| File | Purpose |
|------|---------|
| `lib.rs` | Public API, artifact persistence (NeboLoop) |
| `registry.rs` | Tool registration, execution, corrections |
| `domain.rs` | Domain schema generation (`DomainInput`, `build_domain_schema`) |
| `policy.rs` | Approval, allowlists, origin deny lists |
| `safeguard.rs` | Unconditional safety checks |
| `sandbox_policy.rs` | Skill capability → sandbox config mapping |
| `process.rs` | Background process management |
| `events.rs` | Event, EventBus |
| `origin.rs` | Origin, ToolContext, AskChannels |
| `orchestrator.rs` | SubAgentOrchestrator trait (spawn, DAG, cancel) |
| `research.rs` | Research tool utilities |
| `run_querier.rs` | Query global run registry |

### Skills

| File | Purpose |
|------|---------|
| `skills/mod.rs` | Module exports |
| `skills/skill.rs` | Skill, SkillSource, SkillSummary, dependencies |
| `skills/loader.rs` | Load bundled/installed/user skills |
| `skills/expand.rs` | Template variable expansion (`${plugin.GWS_BIN}`, `${secret.*}`) |
| `skills/bundled/mod.rs` | Embedded bundled skills |

---

*Last updated: 2026-05-25*
