# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## CRITICAL: THE NEBO PARADIGM

Nebo is **ONE primary agent** with a **lane-based concurrency system**. Not multiple independent agents.

```
┌─────────────────────────────────────────────────────────────────┐
│                        THE AGENT                                │
│                                                                 │
│  - ONE WebSocket connection (hub enforces: reconnect = drop old)│
│  - SQLite persistence survives restarts                         │
│  - Spawns SUB-AGENTS as goroutines for parallel work            │
│  - Users only interact with THIS agent                          │
│  - Proactive via heartbeat lane (independent of main lane)      │
│                                                                 │
│  Lane System (supervisor pattern for concurrency):              │
│    ┌────────────────────────────────────────────────────────┐   │
│    │  main      - User conversations (serialized)           │   │
│    │  events    - Scheduled/triggered tasks                  │   │
│    │  subagent  - Sub-agent goroutines                      │   │
│    │  nested    - Tool recursion/callbacks                  │   │
│    │  heartbeat - Proactive heartbeat ticks                 │   │
│    │  comm      - Inter-agent communication messages        │   │
│    └────────────────────────────────────────────────────────┘   │
│                                                                 │
│  Channels (how users reach THE agent):                          │
│    - Web UI (/app/agent) - the primary control plane            │
│    - CLI (nebo chat)                                           │
│    - Telegram / Discord / Slack                                 │
└─────────────────────────────────────────────────────────────────┘
```

| Concept | RIGHT | WRONG |
|---------|-------|-------|
| Agent count | ONE primary agent + sub-agent goroutines | Multiple independent agents |
| Concurrency | Lane-based (serialized main, parallel subagent) | Free-for-all parallelism |
| Lifecycle | Always running, crash recovery via SQLite | Stateless, no recovery |
| UI status page | Shows THE agent's health + lane status | Shows "connected agents" table |
| Parallelism | Sub-agents in subagent lane (goroutines) | Multiple WebSocket connections |

---

## Quick Reference

```bash
# First-time setup
make dev-setup        # go mod download/tidy + pnpm install

# Development (hot reload via air - NO restart needed)
make air              # Backend with hot reload (runs in headless mode)
make dev              # Backend + frontend together
cd app && pnpm dev    # Frontend dev server

# Code Generation
make sqlc             # Regenerate sqlc code after changing .sql files
make gen              # Regenerate TypeScript API client (runs cmd/genapi)

# Database (uses github.com/pressly/goose/v3)
make migrate-up       # Run pending migrations
make migrate-down     # Rollback last migration
make migrate-status   # Check migration status

# Testing
make test                                              # All Go tests
go test -v ./internal/logic/...                        # Logic tests with verbose
go test -v -run TestName ./internal/logic/auth/        # Single test
cd app && pnpm check                                   # TypeScript check
cd app && pnpm test:unit                               # Frontend tests

# Build & Release
make build            # Build binary to bin/nebo (CGO_ENABLED=0, headless)
make desktop          # Build desktop app (CGO_ENABLED=1, -tags desktop)
make package          # Package installer (.dmg/.msi/.deb)
make cli              # Build and install globally
make release          # Build for all platforms (darwin/linux, amd64/arm64)

# Before committing
make build
```

---

## Architecture

### Go Backend (chi router)

```
├── internal/server/         → Main server setup (chi router, routes)
├── internal/handler/        → HTTP handlers
├── internal/types/          → Request/Response types
├── internal/logic/          → Business logic - IMPLEMENT HERE
├── internal/svc/            → ServiceContext (DB, Auth, Email, AgentHub)
├── internal/httputil/       → HTTP utilities (Parse, OkJSON, Error)
├── internal/middleware/     → JWT, security, compression middleware
├── internal/db/             → SQLite + sqlc generated code
│   ├── migrations/          → SQL migration files (numbered: 0001, 0002, etc.)
│   └── queries/             → SQL query files (one per entity)
├── internal/channels/       → Channel integrations (Discord, Telegram, Slack)
├── internal/agenthub/       → WebSocket hub for agent communication
├── internal/mcp/            → MCP server + client (user-scoped MCP, external server bridge)
│   ├── bridge/              → Connects external MCP servers, proxies tools as mcp__{server}__{tool}
│   └── client/              → MCP client for connecting to external servers
└── internal/apps/           → App platform (sandboxed .napp packaging, gRPC adapters)
```

