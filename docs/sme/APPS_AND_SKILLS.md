# Apps & Skills Platform — Internal Reference

The app platform runs sandboxed native binaries over gRPC. The skills system injects YAML+Markdown templates into the agent's system prompt. Both are unified under a single `skill` domain tool.

**Zero public documentation exists.** Everything below is derived from the Nebo codebase.

---

## Apps: The 30-Second Mental Model

```
NeboLoop Store                          Nebo (desktop)
  │                                       │
  │ .napp download                        │
  └──────────────────────────────────────►│
                                          │
    ┌─────────────────────────────────────┤
    │ 1. Extract tar.gz                   │
    │ 2. ED25519 verify (manifest+binary) │
    │ 3. Validate native binary (ELF/Mach-O/PE) │
    │ 4. Spawn process (sanitized env)    │
    │ 5. Wait for Unix socket             │
    │ 6. gRPC dial + health check         │
    │ 7. Register capabilities            │
    │ 8. Supervisor monitors (15s cycle)  │
    └─────────────────────────────────────┘
```

One app = one process. One process = one Unix socket. No network exposure. Deny-by-default permissions.

---

## App Lifecycle

### Install
- `.napp` = tar.gz containing: `manifest.json`, native binary, `signatures.json`, `SKILL.md`, optional `ui/`
- Size limits: 600MB download, 500MB binary, 1MB metadata, 5MB per UI file
- Security: path traversal protection, symlink rejection, allowlist validation, binary format check (rejects shebang/scripts)

### Verify (skipped in dev mode)
- ED25519 signatures over raw bytes (not hashes)
- Signing key: `GET /api/v1/apps/signing-key` (cached 24h)
- Revocations: `GET /api/v1/apps/revocations` (cached 1h, swept hourly)
- keyId = SHA256(publicKey)[:8] hex for rotation detection

### Launch
- Serialized per-app (mutex prevents duplicate launches)
- Kills stale processes from previous Nebo run via `.pid` file
- Env sanitization: only passes `NEBO_APP_*`, `PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL`, `TZ`, `ELEVENLABS_API_KEY`
- Process group isolation: `Setpgid: true` (kill group kills all descendants)
- Per-app logs: `logs/{stdout,stderr}.log` with 2MB rotation
- Socket wait: exponential backoff, max 10s

### Register
Based on `manifest.provides`, creates adapters:

| Capability | Adapter | Registers With | Permission |
|------------|---------|----------------|------------|
| `gateway` | GatewayProviderAdapter → ai.Provider | Provider list | `network:*` |
| `tool:{name}` / `vision` / `browser` | AppToolAdapter → tools.Tool | SkillDomainTool | None |
| `channel:{type}` | AppChannelAdapter | Channel registry | `channel:*` |
| `comm` | AppCommAdapter → comm.CommPlugin | CommPluginManager | `comm:*` |
| `ui` | Stored in uiApps map | UI handler | None |
| `schedule` | AppScheduleAdapter | Replaces built-in cron | `schedule:*` |

Apps with `user:token` permission receive NeboLoop JWT auto-injected via gRPC UserContext.

### Supervise
- 15s health check cycle (OS process alive + gRPC health)
- Exponential backoff: 10s → 20s → 40s → 80s → 160s (capped 5min)
- Rate limit: max 5 restarts/hour per app
- Suppresses file watcher during managed restart (30s)

### Update
- Stop → download new .napp → permission diff
- New permissions: stage to `.pending/` until user approves
- No new permissions: auto-update, preserve `data/` and `logs/`

### Revoke (Quarantine)
- Stop immediately, remove binary, preserve `data/` for forensics
- Mark with `.quarantined` file (prevents relaunch)
- Background sweep every hour checks all running apps

---

## Manifest

```go
type AppManifest struct {
    ID          string   // UUID (immutable)
    Name        string   // Display name
    Version     string   // Semantic version
    Description string
    Runtime     string   // "local" or "remote"
    Protocol    string   // "grpc" (only option)
    Provides    []string // Capabilities: gateway, tool:{name}, channel:{type}, ui, comm, schedule, vision, browser
    Permissions []string // Deny-by-default taxonomy (see below)
    OAuth       []OAuthRequirement
}
```

**Permission taxonomy:** `network:*`, `filesystem:{read,write}`, `memory:{read,write}`, `session:{read,create}`, `tool:{execute,*}`, `shell:bash:execute`, `subagent:spawn`, `channel:{send,receive}`, `comm:{register,send}`, `model:{claude,*}`, `schedule:{create,execute}`, `user:token`, `settings:app:name`

