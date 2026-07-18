# CLAUDE.md

Behavioral guidelines to reduce common LLM coding mistakes. Merge with project-specific instructions as needed.

**Tradeoff:** These guidelines bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them - don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

## 5. No Quick Fixes (CRITICAL)

**Always do the right fix. Never present a "quick" option.**

The quick fix always becomes tech debt we never remember. A "TODO come back to this" never gets revisited. The shortcut you took today is the bug report next quarter and the architectural rewrite next year. Worse, *presenting* a quick option as a choice biases the decision toward it — even when "right" is only marginally more effort.

The rules:

- **NEVER offer "quick fix vs right fix" as a choice.** If you know the right fix, do the right fix. Don't frame it as an option.
- **NEVER ship a workaround as the final answer.** Workarounds during diagnosis are fine ("let me try X to confirm the hypothesis"), but the patch that lands MUST be the proper fix.
- **NEVER leave a bandaid behind.** No `// TODO: do this properly later`, no `// FIXME`, no commented-out alternate paths.
- **NEVER bypass a constraint to "make it work."** If a check, lint, type, or test is blocking you, fix what it's complaining about — don't `--no-verify`, don't `#[allow]`, don't widen a type to silence the compiler.
- **NEVER add a competing pathway** for something that already has one canonical implementation (see Rule 8 in `docs/sme/CODE_AUDITOR.md`). Two ways to do the same thing IS tech debt.

If you genuinely don't know the right fix yet, say so. Investigate. Ask. Don't fill the gap with a shortcut and a comment.

**The test:** Six months from now, will someone reading this code think "why was this done this temporary way?" If yes, you took a shortcut. Redo it.

---

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, and clarifying questions come before implementation rather than after mistakes.

---

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What is Nebo

Nebo is **the operating system for AI employees** — it turns the user's computer into an AI workforce. Pre-built employees are hired from the NeboAI marketplace in one click and run locally with persistent memory, coordinating as a team. Never call it an "assistant" or "companion" — the frame is employees / workforce / hire ("pre-built," never "pre-trained"). It targets non-technical professionals (realtors, lawyers, consultants). Platform-agnostic: macOS, Windows, Linux.

## Tech Stack

- **Backend:** Rust (edition 2024), 18 workspace crates under `crates/` + `src-tauri/`
- **Frontend:** SvelteKit 2 + Svelte 5, Tailwind CSS 4 + DaisyUI 5, TypeScript 5.9
- **Desktop:** Tauri 2 (native window + system tray)
- **Database:** SQLite via rusqlite + r2d2 (WAL mode), stored at `~/.nebo/data/nebo.db`
- **HTTP:** Axum 0.8 with tower middleware
- **AI Providers:** Anthropic, OpenAI, Gemini, Ollama, DeepSeek, CLI wrappers

## Build & Development Commands

### Backend (Rust)
```bash
make dev              # Hot reload headless server (cargo watch)
make build            # Release CLI binary (cargo build --release -p nebo-cli)
make build-desktop    # Tauri desktop app (builds frontend first)
make test             # cargo test (all workspace tests)
make clean            # rm -rf target/ dist/
cargo test -p nebo-agent             # Test a single crate
cargo test -p nebo-tools -- test_name  # Run a specific test
```

### Frontend (SvelteKit)
```bash
cd app
pnpm dev              # Vite dev server on :5173, proxies API to :27895
pnpm build            # Production build (vite build + CSS fallback injection)
pnpm check            # svelte-check + TypeScript diagnostics
pnpm lint             # prettier --check + eslint
pnpm test             # vitest --run
pnpm test:unit        # vitest (watch mode)
```

### Local Development Flow
Run backend and frontend in separate terminals:
1. `make dev` — Rust backend with hot reload on :27895
2. `cd app && pnpm dev` — SvelteKit on :5173 (proxies /api, /ws, /health to :27895)

Access at http://localhost:5173 (dev) or http://localhost:27895 (production/embedded).

### macOS Packaging
```bash
make app-bundle       # Re-sign .app with Developer ID
make dmg              # Create .dmg installer
make notarize         # Notarize with Apple
make install          # Full pipeline → /Applications
```