### Agent (CLI + Core)

```
internal/agent/
├── ai/           # Provider implementations (Anthropic, OpenAI, Gemini, Ollama)
│   ├── api_anthropic.go, api_openai.go, api_gemini.go, api_ollama.go
│   ├── cli_provider.go     # Wraps claude/gemini/codex CLI tools
│   ├── selector.go         # Task-based model routing with fallbacks
│   └── dedupe.go           # Deduplicates repeated messages
├── advisors/     # Internal deliberation system (markdown-based personas)
├── comm/         # Inter-agent communication (CommPlugin, CommHandler, CommPluginManager)
├── config/       # Config loading (models.yaml, config.yaml)
├── embeddings/   # Hybrid search (vector + FTS) for memories
├── memory/       # Memory extraction and context building
├── orchestrator/ # Sub-agent spawning (up to 5 concurrent)
├── plugins/      # hashicorp/go-plugin loader for tool/channel plugins
├── recovery/     # Sub-agent task persistence for crash recovery
├── runner/       # Agentic loop with provider fallback + context compaction
├── session/      # SQLite conversation persistence
├── skills/       # YAML skills, hot-reload, trigger matching
├── tools/        # STRAP domain tools (see below) + registry
└── voice/        # Voice recording for voice input
```

### Frontend (SvelteKit 2 + Svelte 5)

```
app/src/
├── routes/(app)/            → App pages (authenticated) - main UI
├── routes/(setup)/          → First-run setup wizard
├── lib/api/                 → AUTO-GENERATED TypeScript client from .api
├── lib/components/          → Reusable Svelte components
├── lib/stores/              → Svelte stores (auth, websocket)
└── lib/config/site.ts       → Branding/SEO (single source of truth)
```

---

## Adding API Endpoints

1. Add route in `internal/server/server.go`:
```go
// In registerPublicRoutes or registerProtectedRoutes:
r.Get("/widgets/{id}", widget.GetWidgetHandler(svcCtx))
```

2. Create handler in `internal/handler/widget/`:
```go
func GetWidgetHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
    return func(w http.ResponseWriter, r *http.Request) {
        var req types.GetWidgetRequest
        if err := httputil.Parse(r, &req); err != nil {
            httputil.Error(w, err)
            return
        }
        // Call logic
        resp, err := logic.NewGetWidgetLogic(r.Context(), svcCtx).GetWidget(&req)
        if err != nil {
            httputil.Error(w, err)
            return
        }
        httputil.OkJSON(w, resp)
    }
}
```

3. Define types in `internal/types/`:
```go
type GetWidgetRequest struct { Id string `path:"id"` }
type GetWidgetResponse struct { Name string `json:"name"` }
```

4. Implement logic in `internal/logic/widget/getwidgetlogic.go`

### httputil Functions (`internal/httputil/`)

| Function | Purpose |
|----------|---------|
| `Parse(r, v)` | Parses JSON body, path params (`path:"id"`), query params (`form:"name"`) |
| `OkJSON(w, v)` | 200 OK with JSON body |
| `WriteJSON(w, status, v)` | JSON with custom status code |
| `Error(w, err)` | 400 error response |
| `ErrorWithCode(w, code, msg)` | Error with specific status code |
| `Unauthorized(w, msg)` | 401 |
| `NotFound(w, msg)` | 404 |
| `BadRequest(w, msg)` | 400 |
| `InternalError(w, msg)` | 500 |
| `PathVar(r, name)` | Get path variable (chi.URLParam wrapper) |
| `QueryInt(r, name, default)` | Query param as int |
| `QueryString(r, name, default)` | Query param as string |

---

## Adding Database Tables

1. Create migration: `internal/db/migrations/000X_description.sql` (4-digit prefix, uses goose)
2. Create queries: `internal/db/queries/entity.sql` (one file per entity)
3. Run `make sqlc` to generate Go code (config in `sqlc.yaml`, engine: sqlite)
4. Use generated code in `internal/logic/`

---

## Adding Agent Tools (STRAP Pattern)

**To add a new action to an existing domain tool:**

1. Add the action to the resource config in the domain tool file
2. Add input fields to the `*Input` struct if needed
3. Add a case in the `Execute()` switch statement
4. Implement the handler method

**To add a new resource to an existing domain:**

1. Add resource to the `*Resources` map with its actions
2. Add a routing case in `Execute()`
3. Implement handler methods for each action

