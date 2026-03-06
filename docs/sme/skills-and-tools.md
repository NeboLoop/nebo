# Skills & Tools System — Rust SME Reference

> Definitive reference for the entire skills-and-tools stack in the Nebo Rust codebase.
> Covers tool registration, execution pipeline, STRAP domain pattern, built-in tools,
> policy/safeguards, process registry, skill lifecycle, .napp packaging, hooks, the
> ROLE/WORK/TOOL/SKILL hierarchy, and agent integration.

---

## Table of Contents

1. [Tool Registry & Execution Pipeline](#1-tool-registry--execution-pipeline)
2. [STRAP Domain Pattern](#2-strap-domain-pattern)
3. [Built-in Domain Tools Reference](#3-built-in-domain-tools-reference)
4. [Policy & Safeguards](#4-policy--safeguards)
5. [Process Registry](#5-process-registry)
6. [Skill System](#6-skill-system)
7. [.napp Package Format](#7-napp-package-format)
8. [Hooks System](#8-hooks-system)
9. [ROLE/WORK/TOOL/SKILL Hierarchy](#9-roleworktoolskill-hierarchy)
10. [Agent Integration](#10-agent-integration)
11. [Cross-Reference to Go Docs](#11-cross-reference-to-go-docs)

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

**Key methods:**

| Method | Purpose |
|--------|---------|
| `register(tool)` | Add or replace a tool |
| `unregister(name)` | Remove tool by name |
| `get_tool_names()` | List registered tool names |
| `list()` | Return AI-friendly `ToolDefinition` list |
| `execute(ctx, tool_name, input)` | Main execution entry point |
| `set_bridge(bridge)` | Attach MCP bridge for proxy tools |
| `set_policy(policy)` | Update policy |
| `process_registry()` | Access background process manager |
| `register_defaults()` | Register system tool only (no DB) |
| `register_all(store, orchestrator)` | Register all tools with DB access |
| `register_all_with_browser(store, browser, ...)` | All tools + browser manager |
| `register_all_with_permissions(store, ..., perms)` | Filtered by permission map |

### Registration Tiers

**`register_defaults()`** — System tool only (for minimal/CLI mode).

**`register_all(store, orchestrator)`** — Full tool set with database.

**`register_all_with_permissions(store, ..., permissions)`** — Permission-filtered:

| Permission Key | Tools Registered |
|----------------|-----------------|
| *(always)* | system, bot, event, skill, message |
| `"web"` | WebTool |
| `"desktop"` | DesktopTool |
| `"system"` | SettingsTool, SpotlightTool |

### Execution Pipeline

```
Registry.execute(ctx, tool_name, input)
    │
    ├─ 1. Strip MCP prefix: "mcp__{server}__{tool}" → "tool"
    │
    ├─ 2. Safeguard check (UNCONDITIONAL — cannot be bypassed)
    │     └─ check_safeguard(tool_name, input) → block if dangerous
    │
    ├─ 3. Origin deny-list check
    │     └─ policy.is_denied_for_origin(origin, tool_name) → block if denied
    │
    ├─ 4. Tool lookup in registry
    │     ├─ Found → tool.execute_dyn(ctx, input)
    │     └─ Not found → return error with suggestion (tool_correction)
    │
    └─ 5. Return ToolResult
```

### MCP Proxy Tools

```rust
struct McpProxyTool {
    proxy_name: String,         // prefixed name for registry
    original_name: String,      // name sent to MCP server
    tool_description: String,
    tool_schema: Option<serde_json::Value>,
    integration_id: String,
    bridge: Arc<mcp::Bridge>,
}
```

MCP tools are registered with a `mcp__{server}__{tool}` prefix. When executed, the prefix is stripped and the call is forwarded to the MCP bridge.

### Tool Correction (Hallucination Recovery)

The `tool_correction(name)` function provides suggestions for common LLM hallucinations:

| Hallucinated Name | Suggested Alternative |
|-------------------|-----------------------|
| `websearch` | `web(action: "search", ...)` |
| `read` | `system(resource: "file", action: "read", ...)` |
| `write` | `system(resource: "file", action: "write", ...)` |
| `grep` | `system(resource: "file", action: "grep", ...)` |
| *(etc.)* | *(redirects to STRAP domain pattern)* |

---

## 2. STRAP Domain Pattern

**Source:** `crates/tools/src/domain.rs`

STRAP (Structured Tool Resource-Action Pattern) consolidates related operations under a single tool name using `resource` + `action` parameters instead of creating many separate tools.

### Core Structs

```rust
pub struct DomainInput {
    pub resource: String,       // e.g. "file", "shell", "http"
    pub action: String,         // e.g. "read", "exec", "fetch"
}

pub struct ResourceConfig {
    pub name: String,
    pub actions: Vec<String>,
    pub description: String,
}

pub struct FieldConfig {
    pub name: String,
    pub field_type: String,     // "string", "integer", "boolean", "array", "object"
    pub description: String,
    pub required: bool,
    pub enum_values: Vec<String>,
    pub default: Option<serde_json::Value>,
}

pub struct DomainSchemaConfig {
    pub domain: String,         // tool name, e.g. "system"
    pub description: String,
    pub resources: HashMap<String, ResourceConfig>,
    pub fields: Vec<FieldConfig>,
    pub examples: Vec<String>,
}
```

### Key Functions

| Function | Purpose |
|----------|---------|
| `validate_resource_action(resource, action, resources)` | Validate resource/action against allowlist |
| `build_domain_schema(cfg)` | Generate JSON schema for LLM tool calls |
| `build_domain_description(cfg)` | Generate human-readable description text |
| `action_requires_approval(action, dangerous_actions)` | Check if action needs user approval |

### Schema Generation

`build_domain_schema()` generates an OpenAI-compatible JSON schema:
- Includes `resource` enum if multiple resources exist
- Builds `action` enum from all resources' actions
- Marks required fields from `FieldConfig`
- Single-resource tools can use a default (empty-key) resource

---

## 3. Built-in Domain Tools Reference

### 3.1 SystemTool — `"system"`

**Source:** `crates/tools/src/system_tool.rs`

Consolidates file and shell operations under one STRAP domain.

```rust
pub struct SystemTool {
    file_tool: FileTool,
    shell_tool: ShellTool,
}
```

**Resource inference:** The tool infers resource from action when not specified:
- File actions: `read`, `write`, `edit`, `glob`, `grep`
- Shell actions: `exec`, `poll`, `log`

#### 3.1.1 FileTool — Resource: `file`

**Source:** `crates/tools/src/file_tool.rs`

```rust
pub struct FileTool {
    _rg_path: Option<String>,
    pub on_file_read: Option<Box<dyn Fn(&str) + Send + Sync>>,
}
```

| Action | Parameters | Description |
|--------|-----------|-------------|
| `read` | path, offset?, limit? (default 2000) | Read file with line numbering |
| `write` | path, content, append? | Write/append to file; creates parent dirs |
| `edit` | path, old_string, new_string, replace_all? | String replacement in file |
| `glob` | path?, pattern, limit? (default 1000) | List files matching glob pattern |
| `grep` | regex, path?, glob?, case_insensitive?, limit? (default 100) | Pattern search via ripgrep or Rust fallback |

**Security features:**
- `expand_path()` — Resolves `~/` to home directory
- `sensitive_paths()` — Blocks access to: `~/.ssh`, `~/.aws`, `~/.config/gcloud`, `~/.azure`, `~/.gnupg`, `~/.docker/config.json`, `~/.kube/config`, `~/.npmrc`, `~/.password-store`, `~/.bashrc`, `~/.bash_profile`, `~/.zshrc`, `~/.zprofile`, `~/.profile`, `/etc/shadow`, `/etc/passwd`, `/etc/sudoers`
- `validate_file_path()` — Checks sensitive paths and resolves symlinks

#### 3.1.2 ShellTool — Resource: `shell`

**Source:** `crates/tools/src/shell_tool.rs`

```rust
pub struct ShellTool {
    _policy: Policy,
    registry: Arc<ProcessRegistry>,
}
```

**Resource: `bash` (shell)**

| Action | Parameters | Description |
|--------|-----------|-------------|
| `exec` | command, cwd?, timeout? (default 120s) | Synchronous shell execution |
| `background` | command, cwd? | Spawn background session |

**Resource: `process`**

| Action | Parameters | Description |
|--------|-----------|-------------|
| `list` | filter? | List all processes |
| `kill` | pid, signal? (default TERM) | Kill process by PID |
| `info` | pid | Process details (via ps) |

**Resource: `session`**

| Action | Parameters | Description |
|--------|-----------|-------------|
| `list` | | List running and finished sessions |
| `poll` | id | Check status, get new output |
| `log` | id | Full output log |
| `write` | id, data | Send data to stdin |
| `kill` | id | Kill session |

### 3.2 WebTool — `"web"`

**Source:** `crates/tools/src/web_tool.rs`

```rust
pub struct WebTool {
    client: reqwest::Client,                // 30s timeout, 5 redirects
    browser: Option<Arc<browser::Manager>>,
}
```

**Resource: `http`**

| Action | Parameters | Description |
|--------|-----------|-------------|
| `fetch` / `get` | url, headers? | HTTP GET |
| `post` | url, body?, headers? | HTTP POST |
| `put` | url, body?, headers? | HTTP PUT |
| `delete` | url, headers? | HTTP DELETE |
| `head` | url, headers? | HTTP HEAD |

SSRF protection blocks: localhost, 10.x, 172.16-31.x, 192.168.x, link-local. HTML is stripped for readability. Large responses are paginated at 20KB chunks via offset.

**Resource: `search`**

| Action | Parameters | Description |
|--------|-----------|-------------|
| `search` / `query` | query | DuckDuckGo HTML search (no API key) |

Returns up to 10 results with title, URL, snippet.

**Resource: `browser`**

Full browser automation via Chrome extension:

| Action | Description |
|--------|-------------|
| `navigate` | Go to URL |
| `read_page` / `snapshot` | Accessibility tree |
| `click` / `double_click` / `triple_click` / `right_click` | Mouse clicks (by ref or selector) |
| `hover` | Hover over element |
| `fill` / `form_input` | Set input value directly |
| `type` | Character-by-character key events |
| `select` | Select dropdown option |
| `screenshot` | Capture page |
| `scroll` / `scroll_to` | Scroll operations |
| `press` / `key` | Keyboard shortcuts and sequences |
| `drag` | Drag element |
| `go_back` / `go_forward` | Navigation |
| `wait` | Wait for duration (ms or seconds) |
| `evaluate` | Run JS expression |
| `list_tabs` / `new_tab` / `close_tab` | Tab management |
| `status` | Extension connection status |

Browser returns accessibility tree with element refs (`ref_1`, `ref_2`, etc.) used for targeting.

### 3.3 BotTool — `"bot"`

**Source:** `crates/tools/src/bot_tool.rs`

```rust
pub struct BotTool {
    store: Arc<Store>,
    orchestrator: OrchestratorHandle,
    advisor_runner: Option<Arc<dyn AdvisorDeliberator>>,
    hybrid_searcher: Option<Arc<dyn HybridSearcher>>,
}
```

| Resource | Actions | Description |
|----------|---------|-------------|
| `memory` | store, recall, search | Persistent memory management |
| `task` | spawn, orchestrate, status, cancel, create, update, delete | Sub-agent and task management |
| `session` | history, query | Session history access |
| `profile` | get, open_billing | Agent profile and billing |
| `context` | reset, compact, summary | Context window management |
| `advisors` | deliberate | Multi-turn advisor deliberation |
| `vision` | analyze | Image analysis |
| `ask` | prompt, confirm, select | User interaction prompts |

**Traits:**

```rust
pub trait AdvisorDeliberator: Send + Sync {
    fn deliberate<'a>(&'a self, task: &'a str)
        -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
}

pub trait HybridSearcher: Send + Sync {
    fn search<'a>(&'a self, query: &'a str, user_id: &'a str, limit: usize)
        -> Pin<Box<dyn Future<Output = Vec<HybridSearchResult>> + Send + 'a>>;
}
```

### 3.4 EventTool — `"event"`

**Source:** `crates/tools/src/event_tool.rs`

```rust
pub struct EventTool { store: Arc<Store> }
```

| Action | Parameters | Description |
|--------|-----------|-------------|
| `create` | name, cron, task_type, task_data | Schedule recurring task |
| `list` | | List all scheduled tasks |
| `delete` | name | Remove task |
| `pause` | name | Disable task (keeps it) |
| `resume` | name | Re-enable paused task |
| `run` | name | Immediately trigger |
| `history` | name? | Show execution history |

Task types: `bash` (shell command), `agent` (LLM prompt). Cron format: `second minute hour day month weekday`.

### 3.5 SkillTool — `"skill"`

**Source:** `crates/tools/src/skill_tool.rs`

```rust
pub struct SkillTool { loader: Arc<Loader> }
```

| Action | Description |
|--------|-------------|
| `catalog` | List all available skills |
| `help` | Show full content of skill by name |
| `load` | Activate skill for session |
| `unload` | Deactivate skill |
| `create` | Create new skill from YAML |
| `update` | Update existing skill |
| `delete` | Delete user-created skill |

### 3.6 MessageTool — `"message"`

**Source:** `crates/tools/src/message_tool.rs`

```rust
pub struct MessageTool { store: Arc<Store> }
```

| Resource | Action | Description |
|----------|--------|-------------|
| `owner` | notify | Append to companion chat + push notification |
| `notify` | send, alert | System notifications |
| `sms` | conversations, read | SMS management |

### 3.7 DesktopTool — `"desktop"`

**Source:** `crates/tools/src/desktop_tool.rs`

Stateless. macOS-first via AppleScript.

| Resource | Actions | Description |
|----------|---------|-------------|
| `window` | list, focus, minimize, maximize, resize, close | Window management |
| `input` | click, type, press, move | Input simulation (requires approval) |
| `clipboard` | read, write | Clipboard access |
| `notification` | send | Desktop notifications |
| `capture` | screenshot | Screen capture (app or region) |

### 3.8 SettingsTool — `"settings"`

**Source:** `crates/tools/src/settings_tool.rs`

Stateless. macOS-first.

| Resource | Actions | Description |
|----------|---------|-------------|
| `volume` | get, set (0-100) | Audio volume |
| `brightness` | get, set (0-100) | Display brightness |
| `wifi` | status, toggle | WiFi control |
| `bluetooth` | status, toggle | Bluetooth control |
| `battery` | status | Battery info |

### 3.9 SpotlightTool — `"spotlight"`

**Source:** `crates/tools/src/spotlight_tool.rs`

Stateless. OS search index queries.

| Action | Description |
|--------|-------------|
| `search` | Find files by query/pattern |

Platform: macOS uses `mdfind`, Linux uses `plocate` or `find`.

### 3.10 GrepTool (Internal)

**Source:** `crates/tools/src/grep_tool.rs`

```rust
pub struct GrepTool { rg_path: Option<String> }
```

Used by `FileTool::grep`. Tries ripgrep first, falls back to Rust regex. Blocks dangerous root paths (`/`, `/usr`, `/var`, `/etc`, `/System`, `/Library`, `/Applications`, `/bin`, `/sbin`, `/opt`).

---

## 4. Policy & Safeguards

### 4.1 Policy System

**Source:** `crates/tools/src/policy.rs`

```rust
pub enum PolicyLevel {
    Deny,       // Deny all dangerous operations
    Allowlist,  // Only whitelisted commands (default)
    Full,       // Allow all (dangerous!)
}

pub enum AskMode {
    Off,        // Never ask
    OnMiss,     // Ask only for non-whitelisted (default)
    Always,     // Always ask
}

pub struct Policy {
    pub level: PolicyLevel,
    pub ask_mode: AskMode,
    pub allowlist: HashSet<String>,
    pub origin_deny_list: HashMap<Origin, HashSet<String>>,
}
```

**Key methods:**

| Method | Purpose |
|--------|---------|
| `Policy::new()` | Default: Allowlist + OnMiss + SAFE_BINS |
| `Policy::from_config(level, ask_mode, extra)` | Parse config strings |
| `is_denied_for_origin(origin, tool, resource)` | Hard deny check (not bypassable) |
| `requires_approval(cmd)` | Full policy check: level + mode + allowlist |
| `is_allowed(cmd)` | Check exact match, binary name, `binary arg` patterns |
| `add_to_allowlist(pattern)` | Add pattern to allowlist |

**SAFE_BINS** — Commands that never require approval:
```
ls, pwd, cat, head, tail, grep, find, which, type, jq, cut, sort, uniq, wc,
echo, date, env, printenv, git status/log/diff/branch/show, go/node/python --version
```

**Origin-Based Deny List** — Hard deny per origin (cannot be bypassed):

| Origin | Denied Tools |
|--------|-------------|
| Comm, App, Skill | `shell`, `system:shell` |
| User, System | *(no defaults)* |

**`is_dangerous(cmd)`** detects:
- `rm -rf`, `rmdir`, `sudo`, `su`, `chmod 777`, `chown`, `dd`, `mkfs`
- `> /dev/`, `eval`, `exec`, fork bombs
- Piped downloads: `curl|sh`, `wget|bash`

### 4.2 Safeguards (Unconditional)

**Source:** `crates/tools/src/safeguard.rs`

Safeguards are **unconditional** — they cannot be bypassed by any configuration, policy, or user setting.

**Entry point:** `check_safeguard(tool_name, input) -> Option<String>`

Returns `None` if safe, `Some(error_message)` if blocked.

#### File Safeguards (`check_file_safeguard`)

Guards `write` and `edit` actions. Resolves symlinks and canonical paths.

**Protected paths (platform-specific):**

| Platform | Protected Paths |
|----------|----------------|
| macOS | `/System`, `/usr/bin`, `/usr/sbin`, `/usr/lib`, `/bin`, `/sbin`, `/etc` |
| Linux | `/bin`, `/sbin`, `/usr/bin`, `/usr/sbin`, `/usr/lib`, `/boot`, `/etc`, `/proc`, `/sys`, `/dev` |
| Windows | `c:\windows`, `c:\program files`, `c:\program files (x86)` |
| All | `~/.ssh`, `~/.gnupg`, `~/.aws/credentials`, `~/.kube/config`, `~/.docker/config.json` |
| Nebo data | `NEBO_DATA_DIR` env or platform default (macOS: `~/Library/Application Support/Nebo/data`) |

#### Shell Safeguards (`check_shell_safeguard`)

Guards `exec` actions only.

| Pattern | Blocked |
|---------|---------|
| `sudo`, `su` | Always — "never run with elevated privileges" |
| `rm -rf /` or `rm -rf /*` | Root filesystem wipe |
| `dd of=/dev/` | Block device writes |
| `mkfs`, `fdisk`, `gdisk`, `parted`, `wipefs` | Filesystem/partition operations |
| `:(){ :\|:& };:` | Fork bombs |
| `> /dev/*` | Device writes (except `/dev/null`, `/dev/stdout`, `/dev/stderr`) |

### 4.3 Defense Layers Summary

```
Layer 1: SAFEGUARDS (Unconditional)
  └─ Cannot be bypassed — hard-coded dangerous operations & protected paths

Layer 2: ORIGIN DENY LIST
  └─ Comm/App/Skill origins → no shell access

Layer 3: POLICY (Configurable)
  └─ Allowlist-based approval for shell commands

Layer 4: INPUT VALIDATION
  └─ Schema validation, path canonicalization, SSRF protection
```

---

## 5. Process Registry

**Source:** `crates/tools/src/process.rs`

### BackgroundSession

```rust
pub struct BackgroundSession {
    pub id: String,                         // "bg-XXXXXXXX"
    pub pid: u32,
    pub command: String,
    pub exited: bool,
    pub exit_code: Option<i32>,
    output: Arc<Mutex<String>>,             // accumulated output
    pending_stdout: Arc<Mutex<Vec<u8>>>,    // pending stdout buffer
    pending_stderr: Arc<Mutex<Vec<u8>>>,    // pending stderr buffer
    stdin_tx: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    kill_tx: Option<tokio::sync::oneshot::Sender<()>>,
}
```

### ProcessRegistry

```rust
pub struct ProcessRegistry {
    running: Arc<Mutex<HashMap<String, Arc<BackgroundSession>>>>,
    finished: Arc<Mutex<HashMap<String, Arc<BackgroundSession>>>>,
}
```

**Key methods:**

| Method | Purpose |
|--------|---------|
| `spawn_background(command, cwd)` | Spawn async process, return session ID (`bg-XXXXXXXX`) |
| `get_any_session(id)` | Retrieve running or finished session |
| `list_running()` | All running sessions |
| `list_finished()` | All finished sessions |
| `write_stdin(id, data)` | Write to session's stdin |
| `kill_session(id)` | Send kill signal via oneshot channel |

### IO Multiplexing

`handle_process()` is an internal async task that:
1. Reads stdout/stderr in parallel (tokio select)
2. Handles stdin writes via mpsc channel
3. Handles kill signals via oneshot channel
4. Moves session from `running` → `finished` on exit

### Environment Sanitization

`sanitized_env()` filters dangerous variables from subprocess environment:

| Category | Blocked Vars |
|----------|-------------|
| Code injection | `LD_PRELOAD`, `LD_*`, `DYLD_*` |
| Shell manipulation | `IFS`, `CDPATH`, `BASH_ENV`, `ENV`, `PROMPT_COMMAND`, `SHELLOPTS`, `BASHOPTS`, `GLOBIGNORE` |
| Language startup | `PYTHONSTARTUP`, `PYTHONPATH`, `RUBYOPT`, `RUBYLIB`, `PERL5*`, `NODE_OPTIONS` |

### Platform-Specific Shell

```rust
fn shell_command() -> (String, Vec<String>) {
    // Windows: powershell.exe -NoProfile -Command
    // Unix:    bash -c
}
```

---

## 6. Skill System

### 6.1 Skill Struct

**Source:** `crates/tools/src/skills/skill.rs`

```rust
pub struct Skill {
    pub name: String,
    pub description: String,
    pub version: String,                    // default "1.0.0"
    pub author: String,
    pub dependencies: Vec<String>,
    pub tags: Vec<String>,
    pub platform: Vec<String>,             // "macos", "linux", "windows"
    pub triggers: Vec<String>,             // case-insensitive substrings
    pub tools: Vec<String>,                // required tools
    pub priority: i32,                     // higher = match first
    pub max_turns: i32,                    // conversation turns
    pub metadata: HashMap<String, serde_json::Value>,
    pub template: String,                  // markdown body
    pub enabled: bool,
    pub source_path: Option<PathBuf>,
}
```

**Methods:**

| Method | Purpose |
|--------|---------|
| `validate()` | Requires name and description |
| `matches_platform()` | Check if current OS matches (empty = all) |
| `matches_trigger(message)` | Case-insensitive substring matching |

### 6.2 SKILL.md Format

Skills are stored as markdown files with YAML frontmatter:

```markdown
---
name: web-research
description: Deep web research and summarization
version: "1.0.0"
author: nebo
tags: [research, web]
platform: []               # empty = all platforms
triggers:
  - research
  - look up
  - find information
tools:
  - web
  - system
priority: 10
max_turns: 5
enabled: true
---
# Web Research Skill

You are a research assistant. When the user asks you to research a topic:

1. Use `web(action: "search", ...)` to find relevant sources
2. Read the top results with `web(action: "fetch", ...)`
3. Synthesize findings into a clear summary
```

**Parsing:** `parse_skill_md(data)` splits on `---` delimiters, parses YAML frontmatter, and captures the markdown body as the template.

The loader also supports flat `.yaml` / `.yaml.disabled` files for backward compatibility.

### 6.3 Skill Loader

**Source:** `crates/tools/src/skills/loader.rs`

```rust
pub struct Loader {
    skills_dir: PathBuf,                            // User skills directory
    bundled_dir: Option<PathBuf>,                   // Bundled skills directory
    skills: Arc<RwLock<HashMap<String, Skill>>>,
}
```

**Methods:**

| Method | Purpose |
|--------|---------|
| `new(skills_dir, bundled_dir)` | Create loader (doesn't load yet) |
| `load_all()` | Load from bundled dir first, then user dir (overrides by name). Returns count |
| `get(name)` | Retrieve skill by name |
| `list()` | All skills sorted by priority (desc) then name |
| `match_triggers(message, max)` | Return up to `max` matching skills, sorted by priority, enabled only |
| `watch()` | Watch filesystem and hot-reload on changes. Returns `JoinHandle` |

**Loading order:** Bundled skills load first. User skills load second and override bundled skills by name. This allows users to customize bundled skills.

### 6.4 SkillTool Integration

The `SkillTool` wraps the `Loader` as a domain tool:

| Action | Description |
|--------|-------------|
| `catalog` | Returns formatted list of all available skills |
| `help` | Returns full template content of a skill |
| `load` | Activates skill for current session (sets `force_skill` on RunRequest) |
| `unload` | Deactivates skill |
| `create` | Creates new SKILL.md file in user skills directory |
| `update` | Updates existing skill file |
| `delete` | Deletes user-created skill (not bundled) |

---

## 7. .napp Package Format

**Source:** `crates/napp/src/`

### 7.1 Archive Structure

**Source:** `crates/napp/src/napp.rs`

A `.napp` file is a tar archive containing:

| File | Required | Max Size | Description |
|------|----------|----------|-------------|
| `manifest.json` | Yes | 1MB | Package manifest |
| `binary` or `app` | Yes | 500MB | Native executable |
| `signatures.json` | No | 1MB | ED25519 signatures |
| `SKILL.md` / `skill.md` | No | 1MB | Bundled skill |
| `ui/*` | No | 5MB each | Web UI assets |

**`extract_napp(napp_path, dest_dir) -> Result<Manifest, NappError>`**

Security validations during extraction:
1. **Path traversal prevention** — Rejects paths with `..` or leading `/`
2. **Symlink protection** — Rejects `tar::EntryType::Symlink` and `Link`
3. **Size enforcement** — Per-file size limits
4. **Binary format validation** — Only native binaries allowed
5. **File whitelist** — Only known filenames (except `ui/` subdirectory)
6. **Manifest requirement** — Must contain `manifest.json`
7. **Executable permissions** — Sets `0o755` on Unix for binary/app files

### 7.2 Binary Format Validation

```rust
fn validate_binary_format(content: &[u8]) -> Result<(), NappError>;
```

Accepted formats (by magic bytes):

| Format | Magic Bytes |
|--------|-------------|
| ELF | `0x7f 0x45 0x4c 0x46` |
| Mach-O 32-bit | `0xfe 0xed 0xfa 0xce` |
| Mach-O 64-bit | `0xfe 0xed 0xfa 0xcf` |
| Mach-O swapped 32 | `0xce 0xfa 0xed 0xfe` |
| Mach-O swapped 64 | `0xcf 0xfa 0xed 0xfe` |
| Universal Binary | `0xca 0xfe 0xba 0xbe` |
| PE/COFF (Windows) | `0x4d 0x5a` |

Rejected: shebang scripts (`#!`), non-native formats.

### 7.3 Manifest

**Source:** `crates/napp/src/manifest.rs`

```rust
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub runtime: String,                    // default: "local"
    pub protocol: String,                   // default: "grpc"
    pub signature: Option<ManifestSignature>,
    pub startup_timeout: u32,               // seconds, 0-120
    pub provides: Vec<String>,              // capabilities
    pub permissions: Vec<String>,           // required permissions
    pub overrides: Vec<String>,             // hook overrides
    pub oauth: Vec<OAuthRequirement>,
    pub implements: Vec<String>,            // interfaces implemented
}

pub struct ManifestSignature {
    pub algorithm: String,
    pub key_id: String,
}

pub struct OAuthRequirement {
    pub provider: String,
    pub scopes: Vec<String>,
}
```

**Valid capabilities:** `"gateway"`, `"vision"`, `"browser"`, `"comm"`, `"ui"`, `"schedule"`, `"hooks"`, `"tool"`, `"channel"`.

**Valid permission prefixes:**
```
network:, filesystem:, settings:, capability:, memory:, session:, context:,
tool:, shell:, subagent:, lane:, channel:, comm:, notification:, embedding:,
skill:, advisor:, model:, mcp:, database:, storage:, schedule:, voice:,
browser:, oauth:, user:, hook:
```

**Validation rules:**
- `id`, `name`, `version` are required
- Each capability must be in the valid list
- Each permission must start with a valid prefix
- Each override must have a corresponding `hook:` permission
- `startup_timeout` must be ≤ 120 seconds

### 7.4 Napp Registry

**Source:** `crates/napp/src/registry.rs`

```rust
pub struct RegistryConfig {
    pub tools_dir: PathBuf,
    pub neboloop_url: Option<String>,
}

pub struct Registry {
    config: RegistryConfig,
    runtime: Runtime,
    signing: Option<SigningKeyProvider>,
    revocation: Option<RevocationChecker>,
    tools: RwLock<HashMap<String, RegisteredTool>>,
    on_quarantine: RwLock<Option<Box<dyn Fn(QuarantineEvent) + Send + Sync>>>,
}
```

**Key methods:**

| Method | Purpose |
|--------|---------|
| `discover_and_launch()` | Scan tools_dir and launch all valid tools |
| `install_from_url(url)` | Download, extract, verify, and launch |
| `sideload(project_dir)` | Developer mode: symlink, skip signatures |
| `unsideload(tool_id)` | Remove sideloaded tool |
| `uninstall(tool_id)` | Stop and remove tool |
| `list_tools()` | Return `Vec<ToolInfo>` |
| `get_manifest(tool_id)` | Return manifest for tool |
| `handle_install_event(event)` | Process NeboLoop install/uninstall/revoke events |
| `shutdown()` | Stop all tools |

**Launch workflow:**
1. Load and validate manifest
2. Check revocation status (if NeboLoop enabled)
3. Verify signatures (if not sideloaded and `signatures.json` exists)
4. Launch process via runtime
5. Store in registry with capabilities

**Quarantine workflow:**
1. Stop process
2. Remove binary/app files
3. Create `.quarantined` marker file
4. Emit `QuarantineEvent`

### 7.5 Signing & Revocation

**Source:** `crates/napp/src/signing.rs`

```rust
pub struct SigningKeyProvider {
    neboloop_url: String,
    key: RwLock<Option<CachedKey>>,
    ttl: Duration,                          // 24 hours
}

pub struct RevocationChecker {
    neboloop_url: String,
    cache: RwLock<Option<RevocationCache>>,
    ttl: Duration,                          // 1 hour
}
```

**Signing key:** ED25519 public key fetched from `{neboloop_url}/api/v1/apps/signing-key` (base64-encoded). Cached for 24 hours.

**Signature verification (`verify_signatures`):**
1. Load `signatures.json`
2. Verify ED25519 signature of `manifest.json` content
3. Hash binary with SHA256
4. Verify hash matches `binary_hash` in signatures file
5. Verify ED25519 signature of binary

**Revocation checking:** Fetches from `{neboloop_url}/api/v1/apps/revocations`. Returns `{ revoked: ["app-1", ...] }`. Cached for 1 hour. Fails open (returns `Ok(false)` on network error).

### 7.6 Runtime

**Source:** `crates/napp/src/runtime.rs`

```rust
pub struct Process {
    pub tool_id: String,
    pub manifest: Manifest,
    pub pid: u32,
    pub sock_path: PathBuf,
    child: tokio::process::Child,
}

pub struct Runtime {
    _tools_dir: PathBuf,
}
```

**Launch process:**
1. Load and validate manifest
2. Find binary (`binary` or `app` file)
3. Validate binary (format, size via sandbox)
4. Socket path: `{tool_dir}/{tool_id}.sock`
5. Clean up stale socket
6. Create data directory: `{tool_dir}/data`
7. Sanitize environment (see Sandbox)
8. Spawn process with clear environment (allowlist only)
9. On Unix: create new process group (`setpgid`)
10. Write PID file: `{tool_dir}/{tool_id}.sock.pid`
11. Wait for socket (with manifest timeout, default 10s, max 120s)
12. Set socket permissions: `0o600` on Unix
13. Return `Process` with gRPC endpoint (`unix:///path/to/sock`)

**Shutdown:**
- Phase 1: SIGTERM to process group (or `start_kill()` on Windows)
- Phase 2: Wait up to 2 seconds for graceful shutdown
- Phase 3: Force kill if timeout exceeded
- Cleanup: Remove socket and PID files

### 7.7 Sandbox

**Source:** `crates/napp/src/sandbox.rs`

**`sanitize_env()`** builds a clean environment:

| Category | Variables |
|----------|----------|
| Always blocked | `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_API_KEY`, `JWT_SECRET`, `DATABASE_URL`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `GITHUB_TOKEN`, `STRIPE_SECRET_KEY` |
| Allowlisted system | `PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL`, `TZ` |
| Nebo-specific (added) | `NEBO_APP_ID`, `NEBO_APP_NAME`, `NEBO_APP_VERSION`, `NEBO_APP_DIR`, `NEBO_APP_SOCK`, `NEBO_APP_DATA` |

**`validate_binary(path, max_size)`** checks:
1. Not a symlink
2. Must be regular file
3. Size ≤ max_size
4. Executable permissions (Unix: mode & 0o111 != 0)
5. Native binary format (magic bytes validation)
6. Rejects shebang scripts

### 7.8 Supervisor

**Source:** `crates/napp/src/supervisor.rs`

```rust
pub struct Supervisor {
    states: RwLock<HashMap<String, RestartState>>,
    check_interval: Duration,               // 15 seconds
}
```

**Restart policy:**
- Max restarts: 5 per hour
- Window: 1 hour (resets counters)
- Backoff: Exponential (10s → 20s → 40s → 80s → 160s, capped at 5 minutes)
- Next restart allowed only after backoff expires

**Methods:**

| Method | Purpose |
|--------|---------|
| `watch(app_id)` | Start monitoring |
| `unwatch(app_id)` | Stop monitoring |
| `should_restart(app_id)` | Check if restart is allowed |
| `record_restart(app_id)` | Record restart, update backoff |
| `restart_count(app_id)` | Get restart count |
| `check_interval()` | Monitoring interval (15s) |

---

## 8. Hooks System

**Source:** `crates/napp/src/hooks.rs`

### Valid Hook Names

```rust
pub const VALID_HOOKS: &[&str] = &[
    "tool.pre_execute",
    "tool.post_execute",
    "message.pre_send",
    "message.post_receive",
    "memory.pre_store",
    "memory.pre_recall",
    "session.message_append",
    "prompt.system_sections",
    "steering.generate",
    "response.stream",
];
```

### Hook Types

```rust
pub enum HookType {
    Action,     // Fire-and-forget, results discarded
    Filter,     // Chain payload through, can modify or handle
}
```

### HookCaller Trait

```rust
#[async_trait::async_trait]
pub trait HookCaller: Send + Sync {
    async fn call_filter(&self, hook: &str, payload: Vec<u8>)
        -> Result<(Vec<u8>, bool), String>;
    async fn call_action(&self, hook: &str, payload: Vec<u8>)
        -> Result<(), String>;
}
```

### HookDispatcher

```rust
pub struct HookDispatcher {
    hooks: RwLock<HashMap<String, Vec<HookSubscription>>>,
    timeout: Duration,          // 500ms default
    max_failures: u32,          // 3 failures to disable
}
```

**Key methods:**

| Method | Purpose |
|--------|---------|
| `register(hook_name, app_id, hook_type, priority, caller)` | Register hook subscription |
| `unregister_app(app_id)` | Remove all hooks for an app |
| `apply_filter(hook_name, payload)` | Chain payload through filter subscribers |
| `do_action(hook_name, payload)` | Fire action subscribers concurrently |
| `has_subscribers(hook_name)` | Check if any hooks registered |
| `record_failure(hook_name, app_id)` | Increment failure counter |
| `record_success(hook_name, app_id)` | Reset failure counter |

### Filter Execution

1. Iterate subscribers in priority order (lower priority first)
2. Chain payload through each subscriber
3. If any returns `handled=true`, stop chain and return immediately
4. On error, preserve current payload and continue
5. Timeout per subscriber: 500ms

### Action Execution

1. Fire each subscriber concurrently
2. Errors logged but not propagated

### Circuit Breaker

- Track consecutive failures per app-hook pair
- Disable subscription after 3 consecutive failures
- Success resets failure counter
- Hooks remain disabled until manually re-registered

---

## 9. ROLE/WORK/TOOL/SKILL Hierarchy

### 9.1 Hierarchy Overview

```
ROLE → WORK → TOOL → SKILL
(job)   (procedure) (capability) (knowledge)
```

Each layer auto-installs its dependencies downward:
- Installing a **ROLE** installs its referenced WORKs, TOOLs, and SKILLs
- Installing a **WORK** installs its referenced TOOLs and SKILLs
- Installing a **TOOL** installs as a .napp with bundled SKILLs

### 9.2 Code Formats

```
SKILL-XXXX-XXXX-XXXX — Knowledge artifact (markdown)
TOOL-XXXX-XXXX-XXXX  — Executable capability (.napp)
WORK-XXXX-XXXX-XXXX  — Procedure (workflow.json)
ROLE-XXXX-XXXX-XXXX  — Marketplace bundle (ROLE.md)
NEBO-XXXX-XXXX-XXXX  — Agent instance (account linking)
LOOP-XXXX-XXXX-XXXX  — Community (channel linking)
```

Format pattern: `PREFIX-AAAA-BBBB-CCCC` (prefix + 3 groups of 4 characters).

`CodeType` detection and `handle_code()` intercept codes in chat messages and trigger the appropriate installation flow.

### 9.3 RoleDef

**Source:** `crates/napp/src/role.rs`

```rust
pub struct RoleDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub workflows: Vec<String>,     // WORK-* codes
    pub tools: Vec<String>,         // TOOL-* codes
    pub skills: Vec<String>,        // SKILL-* codes
    pub pricing: Option<RolePricing>,
    pub body: String,               // Markdown body
}

pub struct RolePricing {
    pub model: String,              // "monthly_fixed", "per_turn"
    pub cost: f64,
}
```

**ROLE.md Format:**

```yaml
---
id: sales-sdr
name: Sales SDR
description: Outbound sales development representative
workflows:
  - WORK-lead-qualification
tools:
  - TOOL-crm-lookup
skills:
  - SKILL-sales-qualification
pricing:
  model: monthly_fixed
  cost: 47.0
---
# Sales SDR Role

Markdown body describing behavior...
```

**Validation:**
- `id` and `name` are required
- Skill codes must start with `SKILL-`
- Tool codes must start with `TOOL-`
- Workflow codes must start with `WORK-`

### 9.4 WorkflowDef

**Source:** `crates/workflow/src/parser.rs`

```rust
pub struct WorkflowDef {
    pub version: String,                    // "1.0"
    pub id: String,
    pub name: String,
    pub triggers: Vec<Trigger>,             // Event/schedule/manual
    pub inputs: HashMap<String, InputParam>,
    pub activities: Vec<Activity>,
    pub dependencies: Dependencies,
    pub budget: Budget,
}

pub enum Trigger {
    Event { event: String },
    Schedule { cron: String },
    Manual,
}

pub struct Activity {
    pub id: String,
    pub intent: String,                     // What this activity does
    pub skills: Vec<String>,                // SKILL codes
    pub tools: Vec<ToolRef>,                // Tool references
    pub model: String,                      // "haiku", "sonnet", "opus"
    pub steps: Vec<String>,                 // Natural language instructions
    pub token_budget: TokenBudget,          // default max: 4096
    pub on_error: OnError,                  // retry + fallback policy
}

pub enum ToolRef {
    Code(String),                           // "TOOL-A1B2-C3D4-E5F6"
    Interface { interface: String },        // { "interface": "crm-lookup" }
    Pinned { code: String },                // { "code": "TOOL-..." }
}

pub enum Fallback {
    NotifyOwner,                            // Notify owner of failure
    Skip,                                   // Skip to next activity
    Abort,                                  // Abort entire workflow
}
```

**Validation:** ID not empty, name not empty, at least one activity, unique activity IDs, budget sum check.

### 9.5 Database Tables

```sql
-- Installed workflows
CREATE TABLE workflows (
    id TEXT PRIMARY KEY,
    code TEXT UNIQUE,                        -- "WORK-A1B2-C3D4-E5F6"
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    definition TEXT NOT NULL,                -- workflow.json content
    skill_md TEXT,
    manifest TEXT,
    installed_at TEXT DEFAULT (datetime('now')),
    enabled INTEGER DEFAULT 1
);

-- Tool interface bindings (user's choices)
CREATE TABLE workflow_tool_bindings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workflow_id TEXT NOT NULL REFERENCES workflows(id),
    interface_name TEXT NOT NULL,
    tool_code TEXT NOT NULL,
    UNIQUE(workflow_id, interface_name)
);

-- Workflow run history
CREATE TABLE workflow_runs (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id),
    trigger_type TEXT NOT NULL,
    trigger_detail TEXT,
    status TEXT NOT NULL,                    -- "running", "completed", "failed"
    inputs TEXT,
    current_activity TEXT,
    total_tokens_used INTEGER DEFAULT 0,
    total_cost_estimate TEXT,
    error TEXT,
    error_activity TEXT,
    session_key TEXT,
    started_at TEXT DEFAULT (datetime('now')),
    completed_at TEXT
);

-- Per-activity results within a run
CREATE TABLE workflow_activity_results (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES workflow_runs(id),
    activity_id TEXT NOT NULL,
    status TEXT NOT NULL,
    tokens_used INTEGER DEFAULT 0,
    attempts INTEGER DEFAULT 1,
    error TEXT,
    started_at TEXT NOT NULL,
    completed_at TEXT NOT NULL
);

-- Installed roles (marketplace bundles)
CREATE TABLE roles (
    id TEXT PRIMARY KEY,
    code TEXT UNIQUE,
    name TEXT NOT NULL,
    role_md TEXT NOT NULL,
    installed_at TEXT DEFAULT (datetime('now'))
);
```

### 9.6 Workflow Engine

**Source:** `crates/workflow/src/engine.rs`

```rust
pub async fn execute_workflow(
    def: &WorkflowDef,
    inputs: serde_json::Value,
    trigger_type: &str,
    trigger_detail: Option<&str>,
    store: &Arc<Store>,
    provider: &dyn ai::Provider,
    resolved_tools: &[Box<dyn DynTool>],
) -> Result<String, WorkflowError>
```

**Execution flow:**
1. Create workflow run record with UUID
2. Create shared session key: `"workflow-{workflow_id}-{run_id}"`
3. Sequentially execute each activity
4. Filter tools per activity (only declared tools available)
5. Accumulate prior context between activities
6. Track total tokens against `budget.total_per_run`
7. Apply `on_error.fallback` policy on failure
8. Return run ID on success

**Activity execution** is a lean agentic loop (no steering, no memory, no personality):
- Build prompt from `intent + steps + prior_context + inputs`
- Only declared tools available
- Hard token ceiling: `token_budget.max` (default 4096)
- Max 20 iterations per activity
- Retry logic via `on_error.retry` (default 1 attempt)

### 9.7 Trigger Registration

**Source:** `crates/workflow/src/triggers.rs`

```rust
pub fn register_triggers(def: &WorkflowDef, store: &Store);
pub fn unregister_triggers(workflow_id: &str, store: &Store);
```

- `Schedule { cron }` → `store.upsert_cron_job()` with `task_type = "workflow"`
- `Event { event }` → logged as stub (events system not yet ported)
- `Manual` → no registration needed

---

## 10. Agent Integration

### 10.1 Runner

**Source:** `crates/agent/src/runner.rs`

```rust
pub struct Runner {
    sessions: SessionManager,
    providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    tools: Arc<Registry>,
    store: Arc<Store>,
    selector: Arc<ModelSelector>,
    _steering: steering::Pipeline,
    concurrency: Arc<ConcurrencyController>,
    hooks: Arc<napp::HookDispatcher>,
    mcp_context: Option<Arc<tokio::sync::Mutex<ToolContext>>>,
}
```

**`RunRequest`:**
```rust
pub struct RunRequest {
    pub session_key: String,
    pub prompt: String,
    pub system: String,                     // custom system prompt (empty = modular default)
    pub model_override: String,
    pub user_id: String,
    pub skip_memory_extract: bool,          // skip for sub-agents
    pub origin: Origin,
    pub channel: String,                    // web, dm, cli, voice
    pub force_skill: String,                // force a specific skill
    pub max_iterations: usize,              // 0 = default 100
    pub cancel_token: CancellationToken,
}
```

**Key methods:**

| Method | Purpose |
|--------|---------|
| `run(req)` | Spawn agentic loop, return `mpsc::Receiver<StreamEvent>` |
| `chat(prompt)` | One-shot chat (no tools) |
| `reload_providers(providers)` | Hot-reload LLM providers |

### 10.2 Agentic Loop

**Constants:**

| Constant | Value |
|----------|-------|
| `DEFAULT_MAX_ITERATIONS` | 100 |
| `DEFAULT_CONTEXT_TOKEN_LIMIT` | 80,000 |
| `MAX_TRANSIENT_RETRIES` | 10 |
| `MAX_RETRYABLE_RETRIES` | 5 |
| `TOOL_EXECUTION_TIMEOUT` | 300s |
| `MAX_AUTO_CONTINUATIONS` | 3 |

**Main loop flow:**

```
1. Load DB context (agent profile, user profile, personality, scored memories)
2. Build static system prompt (reused across iterations)
3. Detect objective in background (non-blocking)

For each iteration (1..max_iterations):
    a. Check cancellation token
    b. Load messages from session
    c. Apply sliding window pruning + micro-compaction
    d. Run steering pipeline (12 generators)
    e. Inject steering messages into conversation
    f. Filter tools based on context (core + keyword-matched contextual)
    g. Estimate tokens, validate context thresholds
    h. Build messages for LLM (system + messages + steering)
    i. Select best model (task classification + fallback chain)
    j. Stream LLM response (Text, ToolCall, Error, Done events)
    k. For each tool call:
       - Validate & execute with 300s timeout
       - Store result in session
       - Handle retry logic (transient, retryable)
    l. Check for auto-continuation (if agent stops prematurely)
    m. Store iteration progress
    n. Loop back or break

4. Post-run:
    - Extract facts from conversation (async, unless skip_memory_extract)
    - Embed extracted facts (async)
    - Return final stream event (Done or Error)
```

### 10.3 Tool Filtering

**Source:** `crates/agent/src/tool_filter.rs`

**Core tools** (always included):
```rust
["system", "web", "bot", "loop", "event", "message", "skill"]
```

**Contextual groups** (included by keyword matching in last 5 messages):

| Group | Keywords |
|-------|----------|
| `screenshot` | screenshot, screen, capture, visible, see what |
| `vision` | image, photo, picture, screenshot, visual |
| `desktop` | click, type, mouse, keyboard, window, app, open |
| `organizer` | calendar, reminder, contact, email, schedule |

Logic: Include group if keyword matches OR any tool in group was recently called. Falls back to all tools if result is empty.

### 10.4 Steering Pipeline

**Source:** `crates/agent/src/steering.rs`

The steering pipeline generates ephemeral messages injected into the conversation to guide agent behavior. These messages are never persisted and not shown to the user.

```rust
pub struct SteeringMessage {
    pub content: String,
    pub position: Position,     // End or AfterUser
}

pub enum Position {
    End,                        // Append at end of messages
    AfterUser,                  // Insert after last user message
}
```

**12 Generators (in priority order):**

| # | Generator | Trigger | Purpose |
|---|-----------|---------|---------|
| 1 | IdentityGuard | Every 8 turns | Remind agent of identity |
| 2 | ChannelAdapter | Always | Adjust style for dm/cli/voice channels |
| 3 | ToolNudge | 5+ turns, no tool use, has active task | Nudge to use tools |
| 4 | DateTimeRefresh | Every 5 iterations | Update current time |
| 5 | MemoryNudge | 10+ turns, user shares personal info | Suggest memory storage |
| 6 | TaskParameterNudge | 2-5 turns, mentions dates/amounts | Store as parameters |
| 7 | ObjectiveTaskNudge | Has active task, no work tasks | Immediately start action |
| 8 | PendingTaskAction | Active task, iteration 2+, last response text-only | Force tool call |
| 9 | TaskProgress | Every 4 iterations from iteration 4 | Show task state |
| 10 | ActiveObjectiveReminder | Every iteration (except 4-intervals) | Remind of objective |
| 11 | LoopDetector | 4+ consecutive same tool calls | Warn about loop; 6+: hard stop |
| 12 | JanusQuotaWarning | Cost alert present | Emphasize cost-consciousness |

### 10.5 Memory System

**Source:** `crates/agent/src/memory.rs`

**Key functions:**

| Function | Purpose |
|----------|---------|
| `extract_facts(provider, messages)` | LLM-powered fact extraction from conversation |
| `store_facts(store, facts, user_id)` | Persist facts; styles go through reinforcement |
| `decay_score(access_count, accessed_at)` | Score = access_count x 0.7^(days/30) |
| `score_memory(mem)` | Combines confidence x decay |
| `load_memory_context(store, user_id)` | Load top scored memories, formatted into sections |
| `load_scored_memories(store, user_id, limit)` | Ranked memories for prompt assembly |
| `embed_memories_async(store, provider, entries, user_id)` | Background chunking & embedding |

**Memory layers:**

| Layer | Namespace | Description |
|-------|-----------|-------------|
| `tacit` | `personality` | Learned style observations (reinforced on repeat extraction) |
| `tacit` | `preferences` | User preferences and behaviors |
| `tacit` | `artifacts` | Content produced for user |
| `entity` | `default` | People, places, things |
| `daily` | `{YYYY-MM-DD}` | Today's decisions and context |

**Confidence resolution:**
- Explicit facts: 0.9
- Inferred facts: 0.6
- Raw value fallback: clamped 0.0-1.0

**Scoring:** Two-pass overfetch — personality (cap 10), others (cap 30).

### 10.6 Sub-Agent Orchestrator

**Source:** `crates/tools/src/orchestrator.rs`, `crates/agent/src/orchestrator.rs`

```rust
pub struct SpawnRequest {
    pub prompt: String,
    pub description: String,
    pub agent_type: String,             // "explore", "plan", "general"
    pub model_override: String,
    pub parent_session_id: String,
    pub parent_session_key: String,
    pub user_id: String,
    pub wait: bool,
}

pub struct SpawnResult {
    pub task_id: String,
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

pub trait SubAgentOrchestrator: Send + Sync {
    fn spawn(&self, req: SpawnRequest)
        -> Pin<Box<dyn Future<Output = Result<SpawnResult, String>> + Send + '_>>;
    fn execute_dag(&self, prompt: &str, user_id: &str, parent_session_id: &str)
        -> Pin<Box<dyn Future<Output = Result<SpawnResult, String>> + Send + '_>>;
    fn cancel(&self, task_id: &str)
        -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;
    fn status(&self, task_id: &str)
        -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>>;
    fn list_active(&self)
        -> Pin<Box<dyn Future<Output = Vec<(String, String, String)>> + Send + '_>>;
    fn recover(&self)
        -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

pub type OrchestratorHandle = Arc<OnceLock<Box<dyn SubAgentOrchestrator>>>;
```

The `OrchestratorHandle` is a late-binding handle (created empty, filled after Runner is built) to break circular dependency between runner and orchestrator.

**DAG execution (`execute_dag`):**
1. Decompose task into `TaskNode` list via LLM (`decompose_task`)
2. Single-task optimization: run directly as sub-agent
3. Build and validate DAG (cycle detection via Kahn's algorithm)
4. Reactive scheduling loop:
   - Collect ready tasks (all dependencies completed)
   - Spawn in parallel with LLM concurrency permits
   - Reactive select: wait for ANY task to finish
   - Update graph, find new ready tasks
5. Synthesize final output from leaf nodes

**Task decomposition** (`decompose.rs`):
- Max 10 sub-tasks per decomposition
- Agent types: `Explore` (read-only), `Plan` (analysis), `General` (full tools)
- Handles markdown/JSON parsing of LLM response
- Returns `Vec<TaskNode>` with dependency graph

### 10.7 Advisors

**Source:** `crates/agent/src/advisors/`

```rust
pub struct Advisor {
    pub name: String,
    pub role: String,
    pub description: String,
    pub priority: i32,
    pub enabled: bool,
    pub memory_access: bool,
    pub timeout_seconds: i32,
    pub persona: String,                    // Markdown body from ADVISOR.md
    pub source_path: Option<PathBuf>,
}

pub struct Response {
    pub advisor_name: String,
    pub role: String,
    pub critique: String,
    pub confidence: i32,                    // 1-10
    pub risks: String,
    pub suggestion: String,
}
```

Advisors are loaded from `ADVISOR.md` files (YAML frontmatter + markdown persona). The `AdvisorDeliberator` trait enables multi-turn deliberation via the `bot(resource: "advisors", action: "deliberate")` tool.

### 10.8 Context Pruning

**Source:** `crates/agent/src/pruning.rs`, `crates/agent/src/compaction.rs`

| Strategy | Trigger | Mechanism |
|----------|---------|-----------|
| Sliding window | Always | Max 20 messages or 40k tokens (never evicts current-run messages) |
| Micro-compact | Context pressure | Trim old tool results (web, file, shell, system) |
| Full compaction | Evicted messages exceed threshold | LLM-based summarization |
| Fallback summary | First eviction | Quick plaintext summary (no LLM call) |

### 10.9 Model Selection

**Source:** `crates/agent/src/selector.rs`

```rust
pub enum TaskType {
    Vision,
    Audio,
    Reasoning,
    Code,
    General,
}
```

Key methods:
- `select(messages)` — Select best model for conversation
- `classify_task(messages)` — Detect task type from messages
- `mark_failed(model_id)` — Exponential backoff cooldown (5s → 3600s max)
- `resolve_fuzzy(input)` — e.g. `"sonnet"` → `"anthropic/claude-sonnet-4"`

### 10.10 Concurrency Control

**Source:** `crates/agent/src/concurrency.rs`

```rust
pub struct ConcurrencyController {
    // LLM Semaphore: Dynamic permits (2-20 by CPU cores)
    // Tool Semaphore: Fixed 8 permits per iteration
    // Backpressure flag: set by 429 rate limits
}
```

| Method | Purpose |
|--------|---------|
| `acquire_llm_permit()` | Block until permit available |
| `acquire_tool_permit()` | Tool-level concurrency |
| `report_success(rate_limit_meta)` | Clear backpressure, release held permits |
| `report_rate_limit()` | Set backpressure, hold half permits |
| `set_ceiling(new_ceiling)` | Adjust max concurrency dynamically |

### 10.11 System Prompt Structure

**Source:** `crates/agent/src/prompt.rs`

The system prompt is composed of sections:

1. Memory/DB context (scored memories, user profile)
2. SECTION_IDENTITY — Agent is personal companion
3. SECTION_CAPABILITIES — Use tools immediately
4. SECTION_TOOLS_DECLARATION — Tool definitions
5. SECTION_COMM_STYLE — Communication guidelines
6. STRAP tool documentation (platform-specific)
7. SECTION_MEDIA — Inline images & video
8. SECTION_MEMORY_DOCS — Memory system explanation
9. SECTION_TOOL_GUIDE — Route every request to tools
10. SECTION_BEHAVIOR — Context, safety, code reuse
11. SECTION_SYSTEM_ETIQUETTE — Cleanup, focus management

---

## 11. Cross-Reference to Go Docs

### Mapping to Existing SME Docs

| This Section | Existing Go SME Doc | Notes |
|--------------|--------------------|----|
| Tool Registry | [agent-tools.md](agent-tools.md) | Go uses `Registry` struct similarly. Rust adds `DynTool` trait for type-erased dispatch |
| STRAP Domain | [platform-tools.md](platform-tools.md) | STRAP pattern is consistent between Go and Rust |
| Built-in Tools | [platform-tools.md](platform-tools.md) | Same 11 tools. Rust adds browser extension automation |
| Policy & Safeguards | [agent-tools.md](agent-tools.md) | Same defense layers. Rust uses `Arc<RwLock<Policy>>` for thread safety |
| Skill System | [platform-taxonomy.md](platform-taxonomy.md) | Same SKILL.md format. Rust adds hot-reload via `watch()` |
| .napp Package | [plugins-store-oauth-dev.md](plugins-store-oauth-dev.md) | Same archive format. Rust uses tar instead of custom format |
| Hooks | [agent-tools.md](agent-tools.md) | Same 10 hooks. Rust adds circuit breaker (3 failures) |
| Hierarchy | [platform-taxonomy.md](platform-taxonomy.md) | Same ROLE/WORK/TOOL/SKILL hierarchy |
| Agent Loop | [agent-core.md](agent-core.md) | Same agentic loop structure. Rust adds micro-compaction |
| Steering | [agent-core.md](agent-core.md) | Same 12 generators. Rust uses panic recovery per generator |
| Memory | [embeddings-and-memory.md](embeddings-and-memory.md) | Same decay formula. Rust adds two-pass overfetch |
| Orchestrator | [orchestrator-and-recovery.md](orchestrator-and-recovery.md) | Same DAG execution. Rust uses `FuturesUnordered` for reactive scheduling |
| Workflows | [workflow-engine.md](workflow-engine.md) | Canonical spec. Rust implementation matches spec |
| Concurrency | [concurrency-patterns.md](concurrency-patterns.md), [lanes-and-hub.md](lanes-and-hub.md) | Rust uses semaphore-based backpressure instead of Go channels |

### Notable Differences (Go vs Rust)

| Area | Go | Rust |
|------|----|------|
| Tool dispatch | Interface embedding | `DynTool` trait + `Pin<Box<dyn Future>>` |
| Concurrency | Goroutines + channels | Tokio tasks + semaphores + mpsc |
| Memory safety | Runtime panics | Compile-time `Arc<RwLock<>>` |
| MCP bridge | gRPC direct | `mcp::Bridge` with prefix stripping |
| Config hot-reload | Signal-based | `Arc<RwLock<>>` provider reload |
| Orchestrator binding | Direct reference | `OnceLock` late-binding handle |
| Process groups | `syscall.SysProcAttr` | `command_ext::CommandExt::process_group(0)` |
| Binary validation | Optional | Mandatory (magic bytes check) |

---

## Appendix: Key Constants

| Constant | Value | Source |
|----------|-------|--------|
| Shell timeout (default) | 120s | `shell_tool.rs` |
| Tool execution timeout | 300s | `runner.rs` |
| HTTP timeout | 30s | `web_tool.rs` |
| File read limit | 2,000 lines | `file_tool.rs` |
| Glob limit | 1,000 files | `file_tool.rs` |
| Grep limit | 100 matches | `file_tool.rs` |
| Max output (shell) | 50,000 chars | `shell_tool.rs` |
| Session ID format | `bg-XXXXXXXX` | `process.rs` |
| Max iterations | 100 | `runner.rs` |
| Context token limit | 80,000 | `runner.rs` |
| Sliding window | 20 messages / 40k tokens | `pruning.rs` |
| Memory confidence threshold | 0.65 | `memory.rs` |
| Max conversation chars (extraction) | 15,000 | `memory.rs` |
| Hook timeout | 500ms | `hooks.rs` |
| Hook circuit breaker | 3 failures | `hooks.rs` |
| Supervisor max restarts | 5/hour | `supervisor.rs` |
| Supervisor backoff cap | 5 minutes | `supervisor.rs` |
| Max binary size (.napp) | 500MB | `napp.rs` |
| Max UI file size (.napp) | 5MB | `napp.rs` |
| Signing key cache TTL | 24 hours | `signing.rs` |
| Revocation cache TTL | 1 hour | `signing.rs` |
| Graceful shutdown timeout | 2s | `runtime.rs` |
| Socket wait timeout | 10s default, 120s max | `runtime.rs` |
| Max auto-continuations | 3 | `runner.rs` |
| Max DAG sub-tasks | 10 | `decompose.rs` |
| Workflow max iterations/activity | 20 | `engine.rs` |
| Activity token budget default | 4,096 | `parser.rs` |
