# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## CRITICAL: THE GOBOT PARADIGM

GoBot is **ONE agent that is always running**. Not multiple agents. ONE.

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
│    - CLI (gobot chat)                                           │
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

# Code generation (CRITICAL: NEVER run goctl directly)
make gen              # Regenerate handlers/types from .api file

# Database
make sqlc             # Regenerate sqlc code after changing .sql files
make migrate-up       # Run pending migrations
make migrate-down     # Rollback last migration

# Testing
go test -v ./internal/logic/...                        # All Go tests
go test -v -run TestName ./internal/logic/auth/        # Single test
cd app && pnpm check                                   # TypeScript check
cd app && pnpm test:unit                               # Frontend tests

# Before committing
make build && cd app && pnpm build
```

---

## Architecture

### Go Backend (go-zero framework)

```
gobot.api                    → API definition (routes, types) - EDIT THIS to add endpoints
├── internal/handler/        → AUTO-GENERATED from .api (DO NOT EDIT)
├── internal/types/          → AUTO-GENERATED from .api (DO NOT EDIT)
├── internal/logic/          → Business logic - IMPLEMENT HERE
├── internal/svc/            → ServiceContext (DB, Auth, Email, AgentHub)
├── internal/db/             → SQLite + sqlc generated code
├── internal/local/          → Local services (auth, email, settings stores)
└── internal/agenthub/       → WebSocket hub for agent communication
```

### Agent (CLI + Core)

```
agent/
├── ai/           # Provider implementations (Anthropic, OpenAI, Google, Gemini, Ollama, DeepSeek)
├── runner/       # Agentic loop with provider fallback + context compaction
├── tools/        # Tool registry: bash, read, write, edit, glob, grep, web, browser, memory, cron, task
├── skills/       # YAML skills, hot-reload, trigger matching
├── session/      # SQLite conversation persistence
├── memory/       # Persistent fact/preference storage
└── config/       # ~/.gobot/ config loading
```

### Frontend (SvelteKit 2 + Svelte 5)

```
app/src/
├── routes/(app)/            → App pages (authenticated) - main UI
├── routes/(setup)/          → First-run setup wizard
├── lib/api/                 → AUTO-GENERATED TypeScript client from .api
├── lib/stores/              → Svelte stores (auth, websocket)
└── lib/config/site.ts       → Branding/SEO (single source of truth)
```

---

## Adding API Endpoints

1. Define in `gobot.api`:
```go
@server(prefix: /api/v1, jwt: Auth)
service gobot {
    @handler GetWidget
    get /widgets/:id (GetWidgetRequest) returns (GetWidgetResponse)
}

type GetWidgetRequest { Id string `path:"id"` }
type GetWidgetResponse { Name string `json:"name"` }
```

2. Run `make gen`

3. Implement in `internal/logic/getwidgetlogic.go`:
```go
func (l *GetWidgetLogic) GetWidget(req *types.GetWidgetRequest) (*types.GetWidgetResponse, error) {
    // l.svcCtx.DB, l.svcCtx.Auth, l.svcCtx.Email, l.svcCtx.AgentHub
    return &types.GetWidgetResponse{Name: "widget"}, nil
}
```

4. Frontend types auto-available: `import { getWidget } from '$lib/api'`

---

## Critical Rules

- **NEVER run goctl directly** - Always use `make gen`
- **pnpm only** - Never npm or yarn
- **Styles in app.css only** - No inline styles or `<style>` blocks in Svelte files
- **Svelte 5 runes** - Use `$state`, `$derived`, `$props`, `$effect` (NOT Svelte 4 `export let`, `$:`, `<slot>`)
- **DaisyUI components** - Use DaisyUI classes for UI (btn, card, modal, input, etc.)
- **Idiomatic Go** - One function with parameters, not multiple variations (e.g., `Register(token string)` not `RegisterWithToken()` + `Register()`)
- **Minimal changes** - Never remove code that appears unused without asking first

---

## Configuration Files

| File | Purpose |
|------|---------|
| `~/.gobot/models.yaml` | Provider credentials & available models (loaded by agent) |
| `~/.gobot/config.yaml` | Agent settings & tool policies |
| `etc/gobot.yaml` | Server config (ports, database path) |
| `app/src/lib/config/site.ts` | Branding, SEO, social links |
| `.env` | Secrets only (JWT_SECRET) |

---

## Agent Internals

**Sub-Agents:** THE agent spawns sub-agents for parallel work (up to 5 concurrent). Sub-agents are temporary, report back, and users don't interact with them directly.

**Memory Persistence:** Survives restarts via SQLite:
- Conversation history (`internal/db/chats.sql.go`)
- Facts/preferences (`agent/tools/memory.go`)
- Scheduled tasks (`agent/tools/cron.go`)
- Sessions with compaction (`agent/session/`)

**Skills:** YAML files in `~/.gobot/skills/` or `extensions/skills/`. Hot-reload, trigger-based matching, tool restrictions.

---

## Running GoBot

```bash
gobot              # Start server + agent (default)
gobot serve        # Server only
gobot agent        # Agent only
gobot chat         # CLI chat mode
```

Web UI at `http://localhost:29875`

