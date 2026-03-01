# Apps Platform — SME Deep Dive

> **Scope:** This document covers the app platform only. For skills, see the skills sections of `APPS_AND_SKILLS.md`.
> **Verified against:** source code on 2026-02-28.

---

## 1. Architecture Overview

Apps are sandboxed native binaries that extend Nebo's capabilities via gRPC over Unix domain sockets. NeboLoop distributes apps as signed `.napp` packages; Nebo verifies signatures, launches binaries in a sanitized environment, and bridges gRPC services into Nebo's internal interfaces through adapter types.

```
NeboLoop Store
  │
  ▼  .napp download
┌─────────────────────────────────────────────────┐
│  <data_dir>/apps/<app-id>/                      │
│    manifest.json    ← identity + capabilities   │
│    binary (or app)  ← native executable         │
│    signatures.json  ← ED25519 sigs              │
│    SKILL.md         ← required skill definition │
│    app.sock         ← Unix domain socket (gRPC) │
│    data/            ← sandboxed app storage     │
│    logs/            ← per-app stdout/stderr     │
│    .pid             ← PID file for orphan kill  │
│    ui/              ← optional static assets    │
└─────────────────────────────────────────────────┘
         │ gRPC (unix://)
         ▼
┌─────────────────────────────────────────────────┐
│  AppRegistry                                     │
│    Adapters: Gateway → ai.Provider               │
│              Tool    → SkillDomainTool           │
│              Comm    → CommPluginManager          │
│              Channel → channelAdapters map        │
│              UI      → uiApps map (HTTP proxy)   │
│              Schedule→ scheduleAdapter (single)   │
│    Hooks:    HookDispatcher (actions + filters)   │
│    Supervisor: health check + auto-restart        │
│    Watcher:    fsnotify for hot-reload            │
└─────────────────────────────────────────────────┘
```

**Key principle:** Deny by default. Apps declare what they provide (capabilities) and what they need (permissions). Nebo validates both at manifest load time and enforces permissions at capability registration time.

---

## 2. File Inventory

| File | Lines | Purpose |
|------|-------|---------|
| `manifest.go` | 326 | `AppManifest` struct, `LoadManifest`, `ValidateManifest`, permission taxonomy (25 prefixes), capability constants |
| `registry.go` | 988 | `AppRegistry`: `DiscoverAndLaunch`, `launchAndRegister`, capability registration/deregistration, `Quarantine`, `Sideload`/`Unsideload`, `AppCatalog`, revocation sweep, OAuth token push |
| `runtime.go` | 543 | `Runtime`: `Launch`, `Stop`, `StopAll`, `FindBinary`, watcher suppression, per-app launch mutex, socket wait |
| `adapter.go` | 495 | 4 adapters: `GatewayProviderAdapter`, `AppToolAdapter`, `AppCommAdapter`, `AppChannelAdapter` + proto converters |
| `schedule_adapter.go` | 249 | `AppScheduleAdapter`: full CRUD, trigger stream, history |
| `sandbox.go` | 276 | `SandboxConfig`, `sanitizeEnv` (6 `NEBO_APP_*` + 6 system vars), `validateBinary` (magic bytes, shebang rejection), log rotation |
| `signing.go` | 292 | `SigningKeyProvider` (24h cache), `RevocationChecker` (1h cache), `VerifyAppSignatures`, `LoadSignatures` |
| `napp.go` | 202 | `ExtractNapp`: path traversal protection, symlink rejection, allowlist, size limits, binary format check |
| `supervisor.go` | 194 | `Supervisor`: 15s tick, exponential backoff (10s→5min), max 5/hr, deregisters capabilities on budget exhaustion |
| `watcher.go` | 181 | fsnotify watcher: Create/Write/Remove events, 500ms debounce, watcher suppression coordination |
| `install.go` | 254 | `HandleInstallEvent`, `handleUpdate` (permission diff), `DownloadAndExtractNapp` (600MB max download) |
| `envelope.go` | 49 | `ChannelEnvelope`: UUID v7 message IDs, sender, attachments, actions, `platform_data` |
| `process_unix.go` | 58 | `setProcGroup`, `killProcGroup`/`killProcGroupTerm`, `killOrphanGroup`, `isProcessAlive` |
| `orphan_unix.go` | 44 | `killOrphansByBinary`: pgrep -f scan for orphans |
| `process_windows.go` | ~50 | Windows equivalents using `taskkill` and job objects |
| `orphan_windows.go` | ~40 | Windows orphan cleanup |
| `hooks.go` | 269 | `HookDispatcher`: registration, priority dispatch, timeout (500ms), circuit breaker (3 failures) |
| `hooks_test.go` | 379 | 12 unit tests: priority ordering, filter chaining, override, circuit breaker, deregistration, etc. |

**Agent tool:** `internal/agent/tools/app_tool.go` — 7 actions: list, launch, stop, settings, browse, install, uninstall.

