# Tools System — Internal Reference

The tools system is Nebo's entire capability surface: every action the agent can take in the world (read files, run commands, browse the web, send messages, manage memory, automate desktops) is a registered tool. This document is the SME-level reference — read it to become immediately expert.

**No public documentation exists.** Everything below is derived from source code in `internal/agent/tools/`.

---

## Why STRAP Exists

LLMs perform worse when presented with 35+ individual tool definitions. STRAP (**S**ingle **T**ool **R**esource **A**ction **P**attern) consolidates them into 4 domain tools with resource+action routing. The result: ~80% reduction in context window overhead, better tool comprehension, and a consistent `tool(resource: X, action: Y, ...)` invocation pattern.

---

## Architecture Overview

```
Registry (registry.go)
  ├── Domain Tools (STRAP)
  │   ├── file   → read, write, edit, glob, grep
  │   ├── shell  → bash/exec, process/list|kill|info, session/list|poll|log|write|kill
  │   ├── web    → http/fetch, search/query, browser/18 actions
  │   └── agent  → task, reminder, memory, message, session, comm, profile
  │
  ├── Standalone Tools
  │   ├── screenshot  → capture, see (annotated UI snapshots)
  │   ├── vision      → AI image analysis (provider-agnostic)
  │   ├── advisors    → internal deliberation with personas
  │   ├── skill       → unified skill/app interface
  │   ├── store       → NeboLoop app store browsing/install
  │   └── tts         → text-to-speech (platform capability)
  │
  ├── Platform Capabilities (20+ tools, auto-registered via init())
  │   ├── darwin:  accessibility, app, calendar, clipboard, contacts, desktop,
  │   │           dialog, dock, keychain, mail, menubar, music, notification,
  │   │           peekaboo, reminders, shortcuts, spaces, spotlight, system,
  │   │           tts, window
  │   ├── linux:   accessibility, app, calendar, clipboard, contacts, desktop,
  │   │           keychain, mail, music, notification, reminders, shortcuts,
  │   │           spotlight, system, tts, window
  │   └── windows: accessibility, app, calendar, clipboard, contacts, desktop,
  │               keychain, mail, music, notification, reminders, shortcuts,
  │               spotlight, system, tts, window
  │
  └── Security Layers
      ├── Safeguard  (safeguard.go) — unconditional, cannot be bypassed
      ├── Policy     (policy.go)    — configurable approval flow
      └── Origin     (origin.go)    — per-origin tool restrictions
```

---

## Interfaces

### Tool (registry.go)

```go
type Tool interface {
    Name() string
    Description() string
    Schema() json.RawMessage
    Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error)
    RequiresApproval() bool
}

type ToolResult struct {
    Content  string `json:"content"`
    IsError  bool   `json:"is_error,omitempty"`
    ImageURL string `json:"image_url,omitempty"`
}
```

### DomainTool (domain.go)

Extends Tool with STRAP metadata:

```go
type DomainTool interface {
    Tool
    Domain() string
    Resources() []string
    ActionsFor(resource string) []string
}
```

### Capability (capabilities.go)

Wraps a Tool with platform metadata:

```go
type Capability struct {
    Tool          Tool
    Platforms     []string  // "darwin", "linux", "windows", "ios", "all"
    Category      string    // "system", "media", "productivity", "desktop"
    RequiresSetup bool
}
```

---

## File Map