---

## Feature Inventory

### AI Providers (`agent/ai/`)

**IMPORTANT: All model IDs come from `~/.gobot/models.yaml` - NEVER hardcode model IDs in code!**

| Provider | File | Features |
|----------|------|----------|
| Anthropic | `api_anthropic.go` | Streaming, tool calls, extended thinking mode |
| OpenAI | `api_openai.go` | Streaming, tool calls |
| Gemini | `api_gemini.go` | Streaming, tool calls, function declarations, alternating turn normalization |
| Ollama | `api_ollama.go` | Local models, streaming, configurable base URL |
| Claude CLI | `cli_provider.go` | Wraps `claude` command, stream-json parsing, dangerously-skip-permissions |
| Gemini CLI | `cli_provider.go` | Wraps `gemini` command |
| Codex CLI | `cli_provider.go` | Wraps `codex` command, full-auto mode |

### Model Selection (`agent/ai/selector.go`)

| Feature | Description |
|---------|-------------|
| Task Classification | Classifies messages as Vision/Audio/Reasoning/Code/General based on content and keywords |
| TaskRouting Config | Per-task model routing via `models.yaml` with fallbacks |
| Exponential Backoff | 5s→10s→20s→40s...1hr cooldown on failures |
| Cooldown Tracking | Thread-safe cooldown state per model |
| Thinking Mode | Auto-detects models with thinking capability (opus, o1, o3) |

### Agent Tools (`agent/tools/`)

| Tool | File | Description |
|------|------|-------------|
| `bash` | `bash.go` | Shell command execution with timeout |
| `read` | `read.go` | File reading with line limits |
| `write` | `write.go` | File creation and overwriting |
| `edit` | `edit.go` | Text replacement in files |
| `glob` | `glob.go` | File pattern matching |
| `grep` | `grep.go` | Content search with regex |
| `web` | `web.go` | URL fetching and HTML parsing |
| `browser` | `browser.go` | **chromedp** automation: navigate, click, type, screenshot, text, html, evaluate, wait |
| `memory` | `memory.go` | **3-tier memory**: tacit (long-term), daily (date-keyed), entity (people/places/things). FTS search, namespaces, tags |
| `cron` | `cron.go` | Scheduled tasks with **bash AND agent task types**, delivery to Discord/Telegram/Slack |
| `message` | `message.go` | Send messages to connected channels (Discord/Telegram/Slack) |
| `task` | `task.go` | Sub-agent spawning (explore/plan/general types) |
| `sessions` | `sessions.go` | Session management: list, history, status, clear |
| `vision` | `vision.go` | Image analysis via Anthropic API |
| `policy` | `policy.go` | Tool approval policies (allow/deny/ask) |

### Channel Integrations (`internal/channels/`)

| Channel | File | Library | Features |
|---------|------|---------|----------|
| Discord | `discord/discord.go` | `bwmarrin/discordgo` | Message handlers, reply threading, DMs |
| Telegram | `telegram/telegram.go` | `go-telegram/bot` | Message handlers, reply threading, topic support |
| Slack | `slack/slack.go` | `slack-go/slack` | Socket Mode, message handlers |

### Voice (`agent/voice/`, `internal/voice/`)

| Feature | File | Description |
|---------|------|-------------|
| Recording | `agent/voice/recorder.go` | Cross-platform audio recording via sox/ffmpeg/arecord |
| Transcription | `agent/voice/recorder.go` | OpenAI Whisper API integration |
| HTTP Upload | `internal/voice/transcribe.go` | Web endpoint for voice file uploads |