---

## 3. Manifest (`manifest.go`)

### Structure

```go
type AppManifest struct {
    ID             string             `json:"id"`
    Name           string             `json:"name"`
    Version        string             `json:"version"`
    Description    string             `json:"description,omitempty"`
    Runtime        string             `json:"runtime"`           // "local" or "remote"
    Protocol       string             `json:"protocol"`          // "grpc" only
    Signature      ManifestSignature  `json:"signature,omitempty"`
    StartupTimeout int                `json:"startup_timeout,omitempty"` // 0-120s, default 10s
    Provides       []string           `json:"provides"`          // at least one required
    Permissions    []string           `json:"permissions"`
    Overrides      []string           `json:"overrides,omitempty"` // hook names app can fully override
    OAuth          []OAuthRequirement `json:"oauth,omitempty"`
}
```

### Validation Rules (`ValidateManifest`)

- Required fields: `id`, `name`, `version`, `provides` (non-empty)
- Protocol must be `"grpc"` or empty
- Runtime must be `"local"`, `"remote"`, or empty
- `startup_timeout`: 0–120 seconds
- Every capability in `provides` must pass `isValidCapability()`
- Every permission in `permissions` must pass `isValidPermission()`
- Every entry in `overrides` must be a valid hook name (in `ValidHookNames`) and have a matching `hook:<name>` permission

### Capability Types

| Capability | Constant | Description |
|------------|----------|-------------|
| `gateway` | `CapGateway` | AI provider bridge (requires `network:*` permission) |
| `vision` | `CapVision` | Vision/image analysis |
| `browser` | `CapBrowser` | Web browser automation |
| `comm` | `CapComm` | Inter-agent communication (requires `comm:*` permission) |
| `ui` | `CapUI` | Structured template UI |
| `schedule` | `CapSchedule` | Cron/scheduling (requires `schedule:*` permission) |
| `hooks` | `CapHooks` | Actions & filters — intercept/transform built-in behavior (requires `hook:*` permissions) |
| `tool:{name}` | `CapPrefixTool` | Custom tool (parameterized) |
| `channel:{name}` | `CapPrefixChannel` | Messaging channel (requires `channel:*` permission) |

### Permission Taxonomy (26 prefixes)

| Category | Prefixes | Suffix Type |
|----------|----------|-------------|
| **Storage & Config** | `network:`, `filesystem:`, `settings:`, `capability:` | network=flexible, others=strict |
| **Agent Core** | `memory:`, `session:`, `context:` | strict |
| **Execution** | `tool:`, `shell:`, `subagent:`, `lane:` | strict |
| **Communication** | `channel:`, `comm:`, `notification:` | strict |
| **Knowledge** | `embedding:`, `skill:`, `advisor:` | strict |
| **AI** | `model:`, `mcp:` | strict |
| **Storage** | `database:`, `storage:` | strict |
| **Hooks** | `hook:` | flexible (any valid hook name as suffix) |
| **System** | `schedule:`, `voice:`, `browser:`, `oauth:`, `user:` | oauth=flexible, others=strict |

**Suffix validation rules:**
- `*` wildcard is always valid for any prefix
- **Flexible prefixes** (`network:`, `oauth:`, `settings:`): suffixes validated as lowercase alphanumeric + `.`, `-`, `:`, `_` (covers hostnames, provider names, ports)
- **Strict prefixes** (all others): suffix must match an exact list (e.g., `filesystem:read`, `filesystem:write`)

---

## 4. Lifecycle

### 4.1 Launch Sequence (`runtime.go:Launch`)

```
LoadManifest(appDir)
  → Per-app mutex lock (prevents duplicate launches)
  → FindBinary (search: "binary", "app", then tmp/*.exec)
  → Revocation check (if revChecker != nil)
  → Signature verification (skip for symlinks/sideloaded)
  → validateBinary (Lstat for symlinks, size, exec bit, magic bytes)
  → Clean stale socket
  → Create data/ directory
  → Set up per-app log writers (tee to file + prefixed stderr)
  → exec.Command with sanitized env + process group isolation
  → cmd.Start()
  → Write .pid file (orphan cleanup on next startup)
  → Reaper goroutine (calls cmd.Wait() to prevent zombies)
  → waitForSocket (exponential backoff: 50ms→500ms, max 10s or startup_timeout)
  → chmod socket 0600
  → gRPC dial (unix://, insecure credentials, optional inspector interceptors)
  → Create capability-specific gRPC clients based on manifest.provides (incl. HookClient for "hooks" cap)
  → HealthCheck via first available client
  → Store in processes map (replaces old process if exists)
```

### 4.2 Stop Sequence (`runtime.go:stop`)

```
Close gRPC connection
  → SIGTERM to -PID (entire process group)
  → Wait for reaper goroutine (up to 2s)
  → If still alive: SIGKILL to -PID, then wait for reaper
  → Close per-app log files
  → Remove socket and .pid file
```