| File | Purpose |
|------|---------|
| **Core Infrastructure** | |
| `domain.go` | DomainTool interface, `BuildDomainSchema()`, `BuildDomainDescription()`, `ValidateResourceAction()`, `ActionRequiresApproval()` |
| `registry.go` | Tool/ToolResult interfaces, Registry (register/execute/list), MCP prefix stripping, `toolCorrection()` for hallucinated names, `OnChange` listeners |
| `policy.go` | PolicyLevel (deny/allowlist/full), AskMode (off/on-miss/always), SafeBins, approval flow (callback or stdin), `AutonomousCheck` |
| `origin.go` | Origin type (user/comm/app/skill/system), context propagation via `WithOrigin`/`GetOrigin`, session key via `WithSessionKey`/`GetSessionKey` |
| `capabilities.go` | CapabilityRegistry, platform detection (`isIOS` build tag), `RegisterCapability()`, `categoryToPermission` mapping |
| `safeguard.go` | Unconditional hard safety limits — file path protection, shell command blocking, symlink resolution |
| **STRAP Domain Tools** | |
| `file_tool.go` | FileTool: read/write/edit/glob/grep, sensitive path blocking, `OnFileRead` callback, symlink traversal prevention |
| `shell_tool.go` | ShellTool: bash exec (foreground + background), process management, session management, `sanitizedEnv()` for env var sanitization |
| `web_tool.go` | WebDomainTool: HTTP fetch (SSRF-protected), search (DuckDuckGo/Google/native browser), browser automation (3 profiles: native/nebo/chrome), 18 browser actions |
| `agent_tool.go` | AgentDomainTool: task/reminder/memory/message/session/comm/profile, resource aliases, action-to-resource inference, late-bound dependencies |
| **Standalone Tools** | |
| `screenshot.go` | ScreenshotTool: `capture` (full screen) and `see` (annotated window snapshot with element IDs) |
| `vision.go` | VisionTool: provider-agnostic image analysis via `AnalyzeFunc` callback |
| `advisors_tool.go` | AdvisorsTool: concurrent advisor deliberation (up to 5, 30s timeout), confidence scoring |
| `skill_tool.go` | SkillDomainTool: unified skill/app catalog, invocation tracking, TTL-based expiry, trigger matching, token budgeting |
| `neboloop_tool.go` | NeboLoopTool (name: `store`): app/skill store browsing, install/uninstall, `NeboLoopClientProvider` callback |
| `tts.go` | TTS platform capability tool |
| **Snapshot/Vision Pipeline** | |
| `snapshot_store.go` | In-memory singleton `SnapshotStore` with 1h TTL, `LookupElement()` for desktop tool interaction |
| `snapshot_annotator.go` | `AssignElementIDs()`: flatten tree, filter actionable, role-prefix IDs (B=button, T=textfield, L=link, etc.), sort by screen position |
| `snapshot_renderer.go` | Colored annotation overlays on screenshots |
| `snapshot_capture_darwin.go` | macOS window capture via CGWindowListCreateImage |
| `snapshot_capture_linux.go` | Linux window capture |
| `snapshot_capture_windows.go` | Windows window capture |
| `snapshot_accessibility_darwin.go` | macOS accessibility tree extraction |
| `snapshot_accessibility_linux.go` | Linux accessibility tree extraction |
| `snapshot_accessibility_windows.go` | Windows accessibility tree extraction |
| **Supporting Files** | |
| `scheduler.go` | Scheduler interface (Create/Get/List/Update/Delete/Enable/Disable/Trigger/History) |
| `scheduler_manager.go` | SchedulerManager: delegates to app-provided or built-in CronScheduler |
| `cron.go` | CronTool: robfig/cron + SQLite persistence, `AgentTaskCallback` |
| `memory.go` | MemoryTool: 3-tier (tacit/daily/entity), hybrid vector+FTS search, namespace isolation |
| `process_registry.go` | Background process tracking, stdin/stdout pipes, output draining, cleanup sweeper |
| `web_sanitize.go` | `ExtractVisibleText()`, `FormatFetchResult()`, `ChunkText()` for paginated output |
| `grep.go` | GrepTool: ripgrep preferred, fallback to Go regex, used by file.grep |
| `shell_unix.go` / `shell_windows.go` | Platform-specific shell command construction |
| `process_signal_unix.go` / `process_signal_windows.go` | Platform-specific signal handling |
| **Legacy (superseded by STRAP, still present)** | |
| `bash.go`, `bash_sessions.go`, `read.go`, `write.go`, `edit.go`, `glob.go`, `search.go`, `web.go`, `browser.go`, `task.go`, `sessions.go`, `message.go`, `process.go`, `channel_send.go` | |

---

## Execution Flow

When `Registry.Execute(ctx, toolCall)` is called:

