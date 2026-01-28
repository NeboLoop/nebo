# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Quick Reference

```bash
# Development (hot reload - no restart needed)
make air              # Backend with hot reload
cd app && pnpm dev    # Frontend dev server

# Code generation (CRITICAL: NEVER run goctl directly)
make gen              # Regenerate handlers/types from .api file

# Testing
go test -v ./internal/logic/...                        # All Go tests
go test -v -run TestName ./internal/logic/auth/        # Single test
cd app && pnpm check                                   # TypeScript check
cd app && pnpm test:unit                               # Frontend tests

# Database (standalone mode)
make migrate-up       # Run pending migrations
make migrate-down     # Rollback last migration
make migrate-status   # Check migration status

# Before committing
make build && cd app && pnpm build
```

## Architecture

```
gobot.api                   → API definition (routes, types) - EDIT HERE to add endpoints
├── internal/handler/        → AUTO-GENERATED from .api (DO NOT EDIT)
├── internal/types/          → AUTO-GENERATED from .api (DO NOT EDIT)
├── internal/logic/          → Business logic - EDIT HERE for implementation
├── internal/svc/            → ServiceContext
├── internal/db/             → SQLite database
└── internal/local/          → Local auth/email services

app/src/
├── routes/(www)/            → Marketing pages (public)
├── routes/(auth)/           → Auth pages (login, register)
├── routes/(app)/            → App pages (authenticated)
├── lib/api/                 → AUTO-GENERATED TypeScript client
├── lib/config/site.ts       → Branding/SEO (single source of truth)
└── lib/stores/              → Svelte stores (auth)
```

## Adding API Endpoints

1. Define in `gobot.api`:
```
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
    // Use l.svcCtx.DB for database access
    // Use l.svcCtx.Auth for auth operations
    return &types.GetWidgetResponse{Name: "widget"}, nil
}
```

4. Frontend types auto-available: `import { getWidget } from '$lib/api'`

Key service context fields: `l.svcCtx.DB`, `l.svcCtx.Auth`, `l.svcCtx.Email`

## Critical Rules

- **NEVER run goctl directly** - Use `make gen`
- **pnpm only** - Never npm or yarn
- **Styles in app.css only** - No inline styles or `<style>` blocks
- **Svelte 5 runes** - `$state`, `$derived`, `$props`, `$effect` (not Svelte 4 `export let`, `$:`, `<slot>`)
- **DaisyUI components** - Use DaisyUI classes for UI components (btn, card, modal, etc.)
- **Idiomatic Go** - One function with parameters, not multiple variations
- **Minimal changes** - Never remove code that appears unused without asking

## Configuration

| File | Purpose |
|------|---------|
| `app/src/lib/config/site.ts` | Branding, SEO, social links |
| `etc/gobot.yaml` | Products, pricing, backend settings |
| `.env` | Secrets only (API keys, JWT secret) |

## CLI Agent

The GoBot CLI agent (`cmd/gobot-cli/`) is a standalone AI assistant with tool use capabilities.

```bash
# Build and install
make build-cli        # Build to bin/gobot-cli
make cli              # Build and install globally

# Usage
gobot chat "hello"                    # One-shot query
gobot chat --interactive              # Interactive REPL
gobot chat -s myproject "list files"  # Use named session
gobot chat --dangerously "deploy it"  # 100% autonomous (no approval prompts!)
gobot config                          # Show configuration
gobot session list                    # List saved sessions
```

**Configuration:** `~/.gobot/config.yaml`
```yaml
providers:
  - name: anthropic-api
    type: api
    api_key: ${ANTHROPIC_API_KEY}
    model: claude-sonnet-4-20250514
```

**Available Tools:**
- `bash` - Execute shell commands (requires approval)
- `read` - Read file contents
- `write` - Write/create files (requires approval)
- `edit` - Find-and-replace edits (requires approval)
- `glob` - Find files by pattern
- `grep` - Search file contents
- `web` - Fetch URLs

**Agent Architecture:**
```
agent/
├── config/       # ~/.gobot/config.yaml loading
├── session/      # SQLite conversation persistence
├── ai/           # Provider implementations (Anthropic, OpenAI)
├── tools/        # Tool registry and implementations
└── runner/       # Agentic loop with provider fallback
```