### 4.3 Registration Flow (`registry.go:launchAndRegister`)

After `Runtime.Launch()` returns a running `AppProcess`:

1. **DB registration** — Creates `plugin_registry` row with metadata (app_id, provides, permissions, runtime)
2. **Capability registration** — For each declared capability:
   - **Gateway**: Requires `network:*` permission. Creates `GatewayProviderAdapter`, adds to providers slice. Auto-injects NeboLoop JWT if `user:token` permission.
   - **Tool/Vision/Browser**: Creates `AppToolAdapter`, registers through `SkillDomainTool` with slug = `Slugify(manifest.Name)` and loads `SKILL.md`. Fallback: direct tool registry.
   - **Comm**: Requires `comm:*` permission. Creates `AppCommAdapter`, registers with `CommPluginManager`, tracks in `commNames` map.
   - **Channel**: Requires `channel:*` permission. Creates `AppChannelAdapter`, wires lazy message handler.
   - **UI**: Stored in `uiApps` map, proxied via `HandleRequest`.
   - **Schedule**: Requires `schedule:*` permission. One app max (single adapter).
   - **Hooks**: Queries app via `ListHooks()`, validates each subscription has matching `hook:<name>` permission, registers with `HookDispatcher`.
3. **Settings hot-reload** — Registers as `Configurable` in plugin store
4. **DB status update** — Sets `connection_status = "connected"`

### 4.4 Deregistration (`registry.go:deregisterCapabilities`)

Called on: uninstall, quarantine, unsideload, restart budget exhaustion.

| Capability | Action |
|------------|--------|
| Gateway | Filter from providers slice by manifest.ID |
| Tool/Vision/Browser | `skillTool.Unregister(slug)` or `toolReg.Unregister(slug)` |
| Comm | `commMgr.Unregister(commName)`, delete from commNames map |
| Channel | Delete from channelAdapters map |
| Schedule | Nil out scheduleAdapter |
| UI | Delete from uiApps map |
| Hooks | `hookDispatcher.UnregisterApp(manifest.ID)` |
| All | `pluginStore.DeregisterConfigurable(manifest.Name)` |

### 4.5 Uninstall (`registry.go:Uninstall`)

```
Deregister capabilities → Stop app → Delete DB row → os.RemoveAll(appDir)
  → Clean up .pending and .updating directories
```

### 4.6 Quarantine (`registry.go:Quarantine`)

```
Deregister capabilities → Stop app → Remove binary ("binary" + "app" + socket)
  → Write .quarantined marker file → Notify UI via callback
```

Preserves `data/` and `logs/` for forensic analysis. Quarantined apps are skipped by `DiscoverAndLaunch`.

---

## 5. Sandbox (`sandbox.go`)

### Environment Sanitization

Apps receive **only** these environment variables:

| Variable | Source |
|----------|--------|
| `NEBO_APP_DIR` | App directory path |
| `NEBO_APP_SOCK` | Unix socket path |
| `NEBO_APP_ID` | From manifest |
| `NEBO_APP_NAME` | From manifest |
| `NEBO_APP_VERSION` | From manifest |
| `NEBO_APP_DATA` | `{appDir}/data` |
| `PATH` | From parent (allowlisted) |
| `HOME` | From parent (allowlisted) |
| `TMPDIR` | From parent (allowlisted) |
| `LANG` | From parent (allowlisted) |
| `LC_ALL` | From parent (allowlisted) |
| `TZ` | From parent (allowlisted) |

Everything else is stripped. This prevents leaking `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `JWT_SECRET`, etc.

### Binary Validation (`validateBinary`)

Checks performed in order:
1. **Lstat** (not Stat) — detects symlinks
2. **Symlink rejection** — binary must not be a symlink
3. **Regular file check** — rejects devices, pipes, etc.
4. **Executable bit** — `mode & 0111 != 0` (skipped on Windows)
5. **Size limit** — `MaxBinarySizeMB` (default: 500MB)
6. **Format validation** — magic bytes check:

| Format | Magic Bytes | Accepted On |
|--------|-------------|-------------|
| ELF | `\x7fELF` | All platforms |
| Mach-O 32-bit | `\xfe\xed\xfa\xce` | All platforms |
| Mach-O 64-bit | `\xcf\xfa\xed\xfe` | All platforms |
| Mach-O Fat | `\xca\xfe\xba\xbe` | All platforms |
| PE (MZ) | `\x4d\x5a` | All platforms |
| Shebang (`#!`) | `\x23\x21` | **REJECTED** — scripts not allowed |

### Logging

- **Default** (`LogToFile: true`): Output goes to `{appDir}/logs/stdout.log` and `stderr.log`
- Tee'd to Nebo's stderr with `[app:{id}]` prefix for real-time debugging
- **Log rotation**: 2MB max per file, one backup (`.log.1`)
- `prefixWriter`: line-buffered writer that prepends `[app:{id}]` to each line