```
1. MCP Prefix: "mcp__nebo-agent__web" → check if exists as-is (external MCP proxy),
               else strip to "web" (Nebo's own tool exposed via MCP)

2. Lookup: Find tool in map. If not found → return error with:
   - Specific correction hint (e.g., "bash" → "INSTEAD USE: shell(resource: bash, action: exec, ...)")
   - Full list of available tools

3. Origin Deny: policy.IsDeniedForOrigin(origin, toolName)
   - Hard deny, no approval prompt possible
   - Currently disabled (returns nil) — architecture ready for when permission model matures

4. Approval: if tool.RequiresApproval() && policy != nil:
   a. OriginSystem → auto-approve (no one to ask)
   b. AutonomousCheck() → auto-approve
   c. PolicyFull → auto-approve
   d. Bash allowlist check: exact match, binary name, binary+first-arg
   e. ApprovalCallback (web UI) or stdin prompt (CLI)
   f. CLI "a" response → add to allowlist for session

5. Safeguard: CheckSafeguard(toolName, input)
   - UNCONDITIONAL — runs even if approval was granted
   - Cannot be bypassed by any setting
   - Blocks: sudo, su, rm -rf /, dd to devices, fork bombs, all disk formatting commands,
     writes to system paths, writes to sensitive user paths (.ssh, .gnupg, .aws, .kube),
     writes to Nebo's own data directory
   - Resolves symlinks to prevent indirection attacks

6. Execute: tool.Execute(ctx, input) → *ToolResult
```

---

## Domain Tool Details

### `file` Tool (file_tool.go)

**Resources:** flat (no resource field)
**RequiresApproval:** true (checked per-action — read/glob/grep are safe)

| Action | Required Fields | Optional Fields | Notes |
|--------|----------------|-----------------|-------|
| `read` | path | offset (1-based), limit (default 2000) | 1MB line buffer, 2000 char line truncation, `OnFileRead` callback for access tracking |
| `write` | path, content | append | Creates parent dirs, validates sensitive paths |
| `edit` | path, old_string | new_string, replace_all | Exact match, blocks if ambiguous (count > 1 without replace_all) |
| `glob` | pattern | path (default "."), limit (default 1000) | Supports `**`, skips `.dirs`, `node_modules`, `vendor`, `__pycache__`, sorts by mtime newest-first |
| `grep` | regex | path (default "."), glob, case_insensitive, context, limit (default 100) | Delegates to GrepTool, prefers ripgrep |

**Sensitive path blocking (both read and write):**
- `.ssh`, `.aws`, `.config/gcloud`, `.azure`, `.gnupg`, `.docker/config.json`, `.kube/config`
- `.npmrc`, `.password-store`, `Library/Keychains`
- Browser profiles (Chrome, Firefox)
- Shell rc files (`.bashrc`, `.zshrc`, `.bash_profile`, `.zprofile`, `.profile`)
- `/etc/shadow`, `/etc/passwd`, `/etc/sudoers`
- Resolves symlinks to prevent traversal

### `shell` Tool (shell_tool.go)

**Resources:** `bash`, `process`, `session`
**RequiresApproval:** true (all actions)

| Resource | Action | Required Fields | Optional Fields | Notes |
|----------|--------|----------------|-----------------|-------|
| bash | exec | command | timeout (default 120s), cwd, background, yield_ms (default 10000) | 50KB output cap, sanitized env |
| process | list | | filter | `ps aux` (unix) / `tasklist /V` (windows), max 50 results |
| process | kill | pid | signal (SIGTERM/SIGKILL/SIGINT/SIGHUP) | |
| process | info | pid | | Includes open file count via `lsof` |
| session | list | | | Running + recently finished |
| session | poll | session_id | | Drains pending stdout/stderr |
| session | log | session_id | | Full output history |
| session | write | session_id, data | | Write to stdin |
| session | kill | session_id | | Kill background session |

**Resource inference:** Omitted resource is inferred from action/fields:
- `exec` → bash, `poll`/`log`/`write` → session, PID present → process, command present → bash

**Environment sanitization (`sanitizedEnv()`):**
Strips: `LD_PRELOAD`, `LD_LIBRARY_PATH`, `LD_AUDIT`, all `LD_*`, `DYLD_INSERT_LIBRARIES`, all `DYLD_*`, `IFS`, `CDPATH`, `BASH_ENV`, `ENV`, `PROMPT_COMMAND`, all `BASH_FUNC_*`, `SHELLOPTS`, `BASHOPTS`, `GLOBIGNORE`, `PYTHONSTARTUP`, `PYTHONPATH`, `RUBYOPT`, `RUBYLIB`, `PERL5OPT`, `PERL5LIB`, `PERL5DB`, `NODE_OPTIONS`, `HOSTALIASES`, `RESOLV_HOST_CONF`, `LOCALDOMAIN`

### `web` Tool (web_tool.go)

**Resources:** `http`, `search`, `browser`
**RequiresApproval:** true