## Architecture

### Crate Dependency Flow
```
cli / src-tauri (entry points)
  → server (Axum HTTP + WebSocket + handlers)
    → agent (runner, session, memory, steering, compaction)
      → ai (provider trait: Anthropic, OpenAI, Gemini, Ollama)
      → tools (registry, policy, domain tools, skills loader)
    → auth (JWT, keyring, AES-256-GCM encryption)
    → db (SQLite, migrations, connection pool)
    → config (YAML loading from etc/nebo.yaml)
    → types (error enum, constants)
```

Satellite crates: `mcp`, `napp`, `workflow`, `browser`, `comm`, `notify`, `updater`, `voice`, `proto`

### Domain Tools (STRAP Pattern)
10 domain tools consolidate 35+ legacy tools with ~80% context reduction:
- **system** (file + shell + platform) — the meta-tool
- **web** (http + search + browser + devtools)
- **bot** (task + memory + session + profile + context + advisors + vision + ask)
- **loop** (dm + channel + group + topic)
- **message** (owner + sms + notify)
- **event** (scheduling)
- **app** (lifecycle + store)
- **desktop** (input + ui + window + menu + dialog + tts) — platform-specific
- **organizer** (mail + contacts + calendar + reminders) — platform-specific
- **skill** (dynamic per-skill)

### Chat System Flow
```
Frontend (Svelte) → WebSocket /ws → ws.rs handler → chat_dispatch.rs
  → run_chat() → Runner.run() → run_loop() → provider.stream()
  → ClientHub.broadcast() → WebSocket events back to frontend
```

Three WS endpoints: `/ws` (client), `/agent/ws` (agent), `/ws/extension` (Chrome bridge).

### Session Keys
Format: `agent:<id>:<channel>`, `subagent:<parent>:<child>`, `acp:<id>`, `<ch>:group:<id>`, etc. Sessions are decoupled from chats: `session.active_chat_id` points to the current conversation. `rotate_chat()` creates a new conversation under the same session (non-destructive reset). Legacy sessions backfill `active_chat_id = session.name` at runtime.

### Frontend Structure
- `app/src/routes/(app)/` — main app routes (chat, settings, marketplace, etc.)
- `app/src/lib/components/` — Svelte components
- `app/src/lib/stores/` — client-side state
- `app/src/lib/websocket/` — WebSocket client
- `app/src/lib/api/` — auto-generated API client
- `app/src/lib/i18n/` — internationalization

### Configuration
- `etc/nebo.yaml` — embedded config (server host/port, NeboAI URLs, auth placeholders)
- `~/.nebo/settings.json` — user auth, API keys (runtime)
- Env var overrides: `NEBOAI_API_URL`, `NEBOAI_JANUS_URL`, `NEBOAI_COMMS_URL`

## Code Rules

- **Styles only in `app.css`** — never inline styles or `<style>` blocks in Svelte files
- **Use `pnpm` only** — not npm or yarn
- **Minimal changes** — only modify code directly related to the task; preserve all existing functionality
- **Never assume code is unused** — it may be called from frontend, other services, or future features
- **Always build before pushing** — `make build` or `make build-desktop`
- **Idiomatic patterns** — one function with parameters, not multiple function variants for the same operation

## SME Documentation

Deep-dive reference docs live in `docs/sme/`. Key files:
- `CHAT_SYSTEM.md` — chat dispatch, runner, sessions, steering, lanes
- `TOOLS.md` — STRAP pattern, domain tools, tool corrections
- `MEMORY_AND_PROMPT.md` — memory extraction, prompt assembly, steering generators
- `SECURITY.md` — 23 tracked findings, security architecture
- `PLUGIN_SYSTEM.md` — plugin registry, verification, sandbox
- `APPS.md` — app platform, capabilities, lifecycle

## Release

Tag-triggered CI (`.github/workflows/release.yml`): push `v*` tag → builds all platforms → signs (macOS Developer ID + Windows Azure) → notarizes macOS → GitHub Release + CDN upload → updates Homebrew cask + APT repo.