**To create a new domain tool:**

1. Create `internal/agent/tools/newdomain_tool.go`
2. Define the input struct with all fields:
```go
type NewDomainInput struct {
    Resource string `json:"resource"`
    Action   string `json:"action"`
    // ... domain-specific fields
}
```
3. Implement `DomainTool` interface:
```go
func (t *NewDomainTool) Name() string { return "newdomain" }
func (t *NewDomainTool) Domain() string { return "newdomain" }
func (t *NewDomainTool) Resources() []string { return []string{"res1", "res2"} }
func (t *NewDomainTool) ActionsFor(resource string) []string { ... }
func (t *NewDomainTool) Schema() json.RawMessage { return BuildDomainSchema(...) }
func (t *NewDomainTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) { ... }
```
4. Register in `registry.go` `RegisterDefaults()`

**Platform-specific tools:** Use build tags and register via `RegisterCapability()` in `init()`

---

## Critical Rules

- **pnpm only** - Never npm or yarn
- **Styles in app.css only** - No inline styles or `<style>` blocks in Svelte files
- **Svelte 5 runes** - Use `$state`, `$derived`, `$props`, `$effect` (NOT Svelte 4 `export let`, `$:`, `<slot>`)
- **DaisyUI components** - Use DaisyUI classes for UI (btn, card, modal, input, etc.)
- **Idiomatic Go** - One function with parameters, not multiple variations (e.g., `Register(token string)` not `RegisterWithToken()` + `Register()`)
- **Minimal changes** - Never remove code that appears unused without asking first
- **NEVER hardcode model IDs** - All model IDs come from `models.yaml` in the Nebo data directory
- **NEVER run goctl directly** - Only use `make gen` to regenerate API code
- **Always build before pushing** - `make build` (includes frontend build)

---

## App Platform (`internal/apps/`)

Sandboxed app system. NeboLoop distributes apps, Nebo runs them.

- `.napp` packages: tar.gz containing manifest.json, binary, signatures.json, optional ui/
- Apps run in sandboxed UUID directories with gRPC over Unix sockets
- Deny-by-default permissions; apps don't need permission for their own `data/` directory
- ED25519 signing (raw bytes, not hashes); signatures.json is separate from manifest
- MQTT-based install flow: NeboLoop publishes to `neboloop/bot/{botID}/installs`
- Structured template UI (Tier 1): app pushes JSON blocks, Nebo renders Svelte components
- 8 block types: text, heading, input, button, select, toggle, divider, image
- Proto definitions: `proto/apps/v0/` (ui.proto, tool.proto, channel.proto, comm.proto, gateway.proto, schedule.proto, common.proto)

**Key files:** manifest.go (types/validation), runtime.go (process lifecycle), sandbox.go (env sanitization), signing.go (ED25519 verification), napp.go (secure extraction), registry.go (discovery/launch), adapter.go (gRPC bridges), install.go (MQTT listener), channels.go (MQTT channel bridge)

---

## Configuration Files

Data directory location (platform-standard):
- **macOS:** `~/Library/Application Support/Nebo/`
- **Windows:** `%AppData%\Nebo\`
- **Linux:** `~/.config/nebo/`
- **Override:** `NEBO_DATA_DIR` environment variable

| File | Purpose |
|------|---------|
| `<data_dir>/models.yaml` | Provider credentials & available models (loaded by agent) |
| `<data_dir>/config.yaml` | Agent settings, tool policies, lane concurrency, advisors |
| `<data_dir>/skills/` | User-defined YAML skills |
| `<data_dir>/plugins/` | User-installed plugins (tools/, channels/) |
| `etc/nebo.yaml` | Server config (ports, database path) |
| `app/src/lib/config/site.ts` | Branding, SEO, social links |
| `.env` | Secrets only (JWT_SECRET) |

---

## Running Nebo

```bash
nebo              # Desktop mode (native window + system tray + agent)
nebo --headless   # Headless mode (HTTP server + agent, opens browser)
nebo serve        # Server only
nebo agent        # Agent only
nebo chat         # CLI chat mode
nebo chat -i      # Interactive CLI mode
nebo skills list  # List available skills
nebo apps list    # List installed apps
nebo doctor       # System diagnostics
nebo session list # Session management
```

Web UI at `http://localhost:27895`