| Resource | Action | Required Fields | Optional Fields | Notes |
|----------|--------|----------------|-----------------|-------|
| http | fetch | url | method, headers, body, offset | SSRF-protected, HTML→text extraction, chunked pagination |
| search | query | query | engine (duckduckgo/google), limit | Tries native browser first, falls back to HTTP scraper |
| browser | navigate | url | profile, target_id, timeout | |
| browser | snapshot | | profile, target_id | DOM tree with element refs |
| browser | click | | ref/selector, profile, target_id | |
| browser | fill | | ref/selector, value, profile, target_id | Clears field first |
| browser | type | text | ref/selector, profile, target_id | Character-by-character |
| browser | screenshot | | output, profile, target_id | Base64 or file |
| browser | text | | ref/selector, profile, target_id, offset | Paginated extraction |
| browser | evaluate | text (JS) | profile, target_id, timeout | Execute JavaScript |
| browser | wait | | ref/selector, profile, target_id, timeout | |
| browser | scroll | | text (direction), ref/selector, profile, target_id | |
| browser | hover | | ref/selector, profile, target_id | |
| browser | select | value | ref/selector, profile, target_id | Dropdown option |
| browser | back/forward/reload | | profile, target_id | |
| browser | status | | profile | Profile status |
| browser | launch | | profile | Start managed browser |
| browser | close | | profile, target_id | |
| browser | list_pages | | profile | |

**Browser profiles:**
- `native` — Wails webview windows (desktop mode only, fast, undetectable, no Playwright)
- `nebo` — Managed Playwright browser (headless, full automation)
- `chrome` — Extension relay with user's authenticated Chrome sessions

**Resource inference:** Omitted resource is inferred from action:
- `search`/`query` → search, `fetch` → http, all browser actions → browser

**SSRF protection (transport-level):**
- Blocks private IPs: `127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16`, `0.0.0.0/8`, `100.64.0.0/10`, `198.18.0.0/15`, `::1/128`, `fc00::/7`, `fe80::/10`
- Blocks cloud metadata endpoints: `metadata.google.internal`, `metadata.google.com`
- Re-checks on redirect (prevents DNS rebinding)
- Both URL validation (pre-request) AND transport-level check (at connect time)

### `agent` Tool (agent_tool.go)

**Resources:** `task`, `reminder`, `memory`, `message`, `session`, `comm`, `profile`
**RequiresApproval:** false (handled per-action internally)

| Resource | Action | Required Fields | Optional Fields | Notes |
|----------|--------|----------------|-----------------|-------|
| **task** | spawn | prompt | description, wait (default true), timeout (default 300s), agent_type | Sub-agent goroutine via Orchestrator |
| | status | agent_id | | |
| | cancel | agent_id | | |
| | create | subject | | In-memory work task (ephemeral, session-scoped) |
| | update | task_id, status | | Status: pending/in_progress/completed |
| | delete | task_id | | |
| | list | | | Both work tasks + sub-agents |
| **reminder** | create | name | at, schedule, task_type, command, message, deliver | `at` for one-time ("in 5 min"), `schedule` for recurring (cron expression) |
| | list | | | |
| | delete | name | | |
| | pause | name | | Disable schedule |
| | resume | name | | Enable schedule |
| | run | name | | Trigger immediately |
| | history | name | | Execution history |
| **memory** | store | key, value | layer, namespace, tags, metadata | 3-tier: tacit/daily/entity |
| | recall | key | layer, namespace | Direct key lookup |
| | search | query | layer, namespace, limit | Hybrid vector + FTS |
| | list | | layer, namespace, limit | |
| | delete | key | layer, namespace | |
| | clear | | layer, namespace | Clear all in scope |
| **message** | send | channel, to, text | reply_to, thread_id | Via installed channel apps |
| | list | | | List connected channels |
| **session** | list | | | All conversation sessions |
| | history | session_key | limit (default 20) | |
| | status | session_key | | Statistics |
| | clear | session_key | | Clear messages |
| **comm** | send | to, topic, text | msg_type | DM to agent by ID |
| | subscribe | topic | | |
| | unsubscribe | topic | | |
| | list_topics | | | |
| | status | | | Plugin + connection info |
| | send_loop | channel_id, text | | Send to loop channel |
| | list_channels | | | Loop channels |
| | list_loops | | | All loops |
| | get_loop | loop_id | | Loop details |
| | loop_members | loop_id | | |
| | channel_members | channel_id | | |
| | channel_messages | channel_id | limit (default 50) | |
| **profile** | get | | | Read agent profile |
| | update | key, value | | Keys: name, emoji, creature, vibe, custom_personality, quiet_hours_start/end |
| | open_billing | | | Opens NeboLoop billing in browser |