Wildcard: `network:*` matches any `network:...` permission.

---

## gRPC Services (proto/apps/v0/)

All services support `HealthCheck()` and `Configure(SettingsMap)`.

| Service | Key RPCs |
|---------|----------|
| **GatewayService** | `Stream(GatewayRequest) → stream GatewayEvent` — no model field, gateway routes |
| **ToolService** | `Name/Description/Schema/Execute/RequiresApproval` |
| **ChannelService** | `Connect/Disconnect/Send/Receive(stream)` — attachments, actions, threading |
| **CommService** | `Connect/Send/Subscribe/Register/Receive(stream)` — message types: message/mention/proposal/command/task |
| **UIService** | `HandleRequest(HttpRequest) → HttpResponse` — HTTP proxy over gRPC |
| **ScheduleService** | `Create/Get/List/Update/Delete/Enable/Disable/Trigger/History/Triggers(stream)` |

---

## Sandbox Model

**Binary validation:** Lstat (no symlinks) → regular file → executable bit → size check → magic bytes (ELF/Mach-O/PE only, rejects shebang)

**Env sanitization:** Strips all API keys, JWT secrets, OAuth tokens, AWS creds. Only passes NEBO_APP_* and allowlisted system vars.

**NEBO_APP_* vars:** `NEBO_APP_DIR`, `NEBO_APP_SOCK`, `NEBO_APP_ID`, `NEBO_APP_NAME`, `NEBO_APP_VERSION`, `NEBO_APP_DATA`