### Process Isolation (Unix)

- `Setpgid: true` — app runs in its own process group (PID == PGID)
- Kill: `syscall.Kill(-PID, signal)` kills entire group including children
- Stop: SIGTERM → 2s wait → SIGKILL
- Orphan cleanup: 2-phase — PID file check + `pgrep -f` scan

---

## 6. Signing & Revocation (`signing.go`)

### Signature Verification

**`signatures.json`** (separate file from manifest):
```json
{
  "key_id": "...",
  "algorithm": "ed25519",
  "binary_sha256": "hex-encoded hash",
  "binary_signature": "base64-encoded ED25519 sig over raw binary bytes",
  "manifest_signature": "base64-encoded ED25519 sig over manifest.json bytes"
}
```

**Verification steps** (`VerifyAppSignatures`):
1. Load `signatures.json`
2. Verify `key_id` matches server's current key
3. Decode ED25519 public key from base64
4. Verify manifest signature: `ed25519.Verify(pubKey, manifestBytes, manifestSig)`
5. Verify binary SHA256: `sha256.Sum256(binaryBytes) == sigs.BinarySHA256`
6. Verify binary signature: `ed25519.Verify(pubKey, binaryBytes, binarySig)`

**Skip conditions:**
- Sideloaded apps (detected via `os.Lstat` → `ModeSymlink`)
- Dev mode (`keyProvider == nil` when `NeboLoopURL` is not configured)
- Signing key unavailable (logged as warning, verification skipped)

### Signing Key Provider

- Fetches from `{NeboLoopURL}/api/v1/apps/signing-key`
- **Cache TTL: 24 hours**, thread-safe
- `Refresh()` for force-fetch on verification failure
- HTTP client timeout: 5s
- Response size limited to 64KB

### Revocation Checker

- Fetches from `{NeboLoopURL}/api/v1/apps/revocations`
- **Cache TTL: 1 hour**, thread-safe
- Double-checked locking pattern on refresh
- Response size limited to 1MB
- `StartRevocationSweep`: 1-hour ticker goroutine checks all running apps

---

## 7. .napp Package Format (`napp.go`)

### Structure

A `.napp` is a tar.gz archive containing:

| File | Required | Max Size | Description |
|------|----------|----------|-------------|
| `manifest.json` | Yes | 1MB | App manifest |
| `binary` or `app` | Yes | 500MB | Native compiled executable |
| `signatures.json` | Yes | 1MB | ED25519 signatures |
| `SKILL.md` | Yes | 1MB | Skill definition (required since napp.go:131) |
| `ui/*` | No | 5MB each | Static UI assets |

### Security Measures (`ExtractNapp`)

1. **Path traversal protection** — rejects `../` and absolute paths
2. **Symlink rejection** — `TypeSymlink` and `TypeLink` both rejected
3. **Double path escape check** — target must be within `destDir`
4. **File allowlist** — only known files accepted, unexpected files rejected
5. **Size enforcement** — `io.Copy` with `LimitReader(maxSize+1)` to detect lying headers
6. **Binary format validation** — `validateBinaryFormat` runs at extraction time
7. **Cleanup on rejection** — `os.RemoveAll(destDir)` if format check fails

### Download (`DownloadAndExtractNapp`)

- Max download size: 600MB
- Downloads to temp file first, then extracts
- Temp file cleaned up via `defer os.Remove`

---

## 8. Supervisor (`supervisor.go`)

### Configuration

| Parameter | Value |
|-----------|-------|
| Health check interval | 15 seconds |
| Max restarts per window | 5 |
| Window duration | 1 hour |
| Min backoff | 10 seconds |
| Max backoff | 5 minutes |
| Backoff progression | 10s → 20s → 40s → 80s → 160s → 300s (capped) |

### Health Check Logic

```
For each running app:
  1. Check backoff — skip if still in backoff period
  2. Reset window — if > 1 hour since window start
  3. Budget check — skip if restartCount >= 5
  4. OS-level: isProcessAlive(pid) — catches crashes
  5. gRPC-level: HealthCheck with 5s timeout — catches hangs
  6. If either fails → restart
```

### Restart Flow

1. Increment `restartCount`, calculate exponential backoff
2. If over budget: `deregisterCapabilities` and give up
3. Suppress watcher for 30s (prevents redundant restart)
4. Call `registry.restartApp(ctx, appDir)` → `launchAndRegister`
5. Clear watcher suppression

---

## 9. File Watcher (`watcher.go`)

### Events Handled