**Resource aliases:** `routine`, `routines`, `remind`, `reminders`, `schedule`, `schedules`, `job`, `jobs`, `cron`, `event`, `events`, `calendar` → all resolve to `reminder`

**Action-to-resource inference:** `store`/`recall` → memory, `spawn` → task, `pause`/`resume`/`run` → reminder, `subscribe`/`unsubscribe`/`list_topics`/`send_loop`/`list_channels`/`list_loops`/`get_loop`/`loop_members`/`channel_members`/`channel_messages` → comm, `update`/`get` → profile

**Late-bound dependencies (set after construction):**
- `SetOrchestrator()` — sub-agent spawning
- `SetCommService()` — inter-agent communication
- `SetLoopChannelLister()` — loop channel enumeration
- `SetLoopQuerier()` — loop/member/message queries
- `SetChannelSender()` — channel messaging
- `SetAgentCallback()` — cron agent task execution
- `SetRecoveryManager()` — subagent crash recovery

---

## Security System (Three Layers)

### Layer 1: Safeguard (safeguard.go) — UNCONDITIONAL

Cannot be bypassed by autonomous mode, PolicyFull, or any user setting. Runs inside `Registry.Execute()` before `tool.Execute()`.

**File safeguard (write + edit actions only):**

| Platform | Protected Paths |
|----------|----------------|
| macOS | `/System`, `/usr/bin|sbin|lib|libexec|share`, `/bin`, `/sbin`, `/private/var/db`, `/Library/LaunchDaemons|LaunchAgents`, `/etc` |
| Linux | `/bin`, `/sbin`, `/usr/*`, `/boot`, `/etc`, `/proc`, `/sys`, `/dev`, `/root`, `/var/lib/dpkg|rpm|apt` |
| Windows | `C:\Windows`, `C:\Program Files*`, `C:\ProgramData`, `C:\Recovery` |
| Cross-platform | `.ssh`, `.gnupg`, `.aws/credentials`, `.kube/config`, `.docker/config.json` |
| Self-protection | Nebo's own data directory (database, config) |

Resolves symlinks to prevent indirection (e.g., `/etc` → `/private/etc` on macOS).

**Shell safeguard (bash exec only):**

| Pattern | Block |
|---------|-------|
| `sudo` | Direct, piped, chained, subshell — all forms |
| `su` | Direct and chained (but not "suspend", "sum", etc.) |
| `rm -rf /` | All variants including `--no-preserve-root` |
| `dd of=/dev/` | Block device writes |
| All disk formatting | `mkfs`, `fdisk`, `gdisk`, `parted`, `sfdisk`, `cfdisk`, `wipefs`, `sgdisk`, `partprobe`, `diskutil erasedisk|erasevolume|partitiondisk|apfs deletecontainer`, `format` |
| Fork bombs | `:(){ :\|:& };:` and variants |
| `/dev/` writes | Except `/dev/null`, `/dev/stdout`, `/dev/stderr` |
| `rm`/`chmod`/`chown` | When targeting protected system directories |

### Layer 2: Policy (policy.go) — CONFIGURABLE

| Level | Behavior |
|-------|----------|
| `PolicyDeny` | Block all dangerous operations |
| `PolicyAllowlist` (default) | Allow whitelisted, prompt for others |
| `PolicyFull` | Allow everything (dangerous!) |

| AskMode | Behavior |
|---------|----------|
| `AskModeOff` | Never ask |
| `AskModeOnMiss` (default) | Ask only for non-whitelisted |
| `AskModeAlways` | Always ask, even whitelisted |

**Safe bins (always allowed):**
`ls`, `pwd`, `cat`, `head`, `tail`, `grep`, `find`, `which`, `type`, `jq`, `cut`, `sort`, `uniq`, `wc`, `echo`, `date`, `env`, `printenv`, `git status`, `git log`, `git diff`, `git branch`, `git show`, `go version`, `node --version`, `python --version`

**Allowlist matching:** exact → binary name → binary + first arg

**Approval sources:** `ApprovalCallback` (web UI) → stdin prompt (CLI, supports y/N/a)