**File permissions:** 0700 dirs, 0600 socket, per-app log isolation (tee'd to Nebo stderr with `[app:ID]` prefix)

**Dev mode:** No NeboLoop URL → skips signature verification. Symlink sideloading allowed. `make build` auto-triggered if Makefile exists.

---

## Settings System (internal/apps/settings/)

- DB-backed key-value store (`plugin_settings` table)
- Secrets encrypted (display as `••••••••`)
- Hot-reload: UI change → DB upsert → gRPC `Configure()` push → no restart needed
- `Configurable` interface: apps implement `OnSettingsChanged(map[string]string) error`

---

## App OAuth (internal/oauth/broker/)

- Apps declare `OAuth []OAuthRequirement` in manifest
- Flow: ConnectHandler → redirect to provider → CallbackHandler → store token → Configure() push
- Tokens stored in `app_oauth_grants` table
- Frontend: AppDetailModal "Connections" tab

---

## Inspector (Dev Tooling)

- Ring buffer gRPC traffic inspector (`internal/apps/inspector/`)
- Zero-cost when no subscribers (`HasSubscribers()` fast-path)
- SSE subscription for real-time viewing

---

## HTTP API Endpoints

| Method | Endpoint | Purpose |
|--------|----------|---------|
| GET | `/api/v1/plugins` | List all plugins (type filter) |
| GET/PUT | `/api/v1/plugins/{id}[/settings]` | Get/update plugin |
| PATCH | `/api/v1/plugins/{id}/toggle` | Enable/disable |
| GET/POST/DELETE | `/api/v1/store/apps[/{id}][/install]` | Store CRUD + install |
| GET/POST/DELETE | `/api/v1/store/skills[/{id}][/install]` | Store skills |
| ANY | `/api/v1/apps/{id}/api/*` | HTTP proxy to app (10MB body limit) |
| GET | `/api/v1/apps/{id}/ui/*` | Static files (SPA fallback) |
| POST | `/api/v1/apps/{id}/ui/open` | Native window (desktop) or URL (headless) |
| GET | `/apps/{appId}/oauth/{provider}/connect` | Start OAuth |
| GET | `/apps/oauth/callback` | OAuth callback |
| GET | `/apps/{appId}/oauth/grants` | Grant status |
| DELETE | `/apps/{appId}/oauth/{provider}` | Revoke grant |

---

## Key Files

| File | Purpose |
|------|---------|
| `internal/apps/manifest.go` | AppManifest types, validation, capability/permission constants |
| `internal/apps/registry.go` | Central hub: discovery, launch, capability registration, DB |
| `internal/apps/runtime.go` | Process lifecycle, socket wait, health check |
| `internal/apps/adapter.go` | 5 adapters: Gateway, Tool, Comm, Channel, Schedule |
| `internal/apps/sandbox.go` | Env sanitization, binary validation, log management |
| `internal/apps/signing.go` | ED25519 verification, signing key provider, revocation |
| `internal/apps/napp.go` | Secure .napp extraction |
| `internal/apps/supervisor.go` | Auto-restart with backoff + rate limiting |
| `internal/apps/watcher.go` | fsnotify binary changes → restart (500ms debounce) |
| `internal/apps/install.go` | NeboLoop event routing: install/update/uninstall/revoke |
| `internal/apps/settings/store.go` | DB-backed settings with hot-reload |
| `internal/apps/inspector/` | gRPC traffic inspector |
| `internal/handler/plugins/handler.go` | Plugin management + NeboLoop store integration |
| `internal/handler/appui/handlers.go` | App UI proxy, static files, window management |
| `internal/handler/appoauth/handler.go` | OAuth flow coordination |
| `proto/apps/v0/*.proto` | gRPC service definitions (7 services) |

---

# Skills: The 30-Second Mental Model

```
SKILL.md (YAML frontmatter + Markdown body)
  │
  ├─ Loaded from: <data_dir>/skills/ (user) or extensions/skills/ (bundled)
  ├─ Hot-reloaded via fsnotify
  │
  └─► SkillDomainTool (unified domain tool)
       │
       ├─ AutoMatchSkills(message) → trigger matching → top 3 hints → "## Skill Matches"
       ├─ Agent calls skill(name: "...") → recordInvocation → session-scoped state
       └─ ActiveSkillContent(session) → "## Invoked Skills" → injected into system prompt
```

Skills = context injection. Not tools. They shape the agent's behavior by injecting instructions into the system prompt, scoped to a session with TTL-based expiry.

---

## SKILL.md Format

```yaml
---
name: unique-identifier        # required
description: One-liner          # required
version: "1.0.0"               # defaults to "1.0.0"
author: author-name
tags: [categorization]
platform: [macos, linux, windows]  # empty = cross-platform
triggers: [phrase1, phrase2]       # case-insensitive substring match
tools: [tool1, tool2]             # empty = all tools allowed
priority: 10                      # higher = matched first (default 0)
max_turns: 4                      # TTL before auto-expire (0 = use default)
dependencies: [other-skill]
metadata:
  nebo:
    emoji: "👋"
---

Full skill template injected into agent system prompt when invoked.
```

---

## Two-Phase Invocation

### Phase 1: Trigger Matching (`AutoMatchSkills`)
- Case-insensitive substring: `strings.Contains(msgLower, triggerLower)`
- Returns top 3 highest-priority matches as brief hints
- Injected as `## Skill Matches` section in system prompt
- Agent must explicitly call `skill(name: "...")` to activate

### Phase 2: Invocation
- `skill(name: "...", action: "help")` → `recordInvocation(sessionKey, slug)`
- Full SKILL.md template loaded into session state
- Injected as `## Invoked Skills` section in system prompt
- Stays active until TTL expires or manually unloaded

---

## Budgets & Limits

| Limit | Value |
|-------|-------|
| MaxActiveSkills | 4 per session |
| MaxSkillTokenBudget | 16,000 characters combined |
| DefaultSkillTTL | 4 turns (auto-matched) |
| ManualSkillTTL | 6 turns (explicitly loaded) |

Budget is character-count based. Skills sorted by most-recently-invoked. If template exceeds remaining budget, skill is skipped (not included).

---

## Session Scoping

```go
invokedSkills[sessionKey][slug] = &invokedSkillState{
    lastInvokedTurn int      // Turn when last invoked
    content         string   // SKILL.md template snapshot
    name            string   // Display name
    tools           []string // Tool restrictions
    maxTurns        int      // Per-skill TTL override
    manual          bool     // Loaded via explicit call (stickier TTL)
}
sessionTurns[sessionKey] = currentTurn
```

- Per-session, per-skill tracking. NOT shared across sessions.
- Stale skills (inactive > TTL) pruned every turn.
- Trigger re-match refreshes TTL (resets turn counter, doesn't re-inject template).
- `ClearSession(key)` removes all state on session end.

---

## Tool Restrictions

```yaml
tools: [memory, web]  # only these tools allowed when skill active
```

- Empty = all tools allowed
- Non-empty = ONLY those tools permitted
- Multiple active skills: union of all restrictions
- Implementation: `ActiveSkillTools()` collects from all invoked skills

---

## SkillDomainTool (Unified Domain Tool)

All skills — standalone SKILL.md files and app-backed tools — register as entries in one tool.

**Actions:**

| Action | Purpose |
|--------|---------|
| `catalog` | List all available skills |
| `help` / empty | Get full skill documentation |
| `load` | Manually load for session (ManualSkillTTL) |
| `unload` | Remove from session |
| `create` | Create new user skill (writes SKILL.md to disk) |
| `update` | Modify existing user skill |
| `delete` | Remove user skill |

App-backed skills have `adapter != nil` — custom actions forwarded to gRPC ToolService.
Standalone skills have `adapter == nil` — return SKILL.md template directly.

---

## Built-in Skills

| Skill | Priority | TTL | Tools | Triggers |
|-------|----------|-----|-------|----------|
| `introduction` | 100 | 1 turn | [memory] | hello, hi, hey, start, who are you, what can you do, introduce yourself |
| `store-setup` | 90 | 1 turn | [web, skill, agent] | set up skills, install skills, browse the store |

Embedded in binary via `//go:embed skills/*/SKILL.md`. Cannot be edited/deleted via API.

---

## Loading & Hot-Reload

**Sources (priority order):**
1. User skills: `<data_dir>/skills/<slug>/SKILL.md`
2. Bundled skills: `extensions/skills/` (embed.FS)

**Hot-reload chain:**
1. File change detected by fsnotify watcher
2. Loader re-parses SKILL.md
3. `OnChange` callback fires
4. SkillDomainTool re-registers entries (schema cache invalidated)
5. Next agent turn sees updated skills

**Skill settings:** `<data_dir>/skill-settings.json` — `{"disabledSkills": ["name1"]}`. Toggle → persist → OnChange → re-register.

---

## Runner Integration

```go
type SkillProvider interface {
    ActiveSkillContent(sessionKey string) string    // "## Invoked Skills" section
    AutoMatchSkills(sessionKey, message string) string // "## Skill Matches" hints
    ForceLoadSkill(sessionKey, skillName string) bool  // Onboarding, force-load
}
```

**Per-run lifecycle:**
1. `ForceSkill` param or `needsOnboarding` → `ForceLoadSkill("introduction")`
2. `AutoMatchSkills()` → trigger hints injected into system prompt
3. `ActiveSkillContent()` → invoked skill templates injected
4. After each tool execution: re-check `ActiveSkillContent()` (rebuild prompt if changed)
5. Session end: `ClearSession()` removes all tracking

---

## HTTP API

| Method | Endpoint | Purpose |
|--------|----------|---------|
| POST | `/skills` | Create new user skill |
| GET | `/skills/{name}` | Get metadata |
| GET | `/skills/{name}/content` | Get SKILL.md for editing |
| PUT | `/skills/{name}` | Update skill |
| DELETE | `/skills/{name}` | Delete skill |
| POST | `/skills/{name}/toggle` | Toggle enabled/disabled |

Slug validation: `^[a-z0-9][a-z0-9-]*[a-z0-9]$` (2+ chars). Bundled skills protected from edit/delete.

---

## CLI

```bash
nebo skills list          # All skills, sorted by priority, enabled/disabled status
nebo skills show [name]   # Full details + markdown body
```

---

## Key Files

| File | Purpose |
|------|---------|
| `internal/agent/skills/skill.go` | Skill struct, YAML parsing, validation |
| `internal/agent/skills/loader.go` | Loader, hot-watch, filesystem monitoring |
| `internal/agent/tools/skill_tool.go` | Unified domain tool, execution, invocation tracking, trigger matching |
| `cmd/nebo/skills.go` | CLI commands (list, show) |
| `internal/handler/extensions/skillhandlers.go` | HTTP CRUD handlers |
| `internal/local/skillsettings.go` | Persistent enabled/disabled state |
| `extensions/bundled.go` | embed.FS for bundled skills |
| `extensions/skills/introduction/SKILL.md` | First-meeting skill |
| `extensions/skills/store-setup/SKILL.md` | Store discovery skill |

---

## How Apps and Skills Connect

- Apps providing `tool:{name}` → registered as skill entries in SkillDomainTool
- App's SKILL.md (from .napp) provides trigger phrases and documentation
- Agent sees unified `skill` domain tool regardless of backend
- App uninstall → skill entry unregistered
- Store skill install: fetch SKILL.md from NeboLoop → write to `<data_dir>/skills/{slug}/SKILL.md` → fsnotify auto-reloads
