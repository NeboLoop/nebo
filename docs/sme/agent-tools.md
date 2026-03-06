# Agent Tools -- Logic Deep-Dive

This document provides a comprehensive reference for every non-platform agent tool in Nebo's Go implementation. It covers the core tool infrastructure (registry, domain pattern, policy, safeguards, process management) and each domain tool (system/file, system/shell, web, bot, loop, message, event) plus standalone tools (grep, vision, tts, screenshot, query_sessions, web_sanitize, memory).

Source directory: `internal/agent/tools/`

---

## Table of Contents

1. [Core Infrastructure](#1-core-infrastructure)
   - [Registry](#11-registry)
   - [STRAP Domain Pattern](#12-strap-domain-pattern)
   - [Origin Tracking](#13-origin-tracking)
   - [Policy System](#14-policy-system)
   - [Safeguard System](#15-safeguard-system)
   - [Capabilities Registry](#16-capabilities-registry)
   - [Process Registry](#17-process-registry)
   - [Process Signals](#18-process-signals)
   - [Desktop Queue](#19-desktop-queue)
   - [Execution Pipeline](#110-execution-pipeline)
2. [Domain Tools](#2-domain-tools)
   - [SystemTool (system)](#21-systemtool)
   - [FileTool (system/file)](#22-filetool)
   - [ShellTool (system/shell)](#23-shelltool)
   - [WebDomainTool (web)](#24-webdomaintool)
   - [BotTool (bot)](#25-bottool)
   - [LoopTool (loop)](#26-looptool)
   - [MsgTool (message)](#27-msgtool)
   - [EventTool (event)](#28-eventtool)
3. [Standalone Tools](#3-standalone-tools)
   - [GrepTool](#31-greptool)
   - [VisionTool](#32-visiontool)
   - [TTSTool](#33-ttstool)
   - [ScreenshotTool](#34-screenshottool)
   - [QuerySessionsTool](#35-querysessionstool)
   - [MemoryTool](#36-memorytool)
4. [Utilities](#4-utilities)
   - [Web Sanitize](#41-web-sanitize)
   - [Scheduler Interface](#42-scheduler-interface)
   - [Types and Helpers](#43-types-and-helpers)

---

## 1. Core Infrastructure

### 1.1 Registry

**File:** `registry.go`

The `Registry` is the central hub for tool management. It owns the tool map, policy, process registry, desktop queue, hook dispatcher, and change listeners.

#### Struct

```go
type Registry struct {
    mu              sync.RWMutex
    tools           map[string]Tool
    policy          *Policy
    processRegistry *ProcessRegistry
    listeners       []ChangeListener
    desktopQueue    DesktopQueueFunc
    systemTool      *SystemTool
    hooks           HookDispatcher
}
```

#### Tool Interface

Every tool must implement:

```go
type Tool interface {
    Name() string
    Description() string
    Schema() json.RawMessage
    Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error)
    RequiresApproval() bool
}
```

#### ToolResult

```go
type ToolResult struct {
    Content  string `json:"content"`
    IsError  bool   `json:"is_error,omitempty"`
    ImageURL string `json:"image_url,omitempty"`
}
```

#### Registration

- `NewRegistry(policy *Policy) *Registry` -- creates registry, auto-creates a `ProcessRegistry` with background sweeper.
- `Register(tool Tool)` -- adds or overwrites a tool, notifies change listeners.
- `Unregister(name string)` -- removes a tool, notifies change listeners.
- `OnChange(fn ChangeListener)` -- registers a listener called on add/remove. Signature: `func(added []string, removed []string)`.

#### Default Registration

`RegisterDefaultsWithPermissions(permissions map[string]bool)` registers tools filtered by category permissions. Categories: `"chat"`, `"file"`, `"shell"`, `"web"`, `"contacts"`, `"desktop"`, `"media"`, `"system"`. A nil map means allow all.

Registration order:
1. `SystemTool` -- always registered (contains file + shell + platform resources)
2. `WebDomainTool` -- registered if `web` permission is allowed
3. `LoopTool` -- always registered
4. `MsgTool` -- always registered
5. `BotTool` -- registered separately via `RegisterBotTool()` (needs DB + session manager)
6. `EventTool` -- registered separately via `RegisterEventTool()` (needs scheduler)
7. `AppTool` -- registered separately via `RegisterAppTool()`
8. Platform capabilities -- registered via `RegisterPlatformCapabilitiesWithPermissions()`

#### Execution Flow (Registry.Execute)

```
1. MCP prefix strip: "mcp__nebo-agent__web" -> "web" (only if tool not found as-is)
2. Tool lookup (unknown tool -> toolCorrection() message + available tool list)
3. Hard safeguard check: CheckSafeguard(toolName, input)
4. Desktop queue routing: executeWithDesktopQueue(ctx, tool, toolCall)
   -> routes desktop tools through LaneDesktop
   -> otherwise calls executeTool() inline
```

#### MCP Prefix Stripping

`stripMCPPrefix(name string) string` handles tool names leaked from MCP namespace. Pattern: `mcp__{server}__{tool}` -> `{tool}`. Only strips if the full prefixed name is not found in the registry (preserves external MCP proxy tools).

#### Tool Correction

`toolCorrection(name string) string` maps ~40 hallucinated tool names to correct STRAP invocations. Examples:
- `"read"` -> `system(resource: "file", action: "read", path: "...")`
- `"bash"` -> `system(resource: "shell", action: "exec", command: "...")`
- `"websearch"` -> `web(action: "search", query: "...")`
- `"clipboard"` -> `system(resource: "clipboard", action: "get")`
- `"screenshot"` -> `desktop(resource: "screenshot", action: "capture")`

#### HookDispatcher Interface

```go
type HookDispatcher interface {
    ApplyFilter(ctx context.Context, hook string, payload []byte) ([]byte, bool)
    DoAction(ctx context.Context, hook string, payload []byte)
    HasSubscribers(hook string) bool
}
```

Used for `tool.pre_execute` and `tool.post_execute` hooks from the app platform.

---

### 1.2 STRAP Domain Pattern

**File:** `domain.go`

STRAP (Single Tool Resource Action Pattern) consolidates 35+ individual tools into ~5 domain tools, reducing LLM context window overhead by ~80%.

#### DomainTool Interface

```go
type DomainTool interface {
    Tool
    Domain() string
    Resources() []string
    ActionsFor(resource string) []string
}
```

#### Core Types

```go
type DomainInput struct {
    Resource string `json:"resource,omitempty"`
    Action   string `json:"action"`
}

type ResourceConfig struct {
    Name        string
    Actions     []string
    Description string
}

type FieldConfig struct {
    Name        string
    Type        string      // "string", "integer", "boolean", "array", "object"
    Description string
    Required    bool
    RequiredFor []string
    Enum        []string
    Default     any
    Items       string         // Item type for arrays
    ItemSchema  map[string]any // Full JSON Schema for array items
}

type DomainSchemaConfig struct {
    Domain      string
    Description string
    Resources   map[string]ResourceConfig
    Fields      []FieldConfig
    Examples    []string
}
```

#### Schema Generation

`BuildDomainSchema(cfg DomainSchemaConfig) json.RawMessage`:
- Adds `resource` field with enum if multiple resources exist.
- Collects all actions across resources into a single `action` enum.
- Builds `properties` from `FieldConfig` list (respects Type, Enum, Default, Items/ItemSchema).
- Returns JSON Schema with `required: ["action"]` (plus `"resource"` if multi-resource).

`BuildDomainDescription(cfg DomainSchemaConfig) string`:
- Generates human-readable description with resource/action documentation and examples.

#### Validation

`ValidateResourceAction(resource, action string, resources map[string]ResourceConfig) error`:
- Validates resource exists in the resource map.
- Falls back to empty-string key for flat (single-resource) domains.
- Validates action exists in the resource's action list.

`ActionRequiresApproval(action string, dangerousActions []string) bool`:
- Simple set membership check.

---

### 1.3 Origin Tracking

**File:** `origin.go`

Origins identify request sources for per-origin tool restrictions.

```go
type Origin string

const (
    OriginUser   Origin = "user"   // Direct user (web UI, CLI)
    OriginComm   Origin = "comm"   // Inter-agent communication
    OriginApp    Origin = "app"    // External app binary
    OriginSkill  Origin = "skill"  // Matched skill template
    OriginSystem Origin = "system" // Internal system tasks (heartbeat, cron, recovery)
)
```

Context propagation functions:
- `WithOrigin(ctx, origin)` / `GetOrigin(ctx)` -- defaults to `OriginUser` if not set
- `WithSessionKey(ctx, key)` / `GetSessionKey(ctx)`
- `WithSessionID(ctx, id)` / `GetSessionID(ctx)`

---

### 1.4 Policy System

**File:** `policy.go`

Controls tool approval behavior with three security levels and three ask modes.

#### Types

```go
type PolicyLevel string
const (
    PolicyDeny      PolicyLevel = "deny"      // Deny all dangerous
    PolicyAllowlist PolicyLevel = "allowlist"  // Allow whitelisted only
    PolicyFull      PolicyLevel = "full"       // Allow all
)

type AskMode string
const (
    AskModeOff    AskMode = "off"     // Never ask
    AskModeOnMiss AskMode = "on-miss" // Ask only for non-whitelisted
    AskModeAlways AskMode = "always"  // Always ask
)
```

#### Policy Struct

```go
type Policy struct {
    Level            PolicyLevel
    AskMode          AskMode
    Allowlist        map[string]bool
    ApprovalCallback ApprovalCallback
    IsAutonomous     AutonomousCheck  // Checked on every call (live toggle)
    OriginDenyList   map[Origin]map[string]bool
}
```

#### Safe Bins (always allowed)

```go
var SafeBins = []string{
    "ls", "pwd", "cat", "head", "tail", "grep", "find", "which", "type",
    "jq", "cut", "sort", "uniq", "wc", "echo", "date", "env", "printenv",
    "git status", "git log", "git diff", "git branch", "git show",
    "go version", "node --version", "python --version",
}
```

#### Default Origin Deny List

`comm`, `app`, and `skill` origins are denied `shell` and `system:shell` access. Only `user` and `system` origins can execute shell commands.

#### Approval Logic

`RequiresApproval(cmd string) bool`:
1. If `IsAutonomous()` returns true, skip approval.
2. If `PolicyFull`, never require.
3. If `PolicyDeny`, always require.
4. If command is in allowlist and `AskMode != always`, skip approval.
5. Otherwise require unless `AskMode == off`.

`RequestApproval(ctx, toolName, input)`:
1. Auto-approve for `OriginSystem` (cron, heartbeat, recovery).
2. Auto-approve if `IsAutonomous()` is true.
3. Auto-approve if `PolicyFull`.
4. For bash tools, check `RequiresApproval()` with extracted command.
5. Use `ApprovalCallback` (web UI) or fall back to stdin prompt.
6. Stdin prompt accepts `y/yes`, `a/always` (adds to allowlist), default = deny.

`IsDeniedForOrigin(origin, toolName, input...)`:
- Checks bare tool name against deny list.
- Also checks `toolName:resource` compound key if input contains a resource field.
- Hard deny -- no approval prompt, just rejected.

#### Danger Detection

`IsDangerous(cmd string) bool`:
Checks for: `rm -rf`, `rm -r`, `rmdir`, `sudo`, `su `, `chmod 777`, `chown`, `dd `, `mkfs`, `> /dev/`, `curl | sh`, `curl | bash`, `wget | sh`, `eval `, `exec `, fork bomb `:(){ :|:& };:`.

---

### 1.5 Safeguard System

**File:** `safeguard.go`

Hard safety limits that CANNOT be overridden by policy, autonomous mode, or any setting. Runs inside `registry.Execute()` before `tool.Execute()`.

#### Entry Point

```go
func CheckSafeguard(toolName string, input json.RawMessage) error
```

Dispatches to:
- `checkFileSafeguard(input)` for tool name `"file"`
- `checkShellSafeguard(input)` for tool name `"shell"`
- `nil` for all other tools

#### File Safeguard

Only guards `write` and `edit` actions. Resolves path to absolute, checks both original and symlink-resolved path against protected paths.

#### Shell Safeguard

Only guards `bash/exec` (resource `"bash"` or empty, action `"exec"`). Checks:
1. **sudo** -- `hasSudo()` detects direct, piped (`| sudo`), chained (`&& sudo`, `; sudo`), and subshell (`$(sudo`) forms.
2. **su** -- `hasSu()` detects `su `, avoiding false positives with "suspend", "surface", etc.
3. **Destructive commands** -- `checkDestructiveCommand()`:
   - `isRootWipe()` -- `rm -rf /` variants with trailing content checks
   - `dd` writing to `/dev/`
   - Disk formatting: `mkfs`, `fdisk`, `gdisk`, `parted`, `sfdisk`, `cfdisk`, `wipefs`, `sgdisk`, `partprobe`, `diskutil erasedisk/erasevolume/partitiondisk/apfs deletecontainer`, `format`
   - Fork bombs
   - Writing to `/dev/` (except `/dev/null`, `/dev/stdout`, `/dev/stderr`)
   - `rm` targeting protected system directories
   - `chmod`/`chown` on system paths

#### Protected Paths

Platform-dispatched via `isProtectedPath(absPath string) string`:

**macOS** (`isProtectedPathDarwin`):
- `/`, `/System`, `/usr/bin`, `/usr/sbin`, `/usr/lib`, `/usr/libexec`, `/usr/share`
- `/bin`, `/sbin`, `/private/var/db`, `/Library/LaunchDaemons`, `/Library/LaunchAgents`, `/etc`

**Linux** (`isProtectedPathLinux`):
- `/`, `/bin`, `/sbin`, `/usr/bin`, `/usr/sbin`, `/usr/lib`, `/usr/libexec`, `/usr/share`
- `/boot`, `/etc`, `/proc`, `/sys`, `/dev`, `/root`
- `/var/lib/dpkg`, `/var/lib/rpm`, `/var/lib/apt`

**Windows** (`isProtectedPathWindows`):
- `c:\windows`, `c:\program files`, `c:\program files (x86)`, `c:\programdata`, `c:\recovery`, `c:\$recycle.bin`

**Cross-platform sensitive user paths** (`isProtectedUserPath`):
- `~/.ssh`, `~/.gnupg`, `~/.aws/credentials`, `~/.aws/config`, `~/.kube/config`, `~/.docker/config.json`
- Nebo's own data directory (`neboDataDirs()`) -- protects the database from self-harm

---

### 1.6 Capabilities Registry

**File:** `capabilities.go`

Platform-aware tool registration system that replaces the go-plugin architecture (go-plugin doesn't work on iOS/Android).

```go
const (
    PlatformDarwin  = "darwin"
    PlatformLinux   = "linux"
    PlatformWindows = "windows"
    PlatformIOS     = "ios"
    PlatformAndroid = "android"
    PlatformAll     = "all"
)

type Capability struct {
    Tool          Tool
    Platforms     []string
    Category      string   // "system", "media", "productivity", "desktop"
    RequiresSetup bool
}
```

**Global instance:** `var capabilities = NewCapabilityRegistry()`

Registration functions:
- `RegisterCapability(cap) bool` -- registers if available on current platform
- `RegisterPlatformCapabilitiesWithPermissions(tr, permissions)` -- filters by category-to-permission mapping:
  - `"productivity"` -> `"contacts"`
  - `"system"` -> `"system"`
  - `"media"` -> `"media"`
  - `"desktop"` -> `"desktop"`

Platform detection: `detectPlatform()` uses `runtime.GOOS` with an `isIOS` override for iOS build tags.

---

### 1.7 Process Registry

**File:** `process_registry.go`

Tracks running and finished background bash processes.

#### Constants

```go
const (
    DefaultMaxOutputChars        = 50000
    DefaultPendingMaxOutputChars = 10000
    DefaultTailChars             = 2000
    DefaultFinishedTTL           = 30 * time.Minute
    DefaultMaxFinished           = 100
    SweeperInterval              = 5 * time.Minute
)
```

#### ProcessSession

```go
type ProcessSession struct {
    ID, Command, Cwd    string
    PID                 int
    StartedAt           time.Time
    ExitCode            *int
    ExitSignal          string
    Exited, Backgrounded, Truncated bool
    MaxOutputChars, PendingMaxOutputChars, TotalOutputChars int
    PendingStdout, PendingStderr []byte
    Aggregated          string
    Tail                string  // Last 2000 chars
    cmd                 *exec.Cmd
    stdin               io.WriteCloser
    cancel              context.CancelFunc
    mu                  sync.Mutex
}
```

#### ProcessRegistry

```go
type ProcessRegistry struct {
    runningSessions  map[string]*ProcessSession
    finishedSessions map[string]*ProcessSession
    mu               sync.RWMutex
    finishedTTL      time.Duration  // 30 min
    maxFinished      int            // 100
    stopSweeper      chan struct{}
}
```

#### Key Operations

- `NewProcessRegistry()` -- starts background sweeper goroutine.
- `SpawnBackgroundProcess(ctx, command, cwd, yieldMs)`:
  1. Generates slug ID (`adjective-noun` format via `GenerateSessionSlug()`).
  2. Creates cancellable context.
  3. Uses platform-specific shell (`ShellCommand()`).
  4. Sanitizes environment (`sanitizedEnv()`).
  5. Sets up stdin/stdout/stderr pipes.
  6. Starts process, registers session.
  7. Spawns goroutines for stdout/stderr reading and exit watching.
  8. Waits for `yieldMs` (default 10s) before marking as backgrounded.

- `AppendOutput(id, isStderr, data)`:
  - Appends to `Aggregated` (capped at `MaxOutputChars`).
  - Maintains `Tail` (last 2000 chars).
  - Appends to `PendingStdout`/`PendingStderr` (capped at `PendingMaxOutputChars`).
  - Tracks `TotalOutputChars`.

- `DrainPending(id)` -- returns and clears pending stdout/stderr.
- `MarkExited(id, exitCode, exitSignal)` -- moves session from running to finished.
- `sweeper()` -- runs every 5 minutes, removes sessions older than 30 min, enforces max 100 finished.

#### Session Slug Generation

`GenerateSessionSlug(isTaken func(string) bool) string`:
- 15 adjectives: swift, keen, bold, calm, warm, cool, soft, firm, fair, true, safe, wise, kind, neat, pure
- 15 nouns: cove, dale, glen, vale, reef, cape, bay, peak, ford, moor, oak, elm, ash, pine, fern
- Up to 12 attempts with `time.Now().UnixNano()` for randomness
- Fallback: `"proc-" + timestamp`

---

### 1.8 Process Signals

**Files:** `process_signal_unix.go`, `process_signal_windows.go`

`KillProcessWithSignal(process *os.Process, signal string) error`:

**Unix:** Maps signal name to `syscall.Signal`:
- `"SIGTERM"` or default -> `syscall.SIGTERM`
- `"SIGKILL"` -> `syscall.SIGKILL`
- `"SIGINT"` -> `syscall.SIGINT`
- `"SIGHUP"` -> `syscall.SIGHUP`

**Windows:** Always uses `process.Kill()` (no Unix signals).

**Shell Command** (`shell_unix.go`):
```go
func ShellCommand() (shell string, args []string) {
    // Checks /bin/bash, /usr/bin/bash, /usr/local/bin/bash
    // Falls back to "bash" on PATH
    return path, []string{"-c"}
}
```

---

### 1.9 Desktop Queue

**File:** `desktop_queue.go`

Serializes desktop-category tool execution through a dedicated lane to prevent concurrent screen/mouse/keyboard conflicts.

```go
type DesktopQueueFunc func(ctx context.Context, execute func(ctx context.Context) *ToolResult) *ToolResult

var desktopToolNames = map[string]bool{
    "desktop": true, "accessibility": true, "screenshot": true,
    "app": true, "browser": true, "window": true,
    "menubar": true, "dialog": true, "shortcuts": true,
}
```

`executeWithDesktopQueue(ctx, tool, toolCall)`:
- If desktop queue is configured AND tool is a desktop tool, routes through the queue function.
- Otherwise calls `executeTool()` directly.

---

### 1.10 Execution Pipeline

**File:** `desktop_queue.go` (function `executeTool`)

The full execution pipeline for every tool call:

```
1. Origin check: policy.IsDeniedForOrigin(origin, toolName, input)
   -> Hard deny if blocked for this origin

2. HOOK: tool.pre_execute
   -> ApplyFilter("tool.pre_execute", {tool, input})
   -> If handled=true, return app's result directly
   -> Otherwise apply input modifications from filter

3. Approval check: tool.RequiresApproval() && policy.RequestApproval(ctx, name, input)
   -> Denied = error result

4. tool.Execute(ctx, input)

5. Result truncation: 100KB max (100,000 chars)

6. HOOK: tool.post_execute
   -> ApplyFilter("tool.post_execute", {tool, input, result})
   -> Apply result modifications from filter
```

---

## 2. Domain Tools

### 2.1 SystemTool

**File:** `system_tool.go`
**Tool name:** `"system"`

Consolidates OS-level operations. Core resources (always present): `file`, `shell`. Platform resources (registered via `init()` + `RegisterSystemResourceInit()`): `app`, `clipboard`, `settings`, `music`, `search`, `keychain`.

```go
type SystemTool struct {
    fileTool      *FileTool
    shellTool     *ShellTool
    platformTools map[string]Tool
}
```

**Resource inference** (`inferResource`):
- File-only actions: `read`, `write`, `edit`, `glob`, `grep` -> `"file"`
- Shell-only actions: `exec`, `poll`, `log` -> `"shell"`
- Platform actions: `launch`, `quit`, `quit_all`, `activate`, `hide`, `frontmost` -> `"app"`; `type`, `history` -> `"clipboard"`; `volume`, `brightness`, `sleep`, `lock`, `wifi`, `bluetooth`, `darkmode`, `mute`, `unmute` -> `"settings"`; `play`, `pause`, `next`, `previous`, `playlists`, `shuffle` -> `"music"`

**Execute routing:** Parses `DomainInput`, infers resource if empty, delegates to `fileTool`, `shellTool`, or `platformTools[resource]`.

---

### 2.2 FileTool

**File:** `file_tool.go`
**Accessed via:** `system(resource: "file", ...)`

```go
type FileTool struct {
    rgPath     string
    OnFileRead func(path string)  // For post-compaction re-injection tracking
}

type FileInput struct {
    Action      string  // read, write, edit, glob, grep
    Path        string
    Offset, Limit int
    Content     string
    Append      bool
    OldString, NewString string
    ReplaceAll  bool
    Pattern, Regex, Glob string
    CaseInsensitive bool
    Context     int
}
```

#### Actions

**read:**
- Defaults: `offset=1`, `limit=2000`
- 1MB line buffer, 2000-char line truncation
- Output format: `%6d\t%s\n` (line number + tab + content)
- Calls `OnFileRead` callback for access tracking
- Validates path against sensitive paths

**write:**
- Creates parent directories (`MkdirAll`, 0755)
- File permissions: 0644
- Supports `append` mode (`O_APPEND` vs `O_TRUNC`)
- Returns `"Wrote N bytes to path"` or `"Appended N bytes to path"`

**edit:**
- Requires `old_string` (non-empty, different from `new_string`)
- Rejects if `old_string` not found
- If multiple occurrences and `replace_all=false`, returns error with count
- Uses `strings.Replace` (single) or `strings.ReplaceAll`

**glob:**
- Default limit: 1000
- Supports `**` recursive matching via `recursiveGlob()`
- Skips: `.`-prefixed dirs, `node_modules`, `vendor`, `__pycache__`
- Sorts results by modification time (newest first)
- Returns one path per line

**grep:**
- Delegates to `GrepTool` with ripgrep or Go fallback
- Default limit: 100

#### Path Validation

`validateFilePath(rawPath, action)`:
1. Expands `~/` to home directory
2. Resolves to absolute path
3. Resolves symlinks (`filepath.EvalSymlinks`)
4. Checks both original and resolved paths against `sensitivePaths`

**Sensitive paths list:**
- `~/.ssh`, `~/.aws`, `~/.config/gcloud`, `~/.azure`, `~/.gnupg`
- `~/.docker/config.json`, `~/.kube/config`, `~/.npmrc`, `~/.password-store`
- `~/Library/Keychains`, browser profiles (Chrome, Firefox)
- Shell init files: `.bashrc`, `.bash_profile`, `.zshrc`, `.zprofile`, `.profile`
- System: `/etc/shadow`, `/etc/passwd`, `/etc/sudoers`

---

### 2.3 ShellTool

**File:** `shell_tool.go`
**Accessed via:** `system(resource: "shell", ...)`

```go
type ShellTool struct {
    policy   *Policy
    registry *ProcessRegistry
}

type ShellInput struct {
    Resource   string  // bash, process, session
    Action     string  // exec, list, kill, info, poll, log, write
    Command    string
    Timeout    int
    Cwd        string
    Background bool
    YieldMs    int
    PID        int
    Signal     string
    Filter     string
    SessionID  string
    Data       string
}
```

#### Resource Inference

When `Resource` is empty:
- `action == "exec"` -> `"bash"`
- `action == "poll"` or `"log"` or `"write"` -> `"session"`
- `PID > 0` -> `"process"`
- `SessionID != ""` -> `"session"`
- `Command != ""` -> `"bash"`

#### bash/exec

1. If `background=true` and processRegistry available, delegates to `executeBackground()`
2. Default timeout: 120 seconds
3. Uses `ShellCommand()` for platform shell + `sanitizedEnv()`
4. Captures stdout and stderr separately
5. Error handling: timeout -> deadline exceeded message, exit error -> exit code, other -> generic error
6. Output truncation: 50,000 chars max

#### Background Execution

`executeBackground()`:
- Default yield: 10,000 ms (10 seconds)
- Calls `ProcessRegistry.SpawnBackgroundProcess()`
- Waits 100ms for initial output
- Returns session ID, PID, command, and available management actions

#### process/list

- macOS/Linux: `ps aux` (filtered by command name, max 50 lines)
- Windows: `tasklist /V`

#### process/kill

- `os.FindProcess(pid)` + `KillProcessWithSignal(process, signal)`

#### process/info

- macOS: `ps -p {pid} -o pid,ppid,user,%cpu,%mem,state,start,time,command`
- Linux: `ps -p {pid} -o pid,ppid,user,%cpu,%mem,stat,start,time,cmd`
- Windows: `tasklist /FI "PID eq {pid}" /V`
- Unix also runs `lsof -p {pid}` for open file count

#### session/list, poll, log, write, kill

- `list` -- shows running and finished sessions with IDs, PIDs, commands
- `poll` -- drains pending output (incremental), shows status
- `log` -- returns full aggregated output
- `write` -- writes data to session's stdin
- `kill` -- terminates via `ProcessRegistry.KillSession()`

#### Environment Sanitization

`sanitizedEnv()` removes dangerous environment variables:
- Dynamic linker injection: `LD_PRELOAD`, `LD_LIBRARY_PATH`, `LD_AUDIT`, `DYLD_INSERT_LIBRARIES`, `DYLD_LIBRARY_PATH`, `DYLD_FRAMEWORK_PATH`, `DYLD_FALLBACK_LIBRARY_PATH`
- Shell manipulation: `IFS`, `CDPATH`, `BASH_ENV`, `ENV`, `PROMPT_COMMAND`, `BASH_FUNC_*`, `SHELLOPTS`, `BASHOPTS`, `GLOBIGNORE`, `BASH_XTRACEFD`
- DNS/locale: `LOCALDOMAIN`, `HOSTALIASES`, `RESOLV_HOST_CONF`
- Language injection: `PYTHONSTARTUP`, `PYTHONPATH`, `RUBYOPT`, `RUBYLIB`, `PERL5OPT`, `PERL5LIB`, `PERL5DB`, `NODE_OPTIONS`
- Also blocks all `LD_*` and `DYLD_*` prefixes

---

### 2.4 WebDomainTool

**File:** `web_tool.go`
**Tool name:** `"web"`

```go
type WebDomainTool struct {
    client       *http.Client  // SSRF-safe: ssrfSafeTransport() + ssrfSafeRedirectCheck()
    searchAPIKey string
    searchCX     string
    headless     bool
}
```

**Resources:** `http`, `search`, `browser`, `devtools`

#### Resource Inference

When resource is empty, maps action to resource:
- `search`, `query` -> `"search"` (normalizes action to `"query"`)
- `fetch` -> `"http"`
- Browser lifecycle/interaction actions -> `"browser"`
- `console`, `source`, `storage`, `dom`, `cookies`, `performance` -> `"devtools"`

#### http/fetch

1. URL validation via `validateFetchURL()`
2. Default method: `GET`
3. User-Agent: `"Nebo/1.0"`
4. Response processing: `ExtractVisibleText()` for HTML, passthrough for other types
5. Chunked output via `FormatFetchResult()` -- 50,000 char chunks, accessible via `offset` parameter
6. Returns error for HTTP 400+

#### search/query

1. Default engine: `duckduckgo`, default limit: 10
2. DuckDuckGo flow:
   - First tries native browser rendering (`searchViaNativeBrowser`)
   - Falls back to HTTP scraper (`searchDuckDuckGo`)
3. Google requires API key + CX configuration
4. Returns numbered results: title, URL, snippet

#### browser/*

Three profiles:
- `"native"` -- Wails webview windows (fast, undetectable)
- `"nebo"` -- Managed Playwright browser (headless by default)
- `"chrome"` -- Extension relay (uses authenticated Chrome sessions)

Lifecycle actions: `status`, `launch`, `close`, `list_pages`

Interaction actions (18): `navigate`, `snapshot`, `click`, `fill`, `type`, `screenshot`, `text`, `evaluate`, `wait`, `scroll`, `hover`, `select`, `back`, `forward`, `reload`

Element targeting: by `ref` (snapshot element ID like `"e5"`) or by `selector` (CSS selector)

#### devtools/*

6 actions: `console`, `source`, `storage`, `dom`, `cookies`, `performance`

Fields: `level` (console filter), `clear` (clear after read), `key` (storage key), `storage_type` (`"local"`, `"session"`, `"cookies"`)

---

### 2.5 BotTool

**File:** `bot_tool.go`
**Tool name:** `"bot"`

The agent's self-management tool. Consolidates: task, memory, session, profile, context, advisors, vision, ask.

```go
type BotTool struct {
    orchestrator   *orchestrator.Orchestrator
    memory         *MemoryTool
    sessions       *session.Manager
    currentUserID  string
    sessionQuerier SessionQuerier
    identitySyncer func(ctx context.Context, name, role string)
    askCallback    AskCallback
    visionTool     *VisionTool
    advisorsTool   *AdvisorsTool
    workTasks      sync.Map  // sessionKey -> *[]WorkTask
    hooks          HookDispatcher
}
```

Late-bound setters: `SetOrchestrator`, `SetRecoveryManager`, `SetIdentitySyncer`, `SetAskCallback`, `SetCurrentUser`, `SetSessionQuerier`, `SetVisionTool`, `SetAdvisorsTool`, `SetHookDispatcher`

#### Action-to-Resource Inference

```go
var botActionToResource = map[string]string{
    "store": "memory", "recall": "memory", "spawn": "task",
    "deliberate": "advisors", "analyze": "vision",
    "get": "profile", "update": "profile", "open_billing": "profile",
    "reset": "context", "compact": "context", "summary": "context",
}
```

#### task Resource

Actions: `spawn`, `status`, `cancel`, `list`, `create`, `update`, `delete`

**spawn:** Creates a sub-agent via the orchestrator.
- Required: `prompt`
- Optional: `description` (auto-truncated from prompt), `wait` (default true), `timeout` (default 300s), `agent_type` (`"explore"`, `"plan"`, `"general"`)
- Gets subagent model from `provider.GetModelsConfig().LaneRouting.Subagent`
- Builds system prompt via `buildAgentSystemPrompt()` based on agent type

**create/update/delete/list:** In-memory work task tracking with `sync.Map`.
- `WorkTask{ID, Subject, Status, CreatedAt}` -- status: `pending`, `in_progress`, `completed`
- Persisted to DB via `sessions.SetWorkTasks()` as JSON
- Hydrated from DB on first access per session
- Auto-incrementing IDs via `botWorkTaskCounter` (atomic int64)

#### memory Resource

Actions: `store`, `recall`, `search`, `list`, `delete`, `clear`

Delegates to `MemoryTool` with app hook support:
- `memory.pre_store` filter hook before store
- `memory.pre_recall` filter hook before recall

#### session Resource

Actions: `list`, `history`, `status`, `clear`, `query`

- `list` -- lists all sessions for current user
- `history` -- returns messages with tool call details (limit default 20)
- `status` -- message counts per role
- `clear` -- clears session messages
- `query` -- cross-session reads via `SessionQuerier`

#### profile Resource

Actions: `get`, `update`, `open_billing`

- `get` -- reads from SQLite `agent_profile` table
- `update` -- validates key/value:
  - `name` -- max 50 chars, triggers identity sync to NeboLoop
  - `role` -- max 100 chars, triggers identity sync
  - `emoji` -- max 10 chars
  - `creature` -- max 50 chars
  - `vibe` -- max 100 chars
  - `custom_personality` -- max 2000 chars
  - `quiet_hours` -- validates format
- `open_billing` -- opens billing URL in browser via `openBrowserURL()`

#### context Resource

Actions: `reset`, `compact`, `summary`

Placeholder handlers -- actual behavior handled by the runner (returns instruction messages for the runner to act on).

#### advisors Resource

Action: `deliberate`

Delegates to `AdvisorsTool` with `task` and optional `advisors_list`.

#### vision Resource

Action: `analyze`

Delegates to `VisionTool` with `image` and `prompt`.

#### ask Resource

Actions: `prompt`, `confirm`, `select`

- Generates UUID request ID
- Requires `AskCallback` to be set
- Builds `[]AskWidget` from input widgets
- Blocks until user responds via callback

---

### 2.6 LoopTool

**File:** `loop_tool.go`
**Tool name:** `"loop"`

NeboLoop communication: bot-to-bot messaging, loop channels, and topic subscriptions.

```go
type LoopTool struct {
    commService       CommService
    loopChannelLister func(ctx context.Context) ([]LoopChannelInfo, error)
    loopQuerier       LoopQuerier
}
```

**Resources:** `dm`, `channel`, `group`, `topic`

#### Action-to-Resource Inference

```go
var loopActionToResource = map[string]string{
    "subscribe": "topic", "unsubscribe": "topic", "messages": "channel",
}
```

#### dm/send

- Required: `to` (target agent ID), `topic`, `text`
- Default `msg_type`: `"message"` (also: `"mention"`, `"proposal"`, `"command"`, `"info"`)
- Uses `commService.Send(ctx, to, topic, text, msgType)`

#### channel/*

- `send` -- requires `channel_id` and `text`, sends via `commService.Send(ctx, channelID, "", text, "loop_channel")`
- `list` -- uses `loopChannelLister` callback, returns channel name, ID, loop name, loop ID
- `messages` -- requires `channel_id`, default limit 50, max 200, via `loopQuerier.ListChannelMessages()`
- `members` -- requires `channel_id`, shows name, ID, online status, role

#### group/*

- `list` -- `loopQuerier.ListLoops()`, shows name, ID, member count, description
- `get` -- requires `loop_id`, `loopQuerier.GetLoop()`
- `members` -- requires `loop_id`, `loopQuerier.ListLoopMembers()`

#### topic/*

- `subscribe` -- requires `topic`, `commService.Subscribe()`
- `unsubscribe` -- requires `topic`, `commService.Unsubscribe()`
- `list` -- `commService.ListTopics()`
- `status` -- shows plugin name, connected status, agent ID, subscribed topics

---

### 2.7 MsgTool

**File:** `message_tool.go`
**Tool name:** `"message"`

Outbound delivery to humans (not NeboLoop).

```go
type MsgTool struct {
    appendToSession func(content string) error
    sendFrame       func(frame map[string]any) error
    platformTools   map[string]Tool
}
```

**Resources:** `owner` (always), `sms` and `notify` (platform-specific, registered via `init()` + `RegisterMessageResourceInit()`)

#### Action-to-Resource Inference

```go
var msgActionToResource = map[string]string{
    "conversations": "sms", "read": "sms",
    "alert": "notify", "speak": "notify", "dnd_status": "notify",
}
```

#### owner/notify

1. Appends text to companion chat session as assistant message via `appendToSession()`
2. Sends notification frame to web UI via `sendFrame()`:
   ```json
   {"type": "event", "method": "notification", "payload": {"content": "..."}}
   ```

#### sms/* and notify/*

Delegated to platform-specific sub-tools registered via `RegisterMessageResourceInit()`.

- `sms` actions: `send`, `conversations`, `read`, `search`
- `notify` actions: `send`, `alert`, `speak`, `dnd_status`

---

### 2.8 EventTool

**File:** `event_tool.go`
**Tool name:** `"event"`

Scheduling and time-based automation. Flat domain (no resources -- uses empty-string key).

```go
type EventTool struct {
    scheduler Scheduler
}
```

#### Resource Aliases

All map to empty resource: `reminder`, `reminders`, `routine`, `routines`, `remind`, `schedule`, `schedules`, `job`, `jobs`, `cron`, `event`, `events`, `calendar`

#### Actions

**create:**
- Input: `name`, `schedule` (cron expression for recurring), `at` (human-readable for one-time), `task_type` (`"bash"` or `"agent"`), `command`, `message`, `instructions`, `deliver` (JSON `{channel, to}`)
- Returns: `"Created schedule \"name\" (expression: ..., type: ...)"`

**list:**
- Paginates with default limit 50
- Shows name, expression, task type, enabled/disabled status

**delete/pause/resume:**
- By name. Pause calls `Disable()`, resume calls `Enable()`.

**run:**
- Manually triggers a schedule via `Trigger()`, returns output.

**history:**
- Last 10 entries for a named schedule
- Shows timestamp, success/failed status, output

---

## 3. Standalone Tools

### 3.1 GrepTool

**File:** `grep.go`

```go
type GrepTool struct {
    rgPath string
}

type GrepInput struct {
    Pattern         string
    Path            string
    Glob            string
    CaseInsensitive bool
    Context         int
    Limit           int  // default 100
}
```

#### Ripgrep Discovery

`findRipgrep()` priority:
1. Embedded binary via `ripgrep.Path()` (extracted to data dir)
2. System `rg` on PATH
3. Empty string (triggers pure Go fallback)

#### Ripgrep Execution

Flags: `--line-number`, `--no-heading`, `--color=never`, `--max-count={limit}`, optional `-i`, `-C{context}`, `--glob`

Exit code 1 = no matches (not an error).

#### Pure Go Fallback

`executeWithGo()`:
1. Compiles regex (with `(?i)` flag if case-insensitive)
2. Finds files via `findFiles()` -- skips `.`-prefixed dirs, `node_modules`, `vendor`, `__pycache__`, binary extensions (`.exe`, `.bin`, `.so`, `.dylib`, `.png`, `.jpg`, `.gif`, `.ico`, `.zip`, `.tar`, `.gz`). File limit: 10,000.
3. Parallel worker pool: `min(runtime.NumCPU(), 8, len(files))` workers
4. Workers read files line-by-line, match against regex, truncate lines at 500 chars
5. Results sorted by file discovery order then line number

#### Safety

Blocks dangerous root paths: `/`, `/usr`, `/var`, `/etc`, `/System`, `/Library`, `/Applications`, `/bin`, `/sbin`, `/opt`

---

### 3.2 VisionTool

**File:** `vision.go`

```go
type AnalyzeFunc func(ctx context.Context, imageBase64, mediaType, prompt string) (string, error)

type VisionTool struct {
    mu          sync.RWMutex
    analyzeFunc AnalyzeFunc
}
```

Provider-agnostic vision analysis. `AnalyzeFunc` is wired after provider loading via `SetAnalyzeFunc()`.

#### Image Loading

`loadImage(source string)`:
1. **Data URL:** `data:image/png;base64,...` -- extracts media type and base64 data
2. **HTTP URL:** Fetches via `http.Get()`, reads body, base64-encodes, detects media type from Content-Type or extension
3. **File path:** Reads file, detects media type from extension, base64-encodes

Supported formats: `png`, `jpg/jpeg`, `gif`, `webp`

Default prompt: `"Describe this image in detail."`

---

### 3.3 TTSTool

**File:** `tts.go`

```go
type TTSTool struct{}

type ttsInput struct {
    Text string `json:"text"`
}
```

Platform-specific system TTS:
- **macOS:** `say {text}`
- **Linux:** `espeak {text}`
- **Windows:** PowerShell `Add-Type -AssemblyName System.Speech; (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak('{text}')` (single quotes escaped)

Registered as a resource of `DesktopDomainTool` via platform-specific `init()`.

---

### 3.4 ScreenshotTool

**File:** `screenshot.go`

```go
type ScreenshotTool struct{}

type screenshotInput struct {
    Action     string  // "capture" or "see"
    Display    int     // 0 = primary, -1 = all
    Output     string  // File path (optional)
    Format     string  // "file", "base64", "both"
    App        string  // App name for window capture
    Window     string  // "frontmost" or index
    SnapshotID string  // Retrieve previous snapshot
}
```

#### capture Action

- Default format: `"file"`
- Validates display number against active displays
- Display `-1`: captures union of all display bounds
- Saves to `<data_dir>/files/` for web UI serving via `/api/v1/files/{name}`
- Returns `ImageURL` field in `ToolResult`

#### see Action

1. If `SnapshotID` provided, retrieves from `SnapshotStore`
2. Window capture: `CaptureAppWindow(ctx, app, windowIndex)`
3. Full display capture: `screenshot.CaptureRect(bounds)`
4. Gets UI tree with element bounds from accessibility system
5. `AssignElementIDs()` -- assigns IDs like `B1`, `T2`
6. `RenderAnnotations()` -- overlays numbered element IDs on screenshot
7. Stores `Snapshot` in `SnapshotStore` with ID `snap-{timestamp}`
8. Returns annotated image path + element list

**Snapshot struct:**
```go
type Snapshot struct {
    ID, App, WindowTitle string
    CreatedAt            time.Time
    RawPNG, AnnotatedPNG []byte
    Elements             map[string]*Element
    ElementOrder         []string
}
```

---

### 3.5 QuerySessionsTool

**File:** `query_sessions.go`

Cross-session reading for command-center view.

```go
type SessionQuerier interface {
    ListSessions(userID string) ([]SessionInfo, error)
    GetMessages(sessionID string, limit int) ([]SessionMessage, error)
    GetOrCreate(sessionKey, userID string) (*SessionInfo, error)
}

type QuerySessionsTool struct {
    querier SessionQuerier
}
```

Actions:
- `list` -- lists all sessions with session key and last update time
- `read` -- reads messages from a specific session (default limit 10, max 50, content truncated at 500 chars)

---

### 3.6 MemoryTool

**File:** `memory.go`

Three-tier persistent memory with prompt injection detection.

```go
const (
    MaxMemoryKeyLength   = 128
    MaxMemoryValueLength = 2048
)

const (
    LayerTacit  = "tacit"   // Long-term preferences
    LayerDaily  = "daily"   // Day-specific facts (keyed by date)
    LayerEntity = "entity"  // People, places, things
)

type MemoryTool struct {
    sqlDB         *sql.DB
    queries       *db.Queries
    embedder      *embeddings.Service
    searcher      *embeddings.HybridSearcher
    currentUserID string
    sanitize      bool
}
```

#### Input Sanitization

`sanitizeMemoryKey(key)`:
- Trims whitespace, strips control chars, truncates to 128 chars

`sanitizeMemoryValue(value)`:
- Trims whitespace, strips control chars, truncates to 2048 chars
- **Prompt injection detection** via `instructionPatterns` regex:
  - `ignore (all)? previous instructions`
  - `ignore (all)? above`
  - `disregard (all)? previous`
  - `you are now`
  - `new instructions?:`
  - `system:` / `<system>` / `<system-prompt>`
  - `IMPORTANT: you must`
  - `override (all)? previous`
  - `forget (all)? previous`
  - `act as (if|though) you`
  - `pretend you are`
  - `from now on,? you`

Actions: `store`, `recall`, `search`, `list`, `delete`, `clear`

Uses hybrid search (vector embeddings + SQLite FTS) when embedder is available.

---

## 4. Utilities

### 4.1 Web Sanitize

**File:** `web_sanitize.go`

HTML content extraction and chunking for the web fetch tool.

```go
const defaultChunkSize = 50000
```

#### ExtractVisibleText

Parses HTML and returns only visible text. Non-HTML passes through unchanged.

**Skip elements** (entire subtree discarded): `script`, `style`, `noscript`, `svg`, `math`, `template`, `iframe`, `object`, `embed`

**Hidden detection:**
- `aria-hidden="true"`
- `hidden` attribute
- Inline style patterns: `display:none`, `visibility:hidden`, `opacity:0`, `font-size:0`, `left/top: -9999+`

**Text formatting:**
- Block elements get newlines
- Headings get markdown `#` prefix
- List items get bullet `"* "`
- Whitespace collapse: runs of spaces -> single space, 3+ newlines -> 2

#### ChunkText

Splits at paragraph boundaries (`\n\n`), falls back to newline, then hard-cuts at `chunkSize`. Returns requested chunk (0-indexed) and total chunk count.

#### FormatFetchResult

Prepends HTTP header: `HTTP {status}\nContent-Type: {ct}\nOriginal-Size: {bytes} bytes\n`. Adds `Chunk: N/M` if multi-chunk.

---

### 4.2 Scheduler Interface

**Files:** `scheduler.go`, `scheduler_manager.go`

```go
type Scheduler interface {
    Create(ctx, item) (*ScheduleItem, error)
    Get(ctx, name) (*ScheduleItem, error)
    List(ctx, limit, offset, enabledOnly) ([]ScheduleItem, int64, error)
    Update(ctx, item) (*ScheduleItem, error)
    Delete(ctx, name) error
    Enable(ctx, name) (*ScheduleItem, error)
    Disable(ctx, name) (*ScheduleItem, error)
    Trigger(ctx, name) (string, error)
    History(ctx, name, limit, offset) ([]ScheduleHistoryEntry, int64, error)
    SetTriggerHandler(fn func(ScheduleTriggerEvent))
    Close() error
}
```

**ScheduleItem fields:** ID, Name, Expression, TaskType, Command, Message, Instructions, Deliver, Enabled, LastRun, NextRun, RunCount, LastError, CreatedAt, Metadata

**CronScheduler:** Adapter wrapping `CronTool` (which implements Tool interface and conflicts on `Name()`/`Close()` methods). Prefixes all method names with `Scheduler` (e.g., `SchedulerCreate`, `SchedulerGet`).

**SchedulerManager:** Delegates to app scheduler if available, falls back to built-in CronScheduler. Thread-safe with `sync.RWMutex`.

---

### 4.3 Types and Helpers

**File:** `types.go`

#### CommService Interface

```go
type CommService interface {
    Send(ctx, to, topic, content, msgType string) error
    Subscribe(ctx, topic string) error
    Unsubscribe(ctx, topic string) error
    ListTopics() []string
    PluginName() string
    IsConnected() bool
    CommAgentID() string
}
```

#### LoopQuerier Interface

```go
type LoopQuerier interface {
    ListLoops(ctx) ([]LoopInfo, error)
    GetLoop(ctx, loopID) (*LoopInfo, error)
    ListLoopMembers(ctx, loopID) ([]MemberInfo, error)
    ListChannelMembers(ctx, channelID) ([]MemberInfo, error)
    ListChannelMessages(ctx, channelID string, limit int) ([]MessageInfo, error)
}
```

#### Data Types

```go
type LoopChannelInfo struct { ChannelID, ChannelName, LoopID, LoopName string }
type LoopInfo struct { ID, Name, Description string; MemberCount int }
type MemberInfo struct { BotID, BotName, Role string; IsOnline bool }
type MessageInfo struct { ID, From, Content, CreatedAt string }
type AskWidget struct { Type, Label string; Options []string; Default string }
type AskCallback func(ctx, requestID, prompt string, widgets []AskWidget) (string, error)
type WorkTask struct { ID, Subject, Status string; CreatedAt time.Time }
```

#### Helpers

`buildAgentSystemPrompt(agentType, task)`:
- Base prompt for all sub-agents: focus on task, work independently, report concisely
- `"explore"` -- search codebases, understand patterns, do NOT modify files
- `"plan"` -- analyze task, break into steps, identify files, do NOT implement
- default -- generic task execution

`truncateForDescription(prompt)` -- first line or first 50 chars (truncated at 47 + "...")

`openBrowserURL(targetURL)` -- cross-platform: `open` (macOS), `rundll32` (Windows), `xdg-open` (Linux)

`registryAdapter` -- wraps `Registry` to implement `orchestrator.ToolExecutor` interface.