### Layer 3: Origin (origin.go) — PER-ORIGIN RESTRICTIONS

| Origin | Source | Default Deny (currently disabled) |
|--------|--------|----------------------------------|
| `OriginUser` | Web UI, CLI | None |
| `OriginComm` | Inter-agent communication | `shell` |
| `OriginApp` | External app binary | `shell` |
| `OriginSkill` | Matched skill template | `shell` |
| `OriginSystem` | Heartbeat, cron, recovery | None (auto-approves all) |

The deny list is currently disabled (`defaultOriginDenyList()` returns nil). The architecture is ready — re-enable by uncommenting the TODO in policy.go when the permission model matures.

Context propagation: `WithOrigin(ctx, origin)` / `GetOrigin(ctx)` (defaults to OriginUser).

---

## Registration Flow

### Phase 1: `RegisterDefaultsWithPermissions(permissions)`

Permissions map: `"chat"`, `"file"`, `"shell"`, `"web"`, `"contacts"`, `"desktop"`, `"media"`, `"system"`. Nil = allow all.

1. **Domain tools** (`registerDomainToolsWithPermissions`):
   - `file` → permission: `"file"`
   - `shell` → permission: `"shell"` (gets Policy + ProcessRegistry)
   - `web` → permission: `"web"` (gets WebDomainConfig with headless flag)
   - `screenshot` → permission: `"media"`
   - `vision` → permission: `"media"` (AnalyzeFunc wired later)

2. **Platform capabilities** (`RegisterPlatformCapabilitiesWithPermissions`):
   - Category mapping: productivity→contacts, system→system, media→media, desktop→desktop
   - Unknown categories registered by default

### Phase 2: Separately registered tools (require external dependencies)

- `agent` → via `RegisterAgentDomainTool()` (needs DB, session manager, memory tool, scheduler)
- `advisors` → via `RegisterAdvisorsTool()` (needs advisor loader, provider)
- `skill` → registered in `cmd/nebo/agent.go` (needs skills registry)
- `store` → registered in `cmd/nebo/agent.go` (needs NeboLoopClientProvider)

### Dynamic registration