**Desktop mode** (default): Wails v3 native window + system tray. Close window minimizes to tray.
**Headless mode** (`--headless`): No native window, opens browser. Current behavior.

---

## Agent Internals

### Lane System (`internal/agenthub/lane.go`)

Lanes are work queues that organize different types of work.

| Lane | Purpose |
|------|---------|
| `main` | User conversations (serialized, one at a time) |
| `events` | Scheduled/triggered tasks |
| `subagent` | Sub-agent goroutines |
| `nested` | Tool recursion/callbacks |
| `heartbeat` | Proactive heartbeat ticks (runs independently of main) |
| `comm` | Inter-agent communication messages (concurrent) |

Key functions:
- `Enqueue()` - Block until task completes
- `EnqueueAsync()` - Non-blocking queue add
- `pump()` - Processes queue respecting max concurrency

**Lane configuration in `config.yaml`:**
```yaml
lanes:
  main: 1       # User conversations (serialized)
  events: 2     # Scheduled/triggered tasks
  subagent: 0   # Sub-agent operations (0 = unlimited)
  nested: 3     # Nested tool calls (hard cap)
  heartbeat: 1  # Proactive heartbeat ticks (sequential)
  comm: 5       # Inter-agent communication (concurrent)
```

### Sub-Agents (`internal/agent/orchestrator/orchestrator.go`)

Sub-agents are goroutines (NOT separate processes/connections):
- Each gets own session: `subagent-{uuid}`
- Persisted to `pending_tasks` table before spawning (crash recovery)
- Runs own agentic loop via `Runner.Run()`
- Managed via `agent(resource: task, action: spawn, ...)`

### Hub vs Runner vs Agent Command

| Component | File | Responsibility |
|-----------|------|----------------|
| **Hub** | `internal/agenthub/hub.go` | WebSocket connections, agent registry, message routing |
| **Lanes** | `internal/agenthub/lane.go` | Work queues with concurrency limits |
| **Runner** | `internal/agent/runner/runner.go` | Agentic loop, model selection, tool execution |
| **Agent Cmd** | `cmd/nebo/agent.go` | Glue code connecting hub to runner via lanes |

### Memory Persistence

Survives restarts via SQLite:
- Conversation history (`agent_messages` table)
- Facts/preferences (`embeddings` table) - 3-tier: tacit, daily, entity
- Scheduled tasks (`cron_jobs` table)
- Pending sub-agent tasks (`pending_tasks` table)
- Sessions with compaction (`internal/agent/session/`)

**Skills:** YAML files in `<data_dir>/skills/` or `extensions/skills/`. Hot-reload, trigger-based matching, tool restrictions.

**Model Selection:** Task classification (Vision/Audio/Reasoning/Code/General) routes to appropriate model with exponential backoff on failures.

**Tool Registration:** Domain tools are registered in `RegisterDefaults()`. The `AgentDomainTool` requires separate registration via `RegisterAgentDomainTool()` since it needs DB and session manager dependencies. Platform tools auto-register via `RegisterPlatformCapabilities()` in platform-specific `init()` functions.

**Tool Policy** (`internal/agent/tools/policy.go`): Controls tool approval. Levels: `PolicyDeny` (block all dangerous), `PolicyAllowlist` (allow whitelisted commands, default), `PolicyFull` (allow all). Ask modes: `AskModeOff`, `AskModeOnMiss` (default), `AskModeAlways`. Safe bins (always allowed): ls, pwd, cat, grep, find, git status/log/diff, go/node/python --version.

### Advisors System (`internal/agent/advisors/`)

Advisors are internal "voices" that deliberate on tasks before the main agent decides. They do NOT speak to users, commit memory, or persist independently.

**Definition format:** Markdown files with YAML frontmatter (`ADVISOR.md`):
```yaml
---
name: skeptic
role: critic
description: Challenges assumptions and identifies weaknesses
priority: 10
enabled: true
---

You are the Skeptic. Your role is to challenge ideas and find flaws...
```

**Configuration in `config.yaml`:**
```yaml
advisors:
  enabled: true
  max_advisors: 5
  timeout_seconds: 30
```

---

## Key Integrations

### AI Providers (`internal/agent/ai/`)

| Provider | Features |
|----------|----------|
| Anthropic | Streaming, tool calls, extended thinking mode |
| OpenAI | Streaming, tool calls |
| Gemini | Streaming, tool calls, alternating turn normalization |
| Ollama | Local models, streaming |
| CLI Providers | Wraps `claude`, `gemini`, `codex` commands |