| Event | Trigger | Action |
|-------|---------|--------|
| **Create (dir)** | New app directory in `appsDir` | Add to watcher, delay 500ms, launch if manifest exists |
| **Create (manifest.json)** | manifest.json in subdirectory | Launch if not already running |
| **Create/Write (binary/app)** | Binary rebuilt | Debounced restart (500ms) |
| **Write (manifest.json)** | Manifest modified | Debounced restart (500ms) |
| **Remove (top-level)** | App directory removed | Stop app, remove from watcher |

### Coordination with Supervisor

- `SuppressWatcher(appID, duration)`: marks an app as being restarted by supervisor/registry
- `IsWatcherSuppressed(appID)`: watcher checks before restarting — prevents double-launch
- Suppression auto-expires as a safety net

---

## 10. Install & Update (`install.go`)

### Install Event Types

Events from NeboLoop comms SDK (`neboloopsdk.InstallEvent`), type prefix `app_` is stripped:

| Type | Handler | Description |
|------|---------|-------------|
| `installed` | `handleInstall` | Download, extract, launch |
| `updated` | `handleUpdate` | Permission diff, preserve data, swap |
| `uninstalled` | `handleUninstall` | Stop, remove directory |
| `revoked` | `handleRevoke` | Quarantine |

### Update Permission Diff

When a new version arrives:
1. Load old manifest's permissions
2. Download new version to `.updating/` temp dir
3. Compute `permissionDiff(old, new)` — permissions in new but not in old
4. **New permissions found**: Stage to `.pending/`, relaunch old version, require user approval
5. **No new permissions**: Auto-update:
   - Preserve `data/` and `logs/` via rename into new dir
   - Remove old `appDir`, rename `.updating/` → `appDir`
   - Relaunch

### Direct Install (`InstallFromURL`)

For HTTP install handler (immediate, no comms event):
1. Download to temp dir inside `appsDir` (same filesystem for atomic rename)
2. Validate manifest
3. If already installed and running: no-op
4. If already installed but stopped: launch
5. Otherwise: rename temp → permanent, launch

---

## 11. Adapters (`adapter.go`, `schedule_adapter.go`)

### GatewayProviderAdapter

Bridges `pb.GatewayServiceClient` → `ai.Provider` interface.

- `Stream()`: Converts `ai.ChatRequest` → `pb.GatewayRequest`, streams events back as `ai.StreamEvent`
- **Token filtering**: Only apps with `user:token` permission receive the JWT in `UserContext.Token`
- All apps receive `user_id` and `plan` as convenience fields
- Event types: text, tool_call, thinking, error, done

### AppToolAdapter

Bridges `pb.ToolServiceClient` → `tools.Tool` interface.

- Queries app at creation time for: Name, Description, Schema, RequiresApproval
- `Execute()`: Forwards JSON input, returns `ToolResult{Content, IsError}`
- Registered through `SkillDomainTool` with `SKILL.md` content

### AppCommAdapter

Bridges `pb.CommServiceClient` → `comm.CommPlugin` interface.

- Full comm lifecycle: Connect, Disconnect, Send, Subscribe, Unsubscribe, Register, Deregister
- Background goroutine for `Receive` stream (server-sent messages)
- `SetMessageHandler` for inbound message callback

### AppChannelAdapter

Bridges `pb.ChannelServiceClient` → channel interface.

- `Connect()` starts background `Receive` stream for inbound messages
- `Send()` for outbound messages
- Lazy handler wiring: `SetMessageHandler` can be called after `DiscoverAndLaunch`
- Message handler closure reads `onChannelMsg` at call time (not at registration time)

### AppScheduleAdapter

Bridges `pb.ScheduleServiceClient` → `tools.Scheduler` interface.

- Full CRUD: Create, Get, List, Update, Delete
- Enable/Disable toggle
- Manual `Trigger` execution
- `History` with pagination
- `SetTriggerHandler` starts background `Triggers` stream

---

## 12. Agent Tool (`tools/app_tool.go`)

The `AppTool` provides the agent-facing `app()` tool with 7 actions:

| Action | Requires | Backend |
|--------|----------|---------|
| `list` | `AppManager` | Local — lists installed apps with status |
| `launch` | `AppManager` + `id` | Local — launches by ID |
| `stop` | `AppManager` + `id` | Local — stops by ID |
| `settings` | — | Returns "not yet implemented" |
| `browse` | NeboLoop client | Store — list/search/detail, supports query/category/page |
| `install` | NeboLoop client + `id` | Store — installs from NeboLoop |
| `uninstall` | NeboLoop client + `id` | Store — uninstalls via NeboLoop |

**Import cycle avoidance**: `AppManager` interface defined in `tools/app_tool.go`, implemented by `apps.AppRegistry`. Set via `SetAppManager()` late binding.

---

## 13. HTTP API Endpoints

### App UI Routes (protected)

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| GET | `/apps/ui` | `appui.ListUIAppsHandler` | List all apps with UI capability |
| POST | `/apps/{id}/ui/open` | `appui.OpenAppUIHandler` | Open app UI |
| GET | `/apps/{id}/ui/*` | `appui.AppStaticHandler` | Serve app static assets |
| * | `/apps/{id}/api/*` | `appui.AppAPIProxyHandler` | Proxy API calls to app via gRPC |