- Apps can register tools at runtime via `Registry.Register()` (gRPC adapter wrapping app's tool service)
- `OnChange(fn)` listeners fire when tools are added/removed
- `Registry.Unregister(name)` removes tools (used when apps uninstall)

---

## Schema Builder (domain.go)

`BuildDomainSchema(DomainSchemaConfig)` generates JSON Schema from a config:

```go
type DomainSchemaConfig struct {
    Domain      string
    Description string
    Resources   map[string]ResourceConfig  // name → {Name, Actions, Description}
    Fields      []FieldConfig              // {Name, Type, Description, Required, RequiredFor, Enum, Default, Items}
    Examples    []string
}
```

Logic:
1. If multiple resources → adds `resource` field with enum + marks required
2. Collects all actions across resources → `action` field with enum (always required)
3. Each `FieldConfig` → property with type, description, enum, default, items (for arrays)
4. Globally required fields get added to `required` array
5. Examples appended to description text

`BuildDomainDescription(cfg)` generates a human-readable description with resource/action docs.

`ValidateResourceAction(resource, action, resources)` validates against allowed values. Falls back to empty-string resource for flat domains (like `file`).

---

## Snapshot/Vision Pipeline

### Flow: `screenshot(action: "see")`

```
1. Identify target app + window (accessibility API)
2. Capture window screenshot (platform-specific CGWindowListCreateImage / etc.)
3. Extract accessibility tree → []RawElement (platform-specific)
4. AssignElementIDs():
   a. Flatten tree recursively
   b. Filter to actionable elements with valid bounds
   c. Sort by screen position (top-to-bottom, left-to-right, 10px band grouping)
   d. Assign role-prefixed IDs: B1, T1, L1, C1, M1, S1, etc.
5. Render colored annotation overlays on screenshot
6. Store Snapshot in SnapshotStore (1h TTL, singleton)
7. Return: annotated image (base64) + element list (ID, role, label, bounds)
```

### Element ID Prefixes (snapshot_annotator.go)

| Prefix | Role(s) |
|--------|---------|
| B | button |
| T | textfield, text field |
| L | link |
| C | checkbox, check box, toggle |
| M | menu, menu item, menuitem |
| S | slider |
| A | tab, tab group |
| R | radio, radiobutton, radio button |
| P | popup, pop up button, combobox, combo box, select |
| G | image |
| X | static text, text |
| O | toolbar, tool bar |
| I | list |
| W | table |
| Z | scroll bar, scrollbar |
| U | group |
| N | window |

### Interaction via Desktop Tool

When the `desktop` tool receives an element ID (e.g., "B3"):
1. `SnapshotStore.LookupElement("B3", snapshotID)` finds the element
2. Element's `Bounds.Center()` gives screen coordinates
3. Desktop tool clicks/types at those coordinates

---

## Skill System (skill_tool.go)

The `SkillDomainTool` (tool name: `skill`) is the unified interface for standalone skills (SKILL.md files) and app-backed skills (gRPC adapters).

### Key constants

| Constant | Value | Purpose |
|----------|-------|---------|
| `DefaultSkillTTL` | 4 turns | Auto-matched skill expiry |
| `ManualSkillTTL` | 6 turns | Manually loaded skill expiry |
| `MaxActiveSkills` | 4 | Hard cap on concurrent active skills |
| `MaxSkillTokenBudget` | 16,000 chars | Combined content budget |

### Invocation tracking

When the model calls `skill(name: "X")`, the skill is marked as "invoked":
- Invoked skills get their SKILL.md content re-injected into subsequent system prompts
- Skills expire after N turns of inactivity (4 for auto-matched, 6 for manual)
- Combined content capped at 16,000 characters
- `AutoMatchSkills(userMessage)` checks triggers against user input and returns hints

### Actions

- `catalog` — list all registered skills
- `load` — invoke/activate a skill by name
- `create` — write SKILL.md to user skills directory
- `update` — update existing SKILL.md
- `delete` — remove SKILL.md file

---

## NeboLoop Store Tool (neboloop_tool.go)

Tool name: `store`. Two resources:

| Resource | Actions | Notes |
|----------|---------|-------|
| apps | list, get, install, uninstall, featured, popular, reviews | Formats as human-readable text |
| skills | list, get, install, uninstall | |

Uses `NeboLoopClientProvider` callback for lazy client creation. Additional fields: `id`, `query`, `category`, `page`, `page_size`.

---

## Tool Correction (registry.go)

When the LLM hallucinates a non-existent tool name, the registry returns specific correction messages:

| Hallucinated Name | Correction |
|-------------------|------------|
| `websearch`, `web_search` | `web(action: "search", query: "...")` |
| `webfetch`, `web_fetch` | `web(action: "fetch", url: "...")` |
| `read` | `file(action: "read", path: "...")` |
| `write` | `file(action: "write", path: "...", content: "...")` |
| `edit` | `file(action: "edit", path: "...", old_string: "...", new_string: "...")` |
| `grep` | `file(action: "grep", pattern: "...", path: "...")` |
| `glob` | `file(action: "glob", pattern: "...")` |
| `bash` | `shell(resource: "bash", action: "exec", command: "...")` |
| `apps`, `application` | `app(action: "list")` or `app(action: "launch", name: "...")` |

---

## Key Design Patterns

1. **Late binding** — Tools accept dependencies via setter methods (`SetOrchestrator`, `SetCommService`, `SetAnalyzeFunc`, `SetChannelSender`, `SetLoopQuerier`, `SetLoopChannelLister`, `SetRecoveryManager`, `SetAgentCallback`) to avoid import cycles and enable post-initialization wiring.

2. **Resource inference** — All domain tools infer the resource from the action when the LLM omits it. The `agent` tool also normalizes resource name synonyms.

3. **Defense in depth** — Three security layers (Safeguard → Policy → Origin), each independent and additive.

4. **Platform capability system** — Tools register via `init()` in platform-specific files with build tags. The `CapabilityRegistry` filters by current platform. This replaces the go-plugin architecture which doesn't work on mobile.

5. **OnChange listeners** — Registry fires callbacks when tools add/remove, enabling dynamic tool list updates (e.g., app install/uninstall pushes updated tool definitions to the LLM).

6. **MCP bridge** — When Nebo runs as an MCP server (for Claude CLI), tools get exposed as `mcp__nebo-agent__web`. When these names leak into session history from a different provider, the registry strips the prefix to find the actual tool.

7. **Singleton stores** — `SnapshotStore` and `ProcessRegistry` are singletons (or singleton-like) with cleanup goroutines and TTL-based expiration.
