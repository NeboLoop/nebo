# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## CRITICAL: THE NEBO PARADIGM

Nebo is **ONE agent that is always running**. Not multiple agents. ONE.

```
┌─────────────────────────────────────────────────────────────────┐
│                        THE AGENT                                │
│                                                                 │
│  - Always running (Go process runs continuously)                │
│  - If it restarts, it has MEMORY (state persisted in SQLite)    │
│  - Can spawn SUB-AGENTS for parallel work                       │
│  - Users only interact with THIS agent                          │
│  - Proactive via crons, timers, scheduled tasks                 │
│                                                                 │
│  Channels (how users reach THE agent):                          │
│    - Web UI (/app/agent) - the primary control plane            │
│    - CLI (nebo chat)                                           │
│    - Telegram / Discord / Slack                                 │
│    - Voice                                                      │
└─────────────────────────────────────────────────────────────────┘
```

| Concept | RIGHT | WRONG |
|---------|-------|-------|
| Agent count | ONE agent, always | Multiple "agents" list |
| Lifecycle | Always running, persists state | Starts/stops, stateless |
| UI status page | Shows THE agent's health | Shows "connected agents" table |
| Parallelism | Sub-agents spawned by THE agent | Multiple independent agents |

---

## Quick Reference

```bash
# Development (hot reload via air - NO restart needed)
make air              # Backend with hot reload
cd app && pnpm dev    # Frontend dev server

# Database
make sqlc             # Regenerate sqlc code after changing .sql files
make migrate-up       # Run pending migrations
make migrate-down     # Rollback last migration
make migrate-status   # Check migration status

# Testing
go test ./...                                          # All Go tests
go test -v ./internal/logic/...                        # Logic tests with verbose
go test -v -run TestName ./internal/logic/auth/        # Single test
cd app && pnpm check                                   # TypeScript check
cd app && pnpm test:unit                               # Frontend tests

# Build & Release
make build            # Build binary to bin/nebo
make cli              # Build and install globally
make release          # Build for all platforms (darwin/linux, amd64/arm64)

# Before committing
make build && cd app && pnpm build
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
└── internal/agenthub/       → WebSocket hub for agent communication
```

### Agent (CLI + Core)

```
agent/
├── ai/           # Provider implementations (Anthropic, OpenAI, Gemini, Ollama)
│   ├── api_anthropic.go, api_openai.go, api_gemini.go, api_ollama.go
│   ├── cli_provider.go     # Wraps claude/gemini/codex CLI tools
│   └── selector.go         # Task-based model routing with fallbacks
├── runner/       # Agentic loop with provider fallback + context compaction
├── tools/        # Tool registry: bash, read, write, edit, glob, grep, web, browser, memory, cron, task
├── skills/       # YAML skills, hot-reload, trigger matching
├── plugins/      # hashicorp/go-plugin loader for tool/channel plugins
├── orchestrator/ # Sub-agent spawning (up to 5 concurrent)
├── session/      # SQLite conversation persistence
├── memory/       # Persistent fact/preference storage
└── config/       # ~/.nebo/ config loading
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

---

## Adding Database Tables

1. Create migration: `internal/db/migrations/000X_description.sql`
2. Create queries: `internal/db/queries/entity.sql`
3. Run `make sqlc` to generate Go code
4. Use generated code in `internal/logic/`

---

## Critical Rules

- **pnpm only** - Never npm or yarn
- **Styles in app.css only** - No inline styles or `<style>` blocks in Svelte files
- **Svelte 5 runes** - Use `$state`, `$derived`, `$props`, `$effect` (NOT Svelte 4 `export let`, `$:`, `<slot>`)
- **DaisyUI components** - Use DaisyUI classes for UI (btn, card, modal, input, etc.)
- **Idiomatic Go** - One function with parameters, not multiple variations (e.g., `Register(token string)` not `RegisterWithToken()` + `Register()`)
- **Minimal changes** - Never remove code that appears unused without asking first
- **NEVER hardcode model IDs** - All model IDs come from `~/.nebo/models.yaml`

---

## Configuration Files

| File | Purpose |
|------|---------|
| `~/.nebo/models.yaml` | Provider credentials & available models (loaded by agent) |
| `~/.nebo/config.yaml` | Agent settings & tool policies |
| `~/.nebo/skills/` | User-defined YAML skills |
| `~/.nebo/plugins/` | User-installed plugins (tools/, channels/) |
| `etc/nebo.yaml` | Server config (ports, database path) |
| `app/src/lib/config/site.ts` | Branding, SEO, social links |
| `.env` | Secrets only (JWT_SECRET) |

---

## Running Nebo

```bash
nebo              # Start server + agent (default)
nebo serve        # Server only
nebo agent        # Agent only
nebo chat         # CLI chat mode
nebo chat -i      # Interactive CLI mode
nebo skills list  # List available skills
nebo plugins list # List installed plugins
```

Web UI at `http://local.nebo.bot:27895`

---

## Agent Internals

**Sub-Agents:** THE agent spawns sub-agents for parallel work (up to 5 concurrent). Sub-agents are temporary, report back, and users don't interact with them directly.

**Memory Persistence:** Survives restarts via SQLite:
- Conversation history (`internal/db/chats.sql.go`)
- Facts/preferences (`agent/tools/memory.go`) - 3-tier: tacit, daily, entity
- Scheduled tasks (`agent/tools/cron.go`)
- Sessions with compaction (`agent/session/`)

**Skills:** YAML files in `~/.nebo/skills/` or `extensions/skills/`. Hot-reload, trigger-based matching, tool restrictions.

**Model Selection:** Task classification (Vision/Audio/Reasoning/Code/General) routes to appropriate model with exponential backoff on failures.

---

## Key Integrations

### AI Providers (`agent/ai/`)

| Provider | Features |
|----------|----------|
| Anthropic | Streaming, tool calls, extended thinking mode |
| OpenAI | Streaming, tool calls |
| Gemini | Streaming, tool calls, alternating turn normalization |
| Ollama | Local models, streaming |
| CLI Providers | Wraps `claude`, `gemini`, `codex` commands |

### Agent Tools (`agent/tools/`)

Core: `bash`, `read`, `write`, `edit`, `glob`, `grep`, `web`
Browser: `browser` (chromedp), `screenshot`, `vision`
Memory: `memory` (3-tier storage), `sessions`
Orchestration: `task` (sub-agents), `cron`, `message`

### Channel Integrations (`internal/channels/`)

| Channel | Library |
|---------|---------|
| Discord | `bwmarrin/discordgo` |
| Telegram | `go-telegram/bot` |
| Slack | `slack-go/slack` (Socket Mode) |

---

## Not Yet Implemented

| Feature | Notes |
|---------|-------|
| AWS Bedrock | No provider implementation |
| Azure OpenAI | No provider implementation |
| Groq | No provider implementation |
| DeepSeek (native) | Currently uses Ollama; no dedicated provider |