### App OAuth Routes (protected)

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| GET | `/apps/{appId}/oauth/{provider}/connect` | `appoauth.ConnectHandler` | Initiate OAuth flow for app |
| GET | `/apps/oauth/callback` | `appoauth.CallbackHandler` | OAuth callback |
| GET | `/apps/{appId}/oauth/grants` | `appoauth.GrantsHandler` | List OAuth grants |
| DELETE | `/apps/{appId}/oauth/{provider}` | `appoauth.DisconnectHandler` | Revoke OAuth grant |

### Store Routes (protected)

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| GET | `/store/apps` | `plugins.ListStoreAppsHandler` | Browse store |
| GET | `/store/apps/{id}` | `plugins.GetStoreAppHandler` | App detail |
| GET | `/store/apps/{id}/reviews` | `plugins.GetStoreAppReviewsHandler` | App reviews |
| POST | `/store/apps/{id}/install` | `plugins.InstallStoreAppHandler` | Install from store |
| DELETE | `/store/apps/{id}/install` | `plugins.UninstallStoreAppHandler` | Uninstall |

---

## 14. Sideloading (Developer Workflow)

`Sideload(ctx, projectPath)`:

1. Verify path exists and is a directory
2. Validate `manifest.json`
3. Run `make build` if Makefile exists
4. Verify binary exists via `FindBinary`
5. Check for collision at symlink target:
   - Existing symlink to same path → just ensure launched
   - Existing symlink to different path → remove old, create new
   - Regular directory → reject ("not a sideloaded app")