### Plugin System (`agent/plugins/`)

| Feature | File | Description |
|---------|------|-------------|
| Plugin Loader | `loader.go` | hashicorp/go-plugin for tool and channel plugins |
| Hot Reload | `watcher.go` | fsnotify-based file watching |
| Tool Interface | `tool.go` | RPC interface for external tool plugins |
| Channel Interface | `channel.go` | RPC interface for external channel plugins |

### Extension Plugins (`extensions/tools/`)

| Plugin | File | Features |
|--------|------|----------|
| GitHub | `github/main.go` | repos, issues, prs, create_issue, create_pr, search, comment |
| Notion | `notion/main.go` | Databases, pages, search |
| TTS | `tts/main.go` | Text-to-speech output |

### Sub-Agent Orchestration (`agent/orchestrator/`)

| Feature | Description |
|---------|-------------|
| Concurrent Execution | Up to 5 concurrent sub-agents |
| Task Isolation | Separate session per sub-agent |
| Status Tracking | pending/running/completed/failed/cancelled states |
| Timeout Support | Per-agent timeout configuration |
| Cleanup | Automatic cleanup of completed agents by age |

### Memory System (`agent/memory/`)

| Feature | File | Description |
|---------|------|-------------|
| Memory Files | `files.go` | Loads AGENTS.md and MEMORY.md from workspace |
| Auto-Extraction | `extraction.go` | AI-powered fact extraction from conversations |
| 3-Tier Storage | via `tools/memory.go` | tacit (preferences), daily (date-keyed), entity (people/places/things) |

### MCP Server (`internal/mcp/`)

| Feature | File | Description |
|---------|------|-------------|
| OAuth Support | `mcpauth/` | Dynamic client registration, token validation |
| User-Scoped Tools | `server.go` | Per-user tool context |
| Session Persistence | `mcpctx/` | Session caching across requests |

### Runner (`agent/runner/runner.go`)

| Feature | Description |
|---------|-------------|
| Provider Map | Model-based provider switching (e.g., "openai/gpt-5.2" routes to OpenAI provider) |
| Session Management | SQLite-backed conversation persistence |
| Context Compaction | Summarizes old messages when context overflows |
| Memory Auto-Extract | Extracts facts from completed conversations |
| Skill Loading | YAML skills from `~/.gobot/skills/` with trigger matching |
| User Model Switching | Detects "use claude" / "switch to opus" requests |

### Provider Configuration (`internal/provider/models.go`)

**All model IDs are configured in `~/.gobot/models.yaml` - never hardcoded!**

```yaml
# ~/.gobot/models.yaml structure (January 2026)
version: "1.0"
credentials:
  anthropic:
    api_key: ${ANTHROPIC_API_KEY}
  openai:
    api_key: ${OPENAI_API_KEY}
defaults:
  primary: "anthropic/claude-sonnet-4-5-20250929"
  fallbacks:
    - "openai/gpt-5.2"
task_routing:
  vision: "anthropic/claude-sonnet-4-5-20250929"
  audio: "openai/gpt-5.2-audio"
  reasoning: "anthropic/claude-opus-4-5-20250929"
  code: "anthropic/claude-sonnet-4-5-20250929"
  general: "anthropic/claude-sonnet-4-5-20250929"
  fallbacks:
    reasoning:
      - "openai/o3"
providers:
  anthropic:
    - id: claude-sonnet-4-5-20250929
      displayName: Claude Sonnet 4.5
      contextWindow: 1000000
      capabilities: [tools, vision, thinking]
    - id: claude-opus-4-5-20250929
      displayName: Claude Opus 4.5
      contextWindow: 1000000
      capabilities: [tools, vision, thinking, reasoning]
```

### Auth Profiles (`internal/db/queries/auth_profiles.sql`)

| Feature | Description |
|---------|-------------|
| Round-Robin | Profiles selected by priority DESC, then least-recently-used |
| Auth Types | OAuth > Token > API Key priority ordering |
| Cooldown | Per-profile cooldown after errors |
| Usage Tracking | last_used_at, usage_count, error_count |

---

## Not Yet Implemented

| Feature | Notes |
|---------|-------|
| AWS Bedrock | No provider implementation |
| Azure OpenAI | No provider implementation |
| Groq | No provider implementation |
| DeepSeek (native) | Currently uses Ollama; no dedicated provider |
