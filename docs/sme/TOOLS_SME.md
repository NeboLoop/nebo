# Tools System — Rust SME Reference

> Definitive reference for the Nebo Rust tool system. Covers the tool registry,
> execution pipeline, STRAP domain pattern, every built-in tool, policy and safeguards,
> MCP proxy integration, sandbox policies, script execution, tool filtering, agent
> integration, and the event bus.

---

## Table of Contents

1. [Tool Registry & Execution Pipeline](#1-tool-registry--execution-pipeline)
2. [STRAP Domain Pattern](#2-strap-domain-pattern)
3. [Built-in Domain Tools](#3-built-in-domain-tools)
4. [Platform Tools](#4-platform-tools)
5. [Skill Tool](#5-skill-tool)
6. [Workflow Tool](#6-workflow-tool)
7. [Execute Tool (Script Runtime)](#7-execute-tool-script-runtime)
8. [Emit Tool (Event Emission)](#8-emit-tool-event-emission)
9. [Policy & Safeguards](#9-policy--safeguards)
10. [Process Registry](#10-process-registry)
11. [MCP Proxy Integration](#11-mcp-proxy-integration)
12. [Sandbox Policy](#12-sandbox-policy)
13. [Tool Filtering (Agent)](#13-tool-filtering-agent)
14. [Agent Integration](#14-agent-integration)
15. [Event Bus](#15-event-bus)
16. [Tool Registration Strategies](#16-tool-registration-strategies)
17. [Cross-Reference to Go Docs](#17-cross-reference-to-go-docs)

---

## 1. Tool Registry & Execution Pipeline

**Source:** `crates/tools/src/registry.rs`

### Core Traits

The tool system uses two traits for tool dispatch:

```rust
// Static dispatch — used by concrete tool implementations
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> String;
    fn schema(&self) -> serde_json::Value;
    fn requires_approval(&self) -> bool;
    fn execute(&self, ctx: &ToolContext, input: serde_json::Value)
        -> impl Future<Output = ToolResult> + Send;
}

// Dynamic dispatch — type-erased for registry storage
pub trait DynTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> String;
    fn schema(&self) -> serde_json::Value;
    fn requires_approval(&self) -> bool;
    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value)
        -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>>;
}
```

Every concrete `Tool` implementation gets a blanket `DynTool` impl so it can be stored in the registry.

### ToolResult

```rust
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,                     // default false
    pub image_url: Option<String>,          // optional image attachment
}
```

Factory methods: `ToolResult::ok(content)`, `ToolResult::error(content)`.

### Registry Struct

```rust
pub struct Registry {
    tools: Arc<RwLock<HashMap<String, Box<dyn DynTool>>>>,
    policy: Arc<RwLock<Policy>>,
    process_registry: Arc<ProcessRegistry>,
    bridge: std::sync::RwLock<Option<Arc<mcp::Bridge>>>,
}
```

### Execution Pipeline

When `Registry::execute()` is called:

1. **MCP prefix stripping** — if tool name starts with `mcp__`, strip to get original name and delegate to MCP bridge
2. **Hard safeguard check** — unconditional security gates (see [Policy & Safeguards](#9-policy--safeguards))
3. **Origin-based policy check** — deny-list per origin (workflow vs. agent vs. CLI)
4. **Tool lookup** — find `DynTool` in HashMap by name
5. **Dispatch** — call `tool.execute_dyn(ctx, input)`
6. **Return** — `ToolResult` with content, error flag, optional image

### Tool Correction

When the agent hallucinates tool names, the registry provides helpful suggestions:

| Hallucinated | Suggestion |
|---|---|
| `websearch` | `web(action: "search", query: "...")` |
| `read` | `system(resource: "file", action: "read", path: "...")` |
| `bash` | `system(resource: "shell", action: "exec", command: "...")` |
| `app`/`napp` | `skill(action: "catalog")` |
| `workflow` | `work(action: "list")` |

### Key Methods

```rust
pub async fn register(&self, tool: Box<dyn DynTool>)
pub async fn unregister(&self, name: &str)
pub async fn execute(&self, ctx: &ToolContext, tool_name: &str, input: Value) -> ToolResult
pub async fn list(&self) -> Vec<ToolDefinition>
pub async fn register_defaults(&self)
pub async fn register_all(&self, store: Arc<Store>, orchestrator: OrchestratorHandle)
pub async fn register_all_with_permissions(&self, store: Arc<Store>, ..., permissions: Option<Vec<String>>)
pub fn set_bridge(&self, bridge: Arc<mcp::Bridge>)
```

---

## 2. STRAP Domain Pattern

**Source:** `crates/tools/src/domain.rs`, `crates/agent/src/strap/`

STRAP = **S**ingle **T**ool, **R**esource, **A**ction, **P**arameters.

Every domain tool follows the same input schema:

```json
{
  "resource": "file",
  "action": "read",
  "path": "/etc/hosts"
}
```

### DomainInput

```rust
pub struct DomainInput {
    pub resource: String,
    pub action: String,
}
```

Additional parameters (name, content, path, query, etc.) are parsed from the raw JSON value inside each tool's `execute()` implementation.

### STRAP Documentation

Each tool has a `.txt` file in `crates/agent/src/strap/` describing its resources, actions, and parameters. These are platform-specific (macOS, Linux, Windows) and are injected into the system prompt based on which tools are registered.

```rust
fn build_strap_section(tool_names: &[String]) -> String {
    // Only include docs for tools currently in the filtered set
}
```

---

## 3. Built-in Domain Tools

**Source:** `crates/tools/src/bot_tool.rs`, `crates/tools/src/` (various)

### BotTool (`bot`)

The primary agent self-management tool. Always registered.

| Resource | Actions | Purpose |
|---|---|---|
| **memory** | `store`, `recall`, `search`, `list`, `delete`, `clear` | Namespaced key-value storage with hybrid search (FTS5 + vector) |
| **task** | `spawn`, `orchestrate`, `status`, `cancel`, `create`, `update`, `list`, `delete` | Sub-agent spawning, DAG orchestration, pending task CRUD |
| **session** | `list`, `history`, `status`, `clear`, `query` | Chat message history, cross-session search |
| **profile** | `get`, `update`, `open_billing` | Agent name, role, personality, emoji, creature, vibe |
| **context** | `summary`, `reset`, `compact` | Session message counts, auto-compaction |
| **advisors** | `deliberate`, `list` | Advisor runner integration (LLM deliberation) |
| **ask** | `prompt`, `confirm`, `select` | Structured UI prompts with await support |

**Key Traits:**

```rust
// Advisor reasoning interface
pub trait AdvisorDeliberator: Send + Sync {
    fn deliberate(&self, session_id: &str, question: &str)
        -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>>;
}

// Combined text + vector search
pub trait HybridSearcher: Send + Sync {
    fn search(&self, query: &str, user_id: &str, namespace: Option<&str>, limit: usize)
        -> Pin<Box<dyn Future<Output = Vec<MemoryResult>> + Send + '_>>;
}
```

**Memory Namespaces:**
- `tacit/personality` — style observations
- `tacit/preferences` — user preferences
- `tacit/artifacts` — documents created
- `entity/default` — people, places, things
- `daily/{date}` — daily facts

**Builder Pattern:**
```rust
let mut bot = BotTool::new(store, orchestrator);
bot = bot.with_advisor_runner(runner);
bot = bot.with_hybrid_searcher(searcher);
```

### SystemTool (`system`)

Umbrella for file and shell tools. Always registered.

| Resource | Actions | Purpose |
|---|---|---|
| **file** | `read`, `write`, `edit`, `glob`, `grep`, `mkdir`, `delete`, `move`, `copy` | File system operations |
| **shell** | `exec` | Shell command execution |

### WebTool (`web`)

| Resource | Actions | Purpose |
|---|---|---|
| (default) | `search`, `fetch`, `browse`, `screenshot` | Web search, page fetching, browser control |

Requires `"web"` permission. Optional browser manager integration.

### EventTool (`event`)

| Resource | Actions | Purpose |
|---|---|---|
| (default) | `create`, `list`, `update`, `delete`, `toggle` | Scheduled tasks (cron jobs) via DB |

Always registered.

### MessageTool (`message`)

| Resource | Actions | Purpose |
|---|---|---|
| (default) | `send`, `notify` | Owner notifications (OS-native) |

Always registered.

### LoopTool (`loop`)

| Resource | Actions | Purpose |
|---|---|---|
| (default) | `send`, `list`, `join`, `leave` | NeboLoop channel communication |

Requires `"loop"` permission and active `CommPlugin`.

---

## 4. Platform Tools

**Source:** `crates/tools/src/settings_tool.rs`, `music_tool.rs`, `keychain_tool.rs`

### SettingsTool (`settings`)

Cross-platform system settings control. Requires `"system"` permission.

| Resource | macOS | Linux | Windows |
|---|---|---|---|
| `volume` | osascript | pactl/amixer | n/a |
| `brightness` | DisplayServices.framework | brightnessctl/xbacklight | WmiMonitorBrightness |
| `wifi` | networksetup | nmcli/rfkill | netsh wlan |
| `bluetooth` | IOBluetooth framework | bluetoothctl/rfkill | n/a |
| `battery` | pmset | upower/sysfs | WMI battery |
| `darkmode` | System Events preferences | n/a | Registry |
| `sleep` | pmset sleepnow | systemctl suspend | rundll32 powrprof |
| `lock` | pmset displaysleepnow | loginctl/xdg-screensaver | LockWorkStation |
| `info` | osascript system info | /proc + uname | WMI GetWmiObject |
| `mute`/`unmute` | osascript | pactl/amixer | SendKeys |

### MusicTool (`music`)

Media playback control. Requires `"media"` permission.

| Action | macOS | Linux | Windows |
|---|---|---|---|
| `play`/`pause`/`next`/`previous` | AppleScript → Music.app | playerctl (MPRIS D-Bus) | PowerShell SMTC |
| `status` | AppleScript | playerctl | PowerShell (read-only) |
| `search` | AppleScript | n/a | n/a |
| `volume` | AppleScript | playerctl | n/a |
| `playlists` | AppleScript | n/a | n/a |
| `shuffle` | AppleScript | playerctl | n/a |

### KeychainTool (`keychain`)

Credential storage. Requires `"system"` permission. **Requires approval.**

| Action | macOS | Linux | Windows |
|---|---|---|---|
| `get` | `security find-generic-password` | `secret-tool lookup` | `cmdkey /list` |
| `find` | `security find-generic-password -l` | `secret-tool search` | `cmdkey /list` |
| `add` | `security add-generic-password` | `secret-tool store` | `cmdkey /add` |
| `delete` | `security delete-generic-password` | `secret-tool clear` | `cmdkey /delete` |

### Other Platform Tools

| Tool | Permission | Purpose |
|---|---|---|
| **DesktopTool** | `"desktop"` | Mouse, keyboard, window management |
| **AppTool** | `"desktop"` | OS app control (launch, quit, list) |
| **SpotlightTool** | `"system"` | File search (macOS Spotlight, Linux locate) |
| **OrganizerTool** | `"organizer"` | Calendar, contacts, reminders, mail |

---

## 5. Skill Tool

**Source:** `crates/tools/src/skill_tool.rs`

Agent-facing tool for skill management. Always registered (core tool).

```rust
pub struct SkillTool {
    loader: Arc<Loader>,
}
```

### Actions

| Action | Required Params | Purpose |
|---|---|---|
| `catalog` / `list` | — | List all skills with status, source, triggers, capabilities |
| `help` | `name` | Show full SKILL.md content + resource listing |
| `browse` | `name`, `path?` | List resource files in skill directory with sizes |
| `read_resource` | `name`, `path` | Read a specific resource file (path-traversal protected) |
| `load` | `name` | Enable skill (rename `.yaml.disabled` → `.yaml`) |
| `unload` | `name` | Disable skill (rename `.yaml` → `.yaml.disabled`) |
| `create` | `name`, `content` | Write new skill (auto-detects SKILL.md vs .yaml from content) |
| `update` | `name`, `content` | Modify existing skill |
| `delete` | `name` | Remove skill files (both .yaml and directory) |

### Catalog Output Format

```
{count} skills:
- {name} [{enabled|disabled}|{nebo|user}] — {description}
    capabilities: [python, storage]
    resources: (N resource files)
    triggers: (trigger1, trigger2)
```

See [skills-and-tools.md](skills-and-tools.md) for the full skill lifecycle reference.

---

## 6. Workflow Tool

**Source:** `crates/tools/src/workflows/work_tool.rs`

Agent-facing tool for workflow lifecycle management. Requires `workflow_manager`.

```rust
pub struct WorkTool {
    manager: Arc<dyn WorkflowManager>,
}
```

**Requires approval:** Yes.

### Lifecycle Actions (no resource)

| Action | Params | Purpose |
|---|---|---|
| `list` | — | Show all workflows |
| `create` | `name`, `definition` | Create from JSON definition |
| `install` | `code` | Install from marketplace (WORK-XXXX-XXXX) |
| `uninstall` | `id` | Remove by ID |
| `cancel` | `id` | Cancel running workflow by run_id |

### Dispatch Actions (resource = workflow name/id)

| Action | Params | Purpose |
|---|---|---|
| `run` | `inputs?` | Start workflow (async, returns run_id) |
| `status` | — | Latest run info |
| `runs` | — | Last 10 runs |
| `toggle` | — | Enable/disable |

### WorkflowManager Trait

```rust
pub trait WorkflowManager: Send + Sync {
    async fn list(&self) -> Vec<WorkflowInfo>;
    async fn install(&self, code: &str) -> Result<WorkflowInfo, String>;
    async fn uninstall(&self, id: &str) -> Result<(), String>;
    async fn resolve(&self, name_or_id: &str) -> Result<WorkflowInfo, String>;
    async fn run(&self, id: &str, inputs: Value, trigger_type: &str) -> Result<String, String>;
    async fn run_status(&self, run_id: &str) -> Result<WorkflowRunInfo, String>;
    async fn list_runs(&self, workflow_id: &str, limit: i64) -> Vec<WorkflowRunInfo>;
    async fn toggle(&self, id: &str) -> Result<bool, String>;
    async fn create(&self, name: &str, definition: &str) -> Result<WorkflowInfo, String>;
    async fn cancel(&self, run_id: &str) -> Result<(), String>;
}
```

### DTOs

```rust
pub struct WorkflowInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub is_enabled: bool,
    pub trigger_count: usize,
    pub activity_count: usize,
}

pub struct WorkflowRunInfo {
    pub id: String,
    pub workflow_id: String,
    pub status: String,               // "running", "completed", "failed", "cancelled"
    pub trigger_type: String,
    pub total_tokens_used: Option<i64>,
    pub error: Option<String>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
}
```

---

## 7. Execute Tool (Script Runtime)

**Source:** `crates/tools/src/execute_tool.rs`

Runs Python/TypeScript scripts from skill resource directories. **Requires approval.**

```rust
pub struct ExecuteTool {
    loader: Arc<Loader>,
    plan_tier: Arc<RwLock<String>>,
    sandbox: Option<Arc<SandboxManager>>,
}
```

Only registered when both `skill_loader` and `plan_tier` are available.

### Execution Paths

**Local (free tier):**

1. Detect language from file extension (`.py` → Python, `.ts`/`.js` → TypeScript)
2. Find runtime:
   - Bundled: `/tmp/nebo-runtimes/uv` (Python) or `/tmp/nebo-runtimes/bun` (TypeScript)
   - System: `python3`/`python`, `node`/`tsx` via PATH
3. Extract ALL skill resources to temp directory (preserves relative imports)
4. Build sandbox config from skill capabilities
5. Run subprocess: `uv run script.py` or `bun run script.ts`
6. Pass `SKILL_ARGS` environment variable
7. Enforce timeout (default 30s)
8. Capture stdout/stderr
9. Annotate sandbox violations in stderr

**Cloud (paid tier — stubbed):**

- POST `/v1/execute` to Janus with script + capabilities
- Remote sandbox handles execution

**Unsupported (fallback):**

- Structured error with upgrade/install options

### Resource Extraction

Before script execution, ALL files from the skill directory are extracted to a temp working directory preserving relative paths. This allows:
- `from scripts.utils import helper` (Python)
- `import ./lib/common.ts` (TypeScript)
- Asset file references via relative paths

---

## 8. Emit Tool (Event Emission)

**Source:** `crates/tools/src/emit_tool.rs`

Allows workflow activities to fire events into the EventBus. Injected automatically by the workflow engine — no tool declaration needed.

```rust
pub struct EmitTool {
    bus: EventBus,
}
```

### Input

```json
{
  "source": "email.urgent",
  "payload": { "sender": "vip@example.com" }
}
```

- `source` (string, required): Event source identifier
- `payload` (object, optional): Arbitrary event data

### Output

Returns confirmation with timestamp and origin trace.

---

## 9. Policy & Safeguards

**Source:** `crates/tools/src/policy.rs`, `crates/tools/src/safeguard.rs`, `crates/tools/src/origin.rs`

### Origin Tracking

```rust
pub struct ToolContext {
    pub origin: Origin,
    pub session_id: String,
    pub user_id: String,
}

pub enum Origin {
    Agent,      // Interactive agent conversation
    Workflow,   // Background workflow execution
    Cli,        // CLI direct invocation
    Mcp,        // MCP proxy call
}
```

### Policy System

```rust
pub struct Policy {
    pub level: PolicyLevel,
    pub ask_mode: AskMode,
    // ... deny lists per origin
}

pub enum PolicyLevel {
    Unrestricted,
    Standard,
    Restricted,
}

pub enum AskMode {
    Always,     // Always ask user before tool execution
    Smart,      // Ask for destructive/sensitive operations
    Never,      // Never ask (autonomous mode)
}
```

**Origin-based restrictions:** Each origin can have a deny-list preventing specific tool+resource combinations from executing. For example, workflows might be denied `system/shell` access.

### Safeguard System

Hard security gates checked unconditionally before every tool execution:

```rust
fn check_safeguard(tool_name: &str, input: &serde_json::Value) -> Option<String>
```

If a safeguard returns `Some(reason)`, execution is immediately blocked with `ToolResult::error(reason)`. Safeguards cannot be overridden by policy level.

---

## 10. Process Registry

**Source:** `crates/tools/src/process.rs`

Tracks child processes spawned by tools (shell commands, scripts, etc.).

```rust
pub struct ProcessRegistry {
    processes: Arc<RwLock<HashMap<u32, ProcessInfo>>>,
}

pub struct ProcessInfo {
    pub pid: u32,
    pub command: String,
    pub started_at: u64,
}
```

### Key Methods

```rust
pub fn register(&self, pid: u32, command: &str)
pub fn unregister(&self, pid: u32)
pub fn list(&self) -> Vec<ProcessInfo>
pub fn kill(&self, pid: u32) -> Result<(), String>
pub fn kill_all(&self)
```

Used by `SystemTool` (shell exec) and `ExecuteTool` (script execution) to track and clean up child processes.

---

## 11. MCP Proxy Integration

**Source:** `crates/mcp/src/bridge.rs`, `crates/tools/src/registry.rs`

### Architecture

The MCP Bridge manages connections to external MCP servers and registers proxy tools into the agent's registry.

```rust
// Bridge manages external tool connections
pub struct Bridge {
    connections: Mutex<HashMap<String, Connection>>,
}

struct Connection {
    integration_id: String,
    server_type: String,
    tool_names: Vec<String>,       // Registered proxy tool names
}
```

### Tool Name Convention

External tools are namespaced: `mcp__{server_type}__{tool_name}`

Example: `mcp__brave-search__web_search`

### McpProxyTool

Dynamically created tool that delegates to MCP bridge:

```rust
struct McpProxyTool {
    proxy_name: String,              // "mcp__brave-search__web_search"
    original_name: String,           // "web_search"
    tool_description: String,
    tool_schema: Option<Value>,
    integration_id: String,
    bridge: Arc<mcp::Bridge>,
}
```

### ProxyToolRegistry Trait

The Registry implements this trait so the Bridge can register/unregister proxy tools:

```rust
impl mcp::bridge::ProxyToolRegistry for Registry {
    fn register_proxy(&self, name, original_name, description, schema, integration_id);
    fn unregister_proxy(&self, name);
}
```

### Lifecycle

```
Bridge::sync_all(integrations)
├─ Disconnect stale integrations (unregister proxy tools)
├─ Connect new integrations
│  ├─ list_tools() from MCP server
│  ├─ register_proxy() for each tool
│  └─ Store Connection
└─ Bridge::call_tool(integration_id, tool_name, input) → forward remotely
```

### Workflow Exclusion

MCP proxy tools are **explicitly filtered out** from workflow execution:

```rust
let resolved_tools: Vec<Box<dyn DynTool>> = tool_defs
    .iter()
    .filter(|td| !td.name.starts_with("mcp__"))
    .map(|td| { /* wrap in RegistryTool */ })
    .collect();
```

Only built-in tools and .napp tools are available to workflow activities.

---

## 12. Sandbox Policy

**Source:** `crates/tools/src/sandbox_policy.rs`

Capability-based security boundaries for script execution within skills.

### SandboxRuntimeConfig

```rust
pub struct SandboxRuntimeConfig {
    pub filesystem: FilesystemConfig,
    pub network: NetworkConfig,
}

pub struct FilesystemConfig {
    pub allow_write: Vec<String>,
    pub deny_read: Vec<String>,
}

pub struct NetworkConfig {
    pub allowed_domains: Vec<String>,
}
```

### Capability Mapping

```rust
pub fn build_sandbox_config(skill: &Skill, work_dir: &Path) -> SandboxRuntimeConfig
```

| Capability | Filesystem | Network |
|---|---|---|
| *(base)* | write: work_dir, /dev/std*, /tmp/nebo | blocked |
| `storage` | + write: data_dir | — |
| `network` | — | + pypi.org, npm registry, + metadata `allowed_domains` |

### Always Denied (Read)

```
~/.ssh/
~/.gnupg/
~/.aws/credentials
~/.config/gcloud
```

These paths are **always** in the deny-read list regardless of capabilities.

### Always Allowed (Write)

```
{work_dir}          # Isolated temp directory
/dev/stdout
/dev/stderr
/tmp/nebo
```

---

## 13. Tool Filtering (Agent)

**Source:** `crates/agent/src/tool_filter.rs`

Smart contextual filtering determines which tools the LLM sees on each turn.

### Two Tiers

**Tier 1: Core Tools (always included)**

```rust
const CORE_TOOLS: &[&str] = &[
    "system", "web", "bot", "loop", "event", "message", "skill", "work"
];
```

**Tier 2: Contextual Groups (keyword-triggered)**

```rust
const CONTEXTUAL_GROUPS: &[(&str, &[&str])] = &[
    ("screenshot", &["screenshot", "screen", "capture", "visible", "see what"]),
    ("vision",     &["image", "photo", "picture", "screenshot", "visual"]),
    ("desktop",    &["click", "type", "mouse", "keyboard", "window", "app", "open"]),
    ("organizer",  &["calendar", "reminder", "contact", "email", "schedule"]),
];
```

### Algorithm

1. Always include all core tools (8 tools)
2. Scan the last 5 messages for contextual keywords
3. Include contextual tools when keywords match OR when they were recently called
4. If no tools selected after filtering: fallback to ALL tools (never empty)

This keeps the token count low while ensuring relevant tools are available.

---

## 14. Agent Integration

**Source:** `crates/agent/src/runner.rs`, `crates/agent/src/prompt.rs`, `crates/agent/src/steering.rs`

### Tool Injection into Prompts

Tools appear in the agent prompt in three ways:

1. **STRAP documentation** — per-tool `.txt` files from `crates/agent/src/strap/`, only for filtered tools
2. **Tool list reinforcement** — explicit list of available tool names in the system prompt
3. **Tool routing guide** — intent-to-tool mapping (e.g., "Files → system(resource: file, action: read)")

### Steering Nudges (Tool-Related)

| Generator | Fires When | Message |
|---|---|---|
| **Tool nudge** | 5+ turns without tool use, active task | "Consider using your available tools" |
| **Pending task action** | Text-only response, task incomplete | "Call a tool RIGHT NOW to continue" |
| **Loop detector** | Same tool 4+ times consecutively | "Pause and report findings" (4x) / "STOP calling this tool" (6x) |
| **Progress nudge** | Turn 10, then every 10th | "Assess progress" / "Wrap up now" |

### Full Tool Call Lifecycle

```
1. Build ChatRequest
   ├─ messages (with steering injected)
   ├─ tools (filtered tool definitions)
   ├─ system (static + dynamic prompt)
   └─ model selection

2. Acquire LLM permit → stream from provider

3. Process stream events
   ├─ Text → accumulate assistant content
   └─ ToolCall → collect tool_calls vector

4. Hook: message.post_receive (apps can modify response)

5. Save assistant message (with serialized tool_calls)

6. Parallel tool execution (FuturesUnordered)
   ├─ Acquire tool permit per call
   ├─ 300-second timeout per tool
   ├─ Registry.execute(ctx, name, input)
   └─ Collect results as futures complete

7. Sidecar vision verification (for screenshot results)

8. Save tool results as "tool" role messages

9. Hook: agent.turn (notify apps of tool usage)

10. Loop decision
    ├─ Tool calls present → continue (LLM responds to results)
    ├─ Active task + continuation pause → auto-continue
    └─ No tools, no continuation → break
```

### Concurrency & Timeouts

```rust
const TOOL_EXECUTION_TIMEOUT: Duration = Duration::from_secs(300);  // 5 minutes
```

- Tool calls execute concurrently via `FuturesUnordered`
- Each tool gets a concurrency permit (`acquire_tool_permit()`)
- LLM streaming uses a separate permit pool (`acquire_llm_permit()`)

---

## 15. Event Bus

**Source:** `crates/tools/src/events.rs`

Inter-workflow communication via unbounded async channel.

```rust
pub struct Event {
    pub source: String,               // "email.urgent", "workflow.triage.completed"
    pub payload: serde_json::Value,
    pub origin: String,               // "workflow:email-triage:run-550e"
    pub timestamp: u64,               // unix seconds
}

pub struct EventBus {
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
}
```

### API

```rust
pub fn new() -> (EventBus, UnboundedReceiver<Event>)
pub fn emit(&self, event: Event)   // Best-effort, logged if dropped
```

The receiver is consumed by `EventDispatcher` (in `crates/workflow/src/events.rs`) which matches event sources against subscription patterns and triggers workflows.

---

## 16. Tool Registration Strategies

**Source:** `crates/tools/src/registry.rs`

### register_defaults()

System tool only, no database required:

```rust
pub async fn register_defaults(&self) {
    let system_tool = SystemTool::new(policy, self.process_registry.clone());
    self.register(Box::new(system_tool)).await;
}
```

### register_all_with_permissions()

Full tool suite with capability-based filtering:

| Permission | Tools Registered |
|---|---|
| *(always)* | SystemTool, BotTool, EventTool, SkillTool, MessageTool |
| `"web"` | WebTool |
| `"desktop"` | DesktopTool, AppTool |
| `"media"` | MusicTool |
| `"system"` | SettingsTool, SpotlightTool, KeychainTool |
| `"organizer"` | OrganizerTool |
| `"loop"` | LoopTool (also requires CommPlugin) |

### Conditional Registration

| Tool | Required Parameters |
|---|---|
| ExecuteTool | `skill_loader` + `plan_tier` |
| WorkTool | `workflow_manager` |
| WebTool | `"web"` permission + optional browser |
| LoopTool | `"loop"` permission + CommPlugin |

---

## 17. Cross-Reference to Go Docs

| Rust (this doc) | Go Equivalent |
|---|---|
| `crates/tools/src/registry.rs` | `internal/agent/tools/registry.go` |
| `crates/tools/src/bot_tool.rs` | `internal/agent/tools/bot_domain.go` |
| `crates/tools/src/skill_tool.rs` | `internal/agent/tools/skill_domain.go` |
| `crates/tools/src/settings_tool.rs` | `internal/agent/tools/settings_domain.go` |
| `crates/tools/src/execute_tool.rs` | New in Rust (no Go equivalent) |
| `crates/tools/src/emit_tool.rs` | New in Rust (no Go equivalent) |
| `crates/tools/src/sandbox_policy.rs` | New in Rust (no Go equivalent) |
| `crates/tools/src/events.rs` | New in Rust (no Go equivalent) |
| `crates/agent/src/tool_filter.rs` | `internal/agent/tools/filter.go` |
| `crates/mcp/src/bridge.rs` | `internal/mcp/bridge.go` |

**Go-only docs for reference:**
- [agent-tools.md](agent-tools.md) — Go agent tools deep-dive
- [platform-tools.md](platform-tools.md) — Go platform tools deep-dive

**Rust comprehensive reference:**
- [skills-and-tools.md](skills-and-tools.md) — Covers skills + tools together with .napp packaging and hooks

---

*Last updated: 2026-03-08*