6. Create symlink: `os.Symlink(projectPath, appsDir/manifest.ID)`
7. Launch immediately (don't wait for watcher)
8. On launch failure: remove symlink, return error

`Unsideload(appID)`:

1. Verify target is a symlink (safety: never delete real app dirs)
2. Deregister capabilities
3. Stop app
4. Remove symlink

**Signature verification is skipped for sideloaded apps** (detected via `os.Lstat` + `ModeSymlink` check in `runtime.go:167`).

---

## 15. System Prompt Integration

`AppCatalog()` generates markdown injected into the agent's system prompt:

```markdown
## Installed Apps

- **Calendar App** (com.example.calendar) — Manage calendar events. Provides: tool:calendar, ui. Status: running.
- **Janus Gateway** (janus-gateway) — AI gateway proxy. Provides: gateway. Status: running.
```

This makes the agent aware of installed apps and their capabilities.

---

## 16. Settings Hot-Reload

`appConfigurable.OnSettingsChanged(settings)`:

Tries each gRPC client in order until one succeeds:
1. GatewayClient.Configure
2. ToolClient.Configure
3. CommClient.Configure
4. ChannelClient.Configure
5. UIClient.Configure
6. ScheduleClient.Configure
7. HookClient (health check only — hooks don't have a Configure RPC)

**OAuth token push** (`PushOAuthTokens`): Implements `broker.AppTokenReceiver`. Wraps settings change with token data.

**Auto-configure user token** (`autoConfigureUserToken`): Reads NeboLoop JWT from `auth_profiles`, stores as app's `"token"` setting via plugin store. Only for apps with `user:token` permission.

---

## 17. Proto Definitions

Located in `proto/apps/v0/`:

| File | Services |
|------|----------|
| `gateway.proto` | `GatewayService`: Stream, HealthCheck, Configure |
| `tool.proto` | `ToolService`: Name, Description, Schema, RequiresApproval, Execute, HealthCheck, Configure |
| `comm.proto` | `CommService`: Name, Version, Connect, Disconnect, IsConnected, Send, Receive (stream), Subscribe, Unsubscribe, Register, Deregister, HealthCheck, Configure |
| `channel.proto` | `ChannelService`: ID, Connect, Disconnect, Send, Receive (stream), HealthCheck, Configure |
| `ui.proto` | `UIService`: HandleRequest, HealthCheck, Configure |
| `schedule.proto` | `ScheduleService`: Create, Get, List, Update, Delete, Enable, Disable, Trigger, History, Triggers (stream), HealthCheck |
| `hooks.proto` | `HookService`: ApplyFilter, DoAction, ListHooks, HealthCheck |
| `common.proto` | Shared types: Empty, HealthCheckRequest/Response, SettingsMap, etc. |

---

## 18. gRPC Inspector

`inspector.New(1024)` — Always-on ring buffer with zero-cost fast path when no subscribers.

- Unary + stream interceptors record all gRPC traffic per app
- Used for developer tooling / debugging
- Available via `AppRegistry.Inspector()`

---

## 19. Hook System (`hooks.go`)

WordPress-style actions and filters that let apps intercept and transform Nebo's built-in behavior at defined hook points.

### Architecture

```
App (implements HookService)
  ↕ gRPC (unix://)
HookDispatcher (internal/apps/hooks.go)
  ↕ called from integration points
Runner / Registry / BotTool / Prompt Builder
```

**No adapter layer** — unlike other capabilities, hooks don't need a bridge adapter. The `HookDispatcher` calls apps directly via `pb.HookServiceClient` and is injected into Nebo subsystems via a `HookDispatcher` interface (defined in `tools/registry.go` to avoid import cycles).

### HookDispatcher Interface (in `tools/registry.go`)

```go
type HookDispatcher interface {
    ApplyFilter(ctx context.Context, hook string, payload []byte) ([]byte, bool)
    DoAction(ctx context.Context, hook string, payload []byte)
    HasSubscribers(hook string) bool
}
```

This interface is satisfied by `apps.HookDispatcher` via Go's structural typing. Set via late-binding setters: `Registry.SetHookDispatcher()`, `Runner.SetHookDispatcher()`, `BotTool.SetHookDispatcher()`.

### Two Hook Types

| Type | gRPC RPC | Behavior | Return |
|------|----------|----------|--------|
| **Filter** | `ApplyFilter` | Data flows through, each filter receives previous output | Modified payload + `handled` bool |
| **Action** | `DoAction` | Fire-and-forget side effects | Nothing (Empty) |

### Hook Points (10 total)

| Hook Name | Type | Integration Point | Payload |
|-----------|------|-------------------|---------|
| `tool.pre_execute` | Filter | `tools/desktop_queue.go:executeTool` — before approval check | `{"tool","input"}` |
| `tool.post_execute` | Filter | `tools/desktop_queue.go:executeTool` — after tool execution | `{"tool","input","result"}` |
| `message.pre_send` | Filter | `runner/runner.go` — before `provider.Stream()` | `{"system_prompt"}` |
| `message.post_receive` | Filter | `runner/runner.go` — after stream completes | `{"response_text"}` |
| `memory.pre_store` | Filter | `tools/bot_tool.go:handleMemory` — before embeddings write | `{"key","value","layer"}` |
| `memory.pre_recall` | Filter | `tools/bot_tool.go:handleMemory` — before embeddings query | `{"query"}` |
| `session.message_append` | Action | `runner/runner.go` — after `AppendMessage` | `{"session_id","role","content"}` |
| `prompt.system_sections` | Filter | `runner/prompt.go:BuildStaticPrompt` — before joining sections | `{"sections":[...]}` |
| `steering.generate` | Filter | `runner/runner.go` — after built-in steering generators | `{"messages":[...]}` |
| `response.stream` | Filter | Declared valid but **not wired** — per-chunk 500ms gRPC would kill streaming perf | `{"event":{...}}` |

### Priority Ordering

- Numeric priority, lower = runs first (same as WordPress)
- Default priority: 10 (when manifest declares 0)
- Multiple apps on same hook: chained in priority order
- `sort.Slice` after each registration to maintain order

### Override Mechanism

When a filter returns `handled: true`:
1. No further filters in the chain are called
2. Nebo skips the built-in implementation
3. The filter's response payload is used as the final result

**Manifest requirements:**
- `overrides` array must list the hook name
- `permissions` must include matching `hook:<hookname>`
- `ValidateManifest()` enforces both constraints

### Timeout & Circuit Breaker

| Parameter | Value |
|-----------|-------|
| Hook timeout | 500ms (`hookTimeout`) |
| Circuit breaker threshold | 3 consecutive failures (`circuitBreakerThreshold`) |
| Recovery | Disabled until Nebo restart |
| Success behavior | Resets failure counter to 0 |

**Failure flow:**
```
Call with 500ms context timeout
  → Timeout or error → recordFailure(appID) → failures[appID]++
  → If failures >= 3 → disabled[appID] = true → all hooks for app skipped
  → On success → recordSuccess(appID) → failures[appID] = 0
```

### Fast-Path Optimization

All integration points check `HasSubscribers(hook)` before marshaling JSON. When no apps have hooks registered, the overhead is a single RLock + map lookup — effectively zero cost.

### Wiring (cmd/nebo/agent.go)

```go
hookDispatcher := appRegistry.HookDispatcher()
registry.SetHookDispatcher(hookDispatcher)       // tools
r.SetHookDispatcher(hookDispatcher)              // runner
if botTool := registry.GetBotTool(); botTool != nil {
    botTool.SetHookDispatcher(hookDispatcher)    // memory hooks
}
```

### Registration Flow

In `launchAndRegister` when capability is `CapHooks`:
1. Call `proc.HookClient.ListHooks()` to get app's subscriptions
2. For each `HookRegistration`:
   - Check `CheckPermission(manifest, "hook:"+reg.Hook)` — skip if denied
   - Call `hookDispatcher.Register(manifest.ID, reg, proc.HookClient)`
3. Register validates hook name against `ValidHookNames`, rejects unknown hooks

### Tests (12)

| Test | Coverage |
|------|----------|
| `PriorityOrdering` | 3 apps with priorities 5/10/20 run in correct order |
| `FilterChaining` | Output of filter A feeds as input to filter B |
| `Override` | `handled: true` short-circuits chain, skips remaining filters |
| `CircuitBreaker` | 3 timeouts disable app's hooks |
| `Deregistration` | `UnregisterApp` removes all hooks for that app only |
| `NoSubscribers` | Returns original payload unchanged |
| `ActionDoesNotReturnResult` | `DoAction` fires correctly (fire-and-forget) |
| `SuccessResetsFailureCount` | Success resets counter to 0 |
| `InvalidHookName` | Unknown hook names silently rejected |
| `DefaultPriority` | Priority 0 defaults to 10 |
| `FilterSkipsActionEntries` | `ApplyFilter` only calls filter-type entries |
| `ErroredHookIsSkipped` | Hook returning error string is skipped, original payload preserved |
| `HookTimeout` | Verifies timeout constant is 500ms |

---

## 20. Channel Envelope (`envelope.go`)

```go
type ChannelEnvelope struct {
    MessageID    string          // UUID v7 (time-ordered)
    ChannelID    string          // format: {type}:{platform_id}
    Sender       EnvelopeSender  // name, role, bot_id
    Text         string
    Attachments  []Attachment    // type, url, filename, size
    ReplyTo      string          // message_id for threading
    Actions      []Action        // label, callback_id
    PlatformData json.RawMessage // opaque passthrough
    Timestamp    time.Time
}
```

---

## 21. Key Gaps & Known Issues

1. **`handleSettings` returns "not yet implemented"** — app settings action in AppTool is a stub
2. **Single schedule adapter** — only one app can provide schedule capability at a time
3. **No pending update approval UI** — `.pending/` directories are created but there's no user-facing flow to approve permission changes
4. **`VerifySignature` stub** — `manifest.go:166-168` always returns nil (legacy stub, real verification is in `signing.go`)
5. **gRPC inspector** — always-on ring buffer, but no documented endpoint to expose it (dev tooling only)
6. **`DownloadAndExtractNapp` uses `http.Get`** — no auth headers, assumes public download URLs
7. **`response.stream` hook declared but not wired** — listed in `ValidHookNames` but no integration point exists; per-chunk 500ms gRPC calls would degrade streaming performance

---

## 22. Key Source File Quick Reference

For each operation, the primary file to read:

| Operation | File | Key Function |
|-----------|------|-------------|
| Load & validate manifest | `manifest.go` | `LoadManifest`, `ValidateManifest` |
| Launch an app binary | `runtime.go` | `Runtime.Launch` |
| Register capabilities | `registry.go` | `launchAndRegister` |
| Deregister capabilities | `registry.go` | `deregisterCapabilities` |
| Verify signatures | `signing.go` | `VerifyAppSignatures` |
| Extract .napp package | `napp.go` | `ExtractNapp` |
| Sanitize environment | `sandbox.go` | `sanitizeEnv` |
| Validate binary format | `sandbox.go` | `validateBinary`, `validateBinaryFormat` |
| Auto-restart crashed apps | `supervisor.go` | `Supervisor.check`, `restart` |
| Watch for file changes | `watcher.go` | `AppRegistry.Watch` |
| Handle install events | `install.go` | `HandleInstallEvent` |
| Update with permission diff | `install.go` | `handleUpdate` |
| Sideload dev app | `registry.go` | `Sideload` |
| Quarantine revoked app | `registry.go` | `Quarantine` |
| Agent tool actions | `tools/app_tool.go` | `AppTool.Execute` |
| Bridge gateway to provider | `adapter.go` | `GatewayProviderAdapter` |
| Bridge tool to skill | `adapter.go` | `AppToolAdapter` |
| Bridge channel messages | `adapter.go` | `AppChannelAdapter` |
| Schedule CRUD | `schedule_adapter.go` | `AppScheduleAdapter` |
| Hook dispatch (filter/action) | `hooks.go` | `HookDispatcher.ApplyFilter`, `DoAction` |
| Hook registration | `hooks.go` | `HookDispatcher.Register` |
| Hook wiring (tools) | `tools/registry.go` | `Registry.SetHookDispatcher` |
| Hook wiring (runner) | `runner/runner.go` | `Runner.SetHookDispatcher` |
| Hook wiring (memory) | `tools/bot_tool.go` | `BotTool.SetHookDispatcher` |
| Hook wiring (prompt) | `runner/prompt.go` | `PromptContext.Hooks` |
| Hook integration (tool exec) | `tools/desktop_queue.go` | `executeTool` |
| Kill orphaned processes | `process_unix.go` | `killOrphanGroup` |
