# Sidecar Tools — SME Reference

> How sidecar HTTP endpoints become native LLM tools. Covers the `agent.json`
> tool declaration format, `SidecarActionTool` registration, agent-scoped tool
> tracking in the Registry, skill loading for app agents, tool filtering,
> sidecar restore on startup, binary hot-reload detection, and SDK-driven
> tool scoping.
>
> For the sidecar runtime lifecycle (launch, health, restart), see `APPS.md §7-8`.
> For the gRPC proxy protocol, see `APPS.md §9`.

---

## Table of Contents

1. [Design Principle](#1-design-principle)
2. [Discovery Protocol](#2-discovery-protocol)
3. [SidecarActionTool](#3-sidecaractiontool)
4. [Registry: Agent-Scoped Tools](#4-registry-agent-scoped-tools)
5. [Tool Filter Bypass](#5-tool-filter-bypass)
6. [Skill Loading for Apps](#6-skill-loading-for-apps)
7. [End-to-End Flow](#7-end-to-end-flow)
8. [Sidecar Restore on Startup](#8-sidecar-restore-on-startup)
9. [Binary Hot-Reload Detection](#9-binary-hot-reload-detection)
10. [Tool Scoping (SDK-driven)](#10-tool-scoping-sdk-driven)
11. [Key Files](#11-key-files)

---

## 1. Design Principle

Sidecar tools are **native, not proxied**. Each sidecar HTTP endpoint registers
as its own tool in the LLM's tool list. The LLM sees `get_document(id: "abc")`
directly — not `brief(action: "get_document", id: "abc")`.

This matters because:
- **Context efficiency** — tool names are self-describing, no wrapper overhead
- **Schema fidelity** — each tool has its own JSON Schema, not a union schema
- **Skill alignment** — skills reference tools by their native names
- **Filter compatibility** — tools pass through the same filter as all other tools

Previous approach (removed): `SidecarProxyTool` — a single mega-tool that
accepted `action` as a parameter and dispatched internally. This wasted context
and broke the tool filter (the single proxy name didn't match any filter rule).

Earlier iteration also used `GET /_tools` HTTP discovery; this was replaced with
filesystem-based declaration in `agent.json` for consistency with skills/plugins.

---

## 2. Discovery Protocol

Tool definitions are declared in `agent.json` under the `"tools"` array.
Discovery reads from the filesystem, not from an HTTP endpoint — this follows
the same pattern as skills and plugins. No `GET /_tools` call is made.

`AppLifecycle::discover_tools()` calls `read_tool_defs_from_config()` which
parses `agent.json` via `napp::agent::parse_agent_config()` and maps each
`AgentToolDef` into a `SidecarToolDef`.

### agent.json tools array

```json
{
  "tools": [
    {
      "name": "get_document",
      "description": "Retrieve the current document content and metadata",
      "method": "GET",
      "path": "/documents/{id}",
      "input_schema": {
        "type": "object",
        "properties": {
          "id": { "type": "string", "description": "Document ID" }
        },
        "required": ["id"]
      }
    },
    {
      "name": "update_document",
      "description": "Update document content",
      "method": "PUT",
      "path": "/documents/{id}",
      "input_schema": {
        "type": "object",
        "properties": {
          "id": { "type": "string" },
          "content": { "type": "string" },
          "title": { "type": "string" }
        },
        "required": ["id", "content"]
      }
    }
  ]
}
```

### AgentToolDef / SidecarToolDef Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Tool name the LLM sees (e.g. `get_document`) |
| `description` | string | yes | What the tool does (shown to LLM) |
| `method` | string | yes | HTTP method: GET, POST, PUT, DELETE |
| `path` | string | yes | Endpoint path, may contain `{param}` placeholders |
| `input_schema` | object | no | JSON Schema for input; defaults to `{"type": "object", "properties": {}}` |

### Path Parameters

Path placeholders like `/documents/{id}` are resolved from the input object.
The `id` key is consumed from the body for non-GET methods (removed before
serializing the request body). For GET methods, non-path parameters become
query string parameters.

### Discovery Timing

Discovery happens in `AppLifecycle::launch()` after the sidecar process starts
and the Unix socket is ready. If `agent.json` is missing, unparseable, or has
an empty `tools` array, no tools are registered (this is not an error — some
sidecars don't expose tools).

---

## 3. SidecarActionTool

Each `SidecarToolDef` becomes a `SidecarActionTool` — a `DynTool` implementation
that routes LLM tool calls to the sidecar via gRPC.

```
crates/tools/src/sidecar_tool.rs
```

### Struct

```rust
pub struct SidecarActionTool {
    def: SidecarToolDef,        // Immutable after creation
    caller: Arc<dyn SidecarCaller>,  // gRPC connection to sidecar
}
```

### DynTool Implementation

| Method | Behavior |
|--------|----------|
| `name()` | Returns `def.name` (e.g. `"get_document"`) |
| `description()` | Returns `def.description` |
| `schema()` | Returns `def.input_schema` or empty object schema |
| `requires_approval()` | Always `false` (sidecar tools are trusted) |
| `execute_dyn()` | Resolves path params, builds body/query, calls sidecar |

### No Locks

Unlike the previous `SidecarProxyTool` (which used `tokio::sync::RwLock` and
panicked from sync DynTool methods), `SidecarActionTool` stores the tool def
as an immutable field. No locking needed. No async/sync mismatch possible.

### SidecarCaller Trait

```rust
pub trait SidecarCaller: Send + Sync {
    fn call(&self, method: &str, path: &str, query: &str, body: &[u8])
        -> Pin<Box<dyn Future<Output = Result<SidecarResponse, String>> + Send + '_>>;
}
```

Implemented by `GrpcSidecarCaller` in `crates/server/src/app_lifecycle.rs`,
which connects to the sidecar's Unix socket and calls `UIService.HandleRequest`.

### GrpcSidecarCaller Implementation

`GrpcSidecarCaller` stores only the `sock_path: PathBuf`. On each call it creates
a lazy gRPC channel via `tonic::transport::Endpoint::connect_with_connector_lazy`,
using `tower::service_fn` to connect a `tokio::net::UnixStream` to the socket.
This keeps `crates/tools` free of `tonic`/`proto` dependencies — the trait is
defined in `crates/tools`, the implementation lives in `crates/server`.

```rust
struct GrpcSidecarCaller {
    sock_path: PathBuf,
}
// → creates UiServiceClient per call
// → calls client.handle_request(HttpRequest { method, path, query, headers, body })
// → maps HttpResponse { status_code, body } to SidecarResponse
```

---

## 4. Registry: Agent-Scoped Tools

The tool `Registry` tracks which tools belong to which agent's sidecar:

```rust
// crates/tools/src/registry.rs
agent_tools: Arc<RwLock<HashMap<String, HashSet<String>>>>
//                       ^agent_id    ^tool names
```

### API

| Method | Purpose |
|--------|---------|
| `register_for_agent(agent_id, tool)` | Register tool + track ownership |
| `agent_tool_names(agent_id)` | Get all tool names for an agent |
| `unregister_agent_tools(agent_id)` | Remove all tools for an agent |

### Why Not Just `register()`?

Agent-scoped tracking enables:
1. **Clean shutdown** — `unregister_agent_tools()` removes all sidecar tools at once
2. **Hot restart** — on binary change, old tools are removed before new ones register
3. **Tool filter** — the runner asks "which tools belong to this agent?" and passes
   them to the filter so they're always included
4. **Tool scoping** — SDK whitelists a subset of agent tools per context via `agent.json` scopes

---

## 5. Tool Filter Bypass

The tool filter (`crates/agent/src/tool_filter.rs`) decides which tools appear
in each LLM request. Sidecar tools bypass the keyword-based filtering:

```rust
pub fn filter_tools_with_context(
    all_tools: &[ToolDefinition],
    messages: &[ChatMessage],
    called_tools: &[String],
    agent_tool_names: &HashSet<String>,  // ← sidecar tools always pass
) -> (Vec<ToolDefinition>, Vec<String>)
```

A tool is included if ANY of these are true:
- It's in `ALWAYS_INCLUDE_TOOLS` (core: agent, skill, event, message, tool_search)
- It matches an active keyword context (e.g. "browse" → web tool)
- It was already called this session
- It starts with `mcp__` (MCP proxy tools)
- **It's in `agent_tool_names`** (sidecar tools for this agent)

The runner (`runner.rs`) fetches agent tool names before each iteration:

```rust
let agent_tool_names = tools.agent_tool_names(agent_id).await;
let (tool_defs, active_contexts) = tool_filter::filter_tools_with_context(
    &all_tool_defs, &window_messages, &called_tools, &agent_tool_names
);
```

---

## 6. Skill Loading for Apps

Tools without skills are blind. Skills teach the agent *when* and *how* to use
its tools. App skills live in the app's `skills/` directory alongside the sidecar
binary and UI:

```
~/.nebo/nebo/agents/<agent-id>/
├── AGENT.md
├── agent.json          # References skills in "skills" array
├── skills/
│   ├── workspace-management/
│   │   └── SKILL.md    # "When the user asks about projects, use list_projects..."
│   ├── document-editing/
│   │   └── SKILL.md
│   └── collaboration/
│       └── SKILL.md
├── sidecar             # Native binary
└── ui/                 # Static frontend
```

### Loading

`AppLifecycle::launch()` calls `skill_loader.load_app_skills(&tool_dir)`:

1. Scans `<tool_dir>/skills/` for subdirectories containing `SKILL.md`
2. Parses frontmatter (name, description, enabled flag) via `load_skills_from_nested_dir()`
3. Force-enables each skill (`skill.enabled = true`)
4. Inserts into the shared `Loader.skills` map (same as marketplace skills)
5. Returns the list of loaded skill names for cleanup

### Priority / Override Behavior

App skills are loaded **after** `load_all()` (which loads bundled, installed, and
user skills). Since they insert into the same `HashMap<String, Skill>` by name,
app skills with the same name as an existing skill will override it. The effective
priority order during `load_all()` is:

1. Bundled (embedded in binary) — lowest, loaded first
2. Installed `.napp` archives (`nebo/skills/`)
3. Plugin-embedded skills (from `nebo/plugins/`)
4. User loose files (`user/skills/`) — override installed

App skills (from `load_app_skills()`) are loaded separately after `load_all()`
and insert into the same map, so they override any existing skill with the same
name. They are removed on shutdown, restoring the previous skill if one existed.

### Unloading

`AppLifecycle::shutdown()` calls `skill_loader.unload_skills(&loaded_skill_names)`:
removes the skills from the shared map so they don't leak after the app stops.

### Skill Name Resolution

`agent.json` references skills as path-style strings:

```json
{ "skills": ["skills/workspace-management", "skills/document-editing"] }
```

The runner's `extract_skill_name()` converts these to match the SKILL.md
frontmatter `name:` field:

```
"skills/workspace-management" → "workspace-management"
"@nebo/skills/workspace-management@1.0" → "workspace-management"
```

---

## 7. End-to-End Flow

```
1. Server startup / app install
   └── AppLifecycle::new(agent_id, tool_dir, hub, registry, skill_loader)

2. AppLifecycle::launch()
   ├── runtime.launch(&tool_dir)           → starts sidecar process
   ├── discover_tools(&sock_path)          → reads agent.json, builds SidecarActionTools
   │   └── for each AgentToolDef in config.tools:
   │       └── registry.register_for_agent(agent_id, SidecarActionTool)
   ├── skill_loader.load_app_skills(&tool_dir)  → load SKILL.md files
   └── spawn_health_checker()              → monitors process + binary changes

3. User sends chat message (embed → WS → runner)
   ├── tools.agent_tool_names(agent_id)    → {"get_document", "update_document", ...}
   ├── tool_filter includes agent tools    → tools=N (not 0)
   ├── prompt includes skill docs          → LLM knows when/how to use tools
   └── LLM calls get_document(id: "abc")
       └── registry.execute("get_document", input)
           └── SidecarActionTool.execute_dyn()
               └── GrpcSidecarCaller.call("GET", "/documents/abc", "", [])
                   └── UIService.HandleRequest (gRPC over Unix socket)

4. Binary changes on disk (hot reload)
   ├── health_checker detects binary_changed (mtime comparison)
   ├── registry.unregister_agent_tools(agent_id)  → remove old tools
   ├── process.stop() + runtime.launch()           → restart sidecar
   ├── read_tool_defs_from_config()                → re-read agent.json
   └── register_for_agent() for each tool          → register new tools

5. AppLifecycle::shutdown()
   ├── registry.unregister_agent_tools(agent_id)
   ├── skill_loader.unload_skills(&loaded_names)
   └── process.stop()
```

---

## 8. Sidecar Restore on Startup

On server boot, previously-running app sidecars are automatically relaunched.
This is not based on the `agent_workflows` table — it uses the `agents` table
directly.

```rust
// crates/server/src/lib.rs — after tool registration, before comm setup
if let Ok(agents) = state.store.list_agents(1000, 0) {
    for agent in &agents {
        if agent.is_enabled == 0 || agent.is_app.unwrap_or(0) == 0 {
            continue;  // skip non-app or disabled agents
        }
        if let Some(tool_dir) = handlers::agents::app_tool_dir(agent) {
            let mut lifecycle = AppLifecycle::new(...);
            lifecycle.launch().await;  // → discover tools + load skills + spawn health checker
            state.app_lifecycles.write().await.insert(agent.id.clone(), lifecycle);
        }
    }
}
```

The filter is simple: `is_enabled == 1` AND `is_app == 1`. Each restored
lifecycle goes through the full `launch()` path (start process, discover tools,
load skills, spawn health checker), so the agent is fully operational after
server restart.

---

## 9. Binary Hot-Reload Detection

The health checker (spawned per-lifecycle) polls every 15 seconds. It
distinguishes two cases: **binary changed** (hot reload) and **process dead**
(crash). The behavior differs significantly.

### Hot Reload (binary changed, process alive)

Detected via `Process::binary_changed()` — compares `std::fs::metadata(path).modified()`
against the `binary_mtime` captured at launch. Follows symlinks via
`std::fs::canonicalize()` so rebuilds through symlinks are detected.

When a binary change is detected:
1. `registry.unregister_agent_tools(agent_id)` — remove old tools immediately
2. `process.stop()` — SIGTERM, wait 2s, then SIGKILL
3. `runtime.launch(&tool_dir)` — start new process
4. `read_tool_defs_from_config()` + `register_for_agent()` — re-discover tools
5. Broadcast `app_restarted` with `reason: "binary_changed"`

Hot reloads are **immediate** — no backoff, no restart counting. This is the
expected development workflow.

### Crash Restart (process dead)

When `!process.is_alive()` and no binary change:
1. Broadcast `app_crashed`
2. Check `supervisor.should_restart(agent_id)` — respects backoff + limits
3. If allowed: `runtime.launch()` + broadcast `app_restarted` with `restartCount`

### Supervisor Backoff Policy (`crates/napp/src/supervisor.rs`)

- **Max restarts:** 5 per hour (rolling window)
- **Exponential backoff:** 10s, 20s, 40s, 80s, 160s (capped at 5 minutes)
- **Window reset:** after 1 hour of no restarts, count and backoff reset
- **Check interval:** 15 seconds (configured in `Supervisor::new()`)

Note: crash restarts do NOT re-register tools (only hot reloads do).
The sidecar process is relaunched but tool discovery is not re-run after a crash.

---

## 10. Tool Scoping (SDK-driven)

**Status: Implemented**

Publishers define named scopes in `agent.json` that map to subsets of tools,
skills, and plugins. The SDK picks a scope by name when mounting the embed chat.

### agent.json Scopes

```json
{
  "skills": ["skills/workspace-management", "skills/document-editing"],
  "requires": { "plugins": ["gws"] },
  "scopes": {
    "editor": {
      "tools": ["get_document", "update_document", "get_comments"],
      "skills": ["skills/document-editing"]
    },
    "projects": {
      "tools": ["list_projects", "create_project"],
      "skills": ["skills/workspace-management"],
      "plugins": ["gws"]
    }
  }
}
```

### SDK Usage

```typescript
chat.mount(el, { contextId: doc.id, scope: 'editor' });
```

### Data Flow

```
SDK: scope='editor'
  → iframe URL: ?scope=editor&ctx=doc-456
  → WS payload: { scope: 'editor', ... }
  → dispatch_chat → ChatConfig.tool_scope = Some("editor")
  → RunRequest.tool_scope = Some("editor")
  → run_loop resolves scope from agent config:
    1. agent_tool_names intersected with scope.tools
    2. only scope.skills loaded into prompt
    3. scope.plugins merged with requires.plugins
```

### Resolution in run_loop (3 sites)

1. **Skill loading** (lines ~758-784) — `skills_to_load` is `scope.skills` when
   a scope is active, otherwise `cfg.skills` (all)
2. **Plugin pre-activation** (lines ~790-806) — `scope.plugins` merged with
   `requires.plugins` to determine if `plugin` tool should pre-activate
3. **Tool filtering** (lines ~1271-1285) — `agent_tool_names` intersected with
   `scope.tools` before passing to `filter_tools_with_context()`

### Behavior

| SDK `scope` | Tools | Skills | Plugins |
|-------------|-------|--------|---------|
| Not set | All sidecar tools | All agent.json skills | All requires.plugins |
| `"editor"` | Only scope.tools | Only scope.skills | requires.plugins + scope.plugins |
| Unknown name | Warning, falls back to all | Same | Same |

Core system tools (agent, skill, event, message, tool_search) always available
regardless of scope.

---

## 11. Key Files

| File | What It Does |
|------|--------------|
| `crates/tools/src/sidecar_tool.rs` | `SidecarToolDef`, `SidecarActionTool`, `SidecarCaller` trait, `SidecarResponse` |
| `crates/tools/src/registry.rs` | `register_for_agent()`, `agent_tool_names()`, `unregister_agent_tools()` |
| `crates/server/src/app_lifecycle.rs` | `GrpcSidecarCaller`, `AppLifecycle` (launch/shutdown/health/discovery), `read_tool_defs_from_config()` |
| `crates/napp/src/agent.rs` | `AgentConfig`, `AgentToolDef`, `ToolScope`, `parse_agent_config()` |
| `crates/napp/src/runtime.rs` | `Process` (binary_changed, is_alive, stop), `Runtime` (launch) |
| `crates/napp/src/supervisor.rs` | `Supervisor` — restart backoff policy (5/hour, exponential 10s..5min) |
| `crates/agent/src/tool_filter.rs` | `filter_tools_with_context()` with `agent_tool_names` bypass |
| `crates/agent/src/runner.rs` | Fetches agent tool names, intersects with scope, passes to filter each iteration |
| `crates/tools/src/skills/loader.rs` | `load_app_skills()`, `unload_skills()` |
| `@neboai/app-sdk` (npm) `src/chat.ts` | `ChatOptions` with `contextId` and `scope` |
| `app/src/routes/(embed)/chat-embed/[agentId]/+page.svelte` | Reads `ctx` param, derives scoped session key |
| `crates/server/src/handlers/ws.rs` | Session key passthrough for document-scoped sessions |