### Agent Tools - STRAP Pattern (`internal/agent/tools/`)

Tools use the **STRAP (Single Tool Resource Action Pattern)** - consolidating 35+ individual tools into 4 domain tools for reduced LLM context overhead (~80% reduction).

| Domain | Tool Name | Resources | Actions |
|--------|-----------|-----------|---------|
| File | `file` | - | read, write, edit, glob, grep |
| Shell | `shell` | bash, process, session | exec, bg, kill, list, status, send |
| Web | `web` | - | fetch, search, navigate, click, type, screenshot |
| Agent | `agent` | task, cron, memory, message, session, comm | spawn, create, store, recall, send, list, subscribe, etc. |

**Usage pattern:**
```
file(action: read, path: "/tmp/test.txt")
shell(resource: bash, action: exec, command: "ls -la")
web(action: search, query: "golang")
agent(resource: memory, action: store, key: "user/name", value: "Alice", layer: "tacit")
agent(resource: comm, action: send, to: "agent-2", topic: "tasks", text: "Can you handle this?")
```

**Key files:**
- `domain.go` - DomainTool interface, validators, schema builder
- `file_tool.go` - File operations (replaces read.go, write.go, edit.go, glob.go, grep.go)
- `shell_tool.go` - Shell operations (replaces bash.go, process.go, bash_sessions.go)
- `web_tool.go` - Web operations (replaces web.go, search.go, browser.go)
- `agent_tool.go` - Agent operations (replaces task.go, cron.go, memory.go, message.go, sessions.go)
- `registry.go` - Tool registration and execution

**Standalone tools:** `screenshot`, `vision` (requires API key), platform capabilities (*_darwin.go, *_windows.go, *_linux.go — auto-registered via `init()` with build tags)

### Origin-Based Tool Restrictions (`internal/agent/tools/origin.go`)

Origins track where a request came from and enforce per-origin tool restrictions via the policy system:

| Origin | Source | Default Restrictions |
|--------|--------|---------------------|
| `OriginUser` | Direct user interaction (web UI, CLI) | None |
| `OriginComm` | Inter-agent communication | Denies: shell |
| `OriginApp` | External app binary | Denies: shell |
| `OriginSkill` | Matched skill template | Denies: shell |
| `OriginSystem` | Internal system tasks (heartbeat, cron, recovery) | None |

Use `WithOrigin(ctx, origin)` / `GetOrigin(ctx)` to propagate origin through context. The registry checks `Policy.IsDeniedForOrigin()` before approval logic.

**Memory 3-tier system:**
- `tacit` - Long-term preferences, learned behaviors
- `daily` - Day-specific facts (keyed by date)
- `entity` - Information about people, places, things

### Channel Integrations

Channels (Telegram, Discord, Slack) are bridged through NeboLoop via MQTT, not embedded directly in Nebo.

- **Inbound:** NeboLoop runs platform bridges, publishes messages to MQTT topic `neboloop/bot/{botID}/channels/{channelType}/inbound` (legacy: `chat/in`)
- **Outbound:** Nebo publishes replies to `neboloop/bot/{botID}/channels/{channelType}/outbound` (legacy: `chat/out`)
- **Implementation:** `internal/apps/channels.go` — ChannelBridge using autopaho MQTT client
- **Wired in:** `cmd/nebo/agent.go` — starts on NeboLoop comm connect (startup + mid-session code redemption)

### Provider Loading (`cmd/nebo/providers.go`)

Provider detection priority:
1. **Database** — API keys from UI (Settings > Providers) stored in `auth_profiles` table
2. **Config file** — `models.yaml` credentials section (env var expansion via `os.ExpandEnv`)
3. **CLI auto-discovery** — If `models.yaml` `defaults.primary` starts with `claude-code`/`codex-cli`/`gemini-cli`, checks PATH for the CLI binary

**Desktop app PATH caveat:** macOS GUI apps get a minimal PATH. `ensureUserPath()` in `cmd/nebo/root.go` augments PATH with `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, etc. so CLI tools are discoverable when launched from Finder/Dock.

---

## Licensing

- **Entire project:** Apache License 2.0 — fully open source as of February 16, 2026
- Includes core runtime, App SDK, proto definitions, and reference apps
- NEVER reference iPhone/iOS/App Store in code comments

