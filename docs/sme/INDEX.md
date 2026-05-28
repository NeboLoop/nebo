# Nebo Systems — Master Index

> **Last updated:** 2026-05-20
> **Purpose:** Exhaustive inventory of every system in Nebo, mapped to its SME documentation status.

---

## Summary

| Category | Systems | Documented | Missing SME |
|----------|---------|------------|-------------|
| Core Agent | 10 | 10 | 0 |
| Server & Chat | 8 | 8 | 0 |
| Tools | 6 | 6 | 0 |
| AI & Providers | 3 | 3 | 0 |
| Data & Storage | 4 | 4 | 0 |
| Auth & Security | 4 | 4 | 0 |
| Communication | 3 | 3 | 0 |
| Desktop & Platform | 6 | 6 | 0 |
| App Platform | 5 | 5 | 0 |
| Frontend | 8 | 8 | 0 |
| Infrastructure | 4 | 4 | 0 |
| **Totals** | **61** | **61** | **0** |

---

## 1. Core Agent Systems

| # | System | Crate/Module | SME Doc | Status |
|---|--------|-------------|---------|--------|
| 1 | Agent Runner (agentic loop) | `agent/runner.rs` | `AGENTS_SME.md` | Current |
| 2 | Memory Extraction & Prompt Assembly | `agent/memory.rs`, `prompt.rs`, `db_context.rs` | `MEMORY_AND_PROMPT.md` | Current |
| 3 | Memory Consolidation & Flush | `agent/memory_consolidation.rs`, `memory_flush.rs`, `memory_debounce.rs` | `MEMORY_AND_PROMPT.md` | Current |
| 4 | Steering Directives | `agent/steering/` | `AGENTS_SME.md` §14 | Current |
| 5 | Context Pruning & Compaction | `agent/pruning.rs`, `compaction.rs` | `MEMORY_AND_PROMPT.md` §15 | Current |
| 6 | Sub-Agent Orchestration | `agent/orchestrator.rs`, `decompose.rs`, `task_graph.rs` | `AGENT_ORCHESTRATION_SME.md` | Current |
| 7 | Tool Filter & Contextual Loading | `agent/tool_filter.rs` | `AGENTS_SME.md` §21 | Current |
| 8 | Followup Suggestions | `agent/followup.rs` | `AGENTS_SME.md` §23 | Current |
| 9 | Session & Transcript Management | `agent/session.rs`, `transcript.rs` | `CHAT_SYSTEM.md` | Current |
| 10 | Lane System (FIFO queues) | `agent/lanes.rs` | `CHAT_SYSTEM.md` | Current |

## 2. Server & Chat Systems

| # | System | Crate/Module | SME Doc | Status |
|---|--------|-------------|---------|--------|
| 11 | Chat Dispatch & Streaming | `server/chat_dispatch.rs` | `CHAT_SYSTEM.md` | Current |
| 12 | WebSocket Handlers | `server/handlers/ws.rs` | `CHAT_SYSTEM.md` | Current |
| 13 | Ghost Text (inline completion) | `server/handlers/ws.rs` | `CHAT_SYSTEM.md` §26 | Current |
| 14 | Redaction System | `server/redact.rs` | `CHAT_SYSTEM.md` §25 | Current |
| 15 | Run Registry & Progress | `server/run_registry.rs` | `CHAT_SYSTEM.md` | Current |
| 16 | Heartbeat & Proactive | `server/heartbeat.rs` | `AGENTS_SME.md` | Current |
| 17 | Middleware & CORS | `server/middleware.rs` | `MIDDLEWARE_SCHEDULER_SME.md` §1 | Current |
| 18 | Scheduler (background jobs) | `server/scheduler.rs` | `MIDDLEWARE_SCHEDULER_SME.md` §2 | Current |

## 3. Tool Systems

| # | System | Crate/Module | SME Doc | Status |
|---|--------|-------------|---------|--------|
| 19 | Tool Registry & Policy | `tools/registry.rs`, `policy.rs` | `TOOLS_SME.md` | Current |
| 20 | STRAP Domain Tools (10) | `tools/{system,web,bot,loop,...}_tool.rs` | `TOOLS_SME.md` | Current |
| 21 | Sidecar Tools | `tools/sidecar_tool.rs` | `SIDECAR_TOOLS_SME.md` | Current |
| 22 | Skill Loader & Runtime | `tools/skills/loader.rs` | `SKILLS_SME.md` | Current |
| 23 | Plugin Tool Execution | `tools/plugin_tool.rs` | `PLUGIN_SYSTEM.md` | Current |
| 24 | Tool Search (deferred loading) | `tools/tool_search.rs` | `TOOLS_SME.md` §4 | Current |

## 4. AI & Provider Systems

| # | System | Crate/Module | SME Doc | Status |
|---|--------|-------------|---------|--------|
| 25 | Provider Abstraction & Routing | `ai/providers/`, `ai/types.rs` | `PROVIDER_SYSTEM.md` | Current |
| 26 | Embedding System | `ai/embedding.rs` | `EMBEDDING_SYSTEM_SME.md` | Current |
| 27 | Model Catalog & Selection | `config/models.rs`, `agent/selector.rs` | `MODEL_CATALOG_SME.md` | Current |

## 5. Data & Storage Systems

| # | System | Crate/Module | SME Doc | Status |
|---|--------|-------------|---------|--------|
| 28 | SQLite Schema & Migrations | `db/migrations/` (92 files) | `DATABASE_LAYER_SME.md` | Current |
| 29 | Connection Pool & Store | `db/pool.rs`, `db/store.rs` | `DATABASE_LAYER_SME.md` | Current |
| 30 | Query Layer | `db/queries/{agents,memories,...}.rs` | `DATABASE_LAYER_SME.md` | Current |
| 31 | Configuration System | `config/` (YAML, env, settings.json) | `CONFIG_SYSTEM_SME.md` | Current |

## 6. Auth & Security Systems

| # | System | Crate/Module | SME Doc | Status |
|---|--------|-------------|---------|--------|
| 32 | Permissions & Capabilities | `server/handlers/`, policy | `PERMISSIONS_SME.md` | Current |
| 33 | Content Protection | `agent/sanitize.rs` | `CONTENT_PROTECTION.md` | Current |
| 34 | Auth System (JWT, keyring, encryption) | `auth/` (jwt, keyring, credential, service) | `AUTH_SYSTEM_SME.md` | Current |
| 35 | Secret Scanning | `agent/secret_scan.rs` | `SECRET_SCANNING_SME.md` | Current |

## 7. Communication Systems

| # | System | Crate/Module | SME Doc | Status |
|---|--------|-------------|---------|--------|
| 36 | NeboAI Connection (API, Comms, Janus) | `comm/neboai.rs`, `comm/api.rs` | `NEBOAI_CONNECTION.md` | Current |
| 37 | Comm Plugin Framework | `comm/manager.rs`, `comm/types.rs`, wire protocol | `COMM_FRAMEWORK_SME.md` | Current |
| 38 | Notification System | `notify/`, `server/handlers/notification.rs` | `NOTIFICATION_SYSTEM_SME.md` | Current |

## 8. Desktop & Platform Systems

| # | System | Crate/Module | SME Doc | Status |
|---|--------|-------------|---------|--------|
| 39 | Browser Automation (CDP) | `browser/` (14 modules) | `BROWSER_AUTOMATION.md` | Current |
| 40 | Voice Pipeline (TTS/STT) | `voice/` | `VOICE_PIPELINE_SME.md` | Current |
| 41 | Tauri Desktop App | `src-tauri/` (window, tray, hotkeys) | `TAURI_DESKTOP_SME.md` | Current |
| 42 | Desktop Tools | `tools/desktop_tool.rs`, `desktop_daemon.rs`, `desktop_snapshot.rs` | `DESKTOP_TOOLS_SME.md` | Current |
| 43 | Platform Tools | `tools/organizer.rs`, `tools/music_tool.rs`, `tools/spotlight_tool.rs`, `tools/keychain_tool.rs` | `PLATFORM_TOOLS_SME.md` | Current |
| 44 | VM Sandbox | `vm/`, `vm-daemon/`, `tools/vm_tool.rs` | `VM_SANDBOX_SME.md` | Current |

## 9. App Platform Systems

| # | System | Crate/Module | SME Doc | Status |
|---|--------|-------------|---------|--------|
| 45 | App Lifecycle & Sidecar Management | `server/app_lifecycle.rs` | `APPS.md` | Current |
| 46 | A2UI Protocol | `a2ui/` (3 sub-crates), `server/a2ui.rs` | `A2UI_PROTOCOL.md` | Current |
| 47 | A2UI Integration (frontend) | Removed — apps own their UI via `@neboai/app-sdk` | `A2UI_INTEGRATION.md` | Legacy |
| 48 | App SDK (`@neboai/app-sdk`) | Published on npm; source at `NeboAI/app-sdk` | `APPS.md` | Current |
| 49 | Napp Package Format | `napp/` (napp.rs, sealed.rs, reader.rs, signing.rs) | `NAPP_FORMAT_SME.md` | Current |

## 10. Frontend Systems

| # | System | Crate/Module | SME Doc | Status |
|---|--------|-------------|---------|--------|
| 50 | Chat Controller & UI | `app/src/lib/chat/`, `components/chat/` | `CHAT_SYSTEM.md` §29 | Current |
| 51 | Calendar & Schedule | `app/src/lib/stores/schedule.ts`, `components/Color*View.svelte` | `CALENDAR_SYSTEM.md` | Current |
| 52 | Marketplace UI | `app/src/routes/marketplace/`, `components/marketplace/` | `APPS.md` | Current |
| 53 | Slash Commands (frontend) | `components/chat/SlashCommandMenu.svelte` | `SLASH_COMMANDS.md` | Current |
| 54 | Workflow Builder UI | `components/workflow/` (5 components) | `WORKFLOW_BUILDER_UI_SME.md` | Current |
| 55 | Settings System (frontend) | `app/src/routes/settings/` (23 pages) | `FRONTEND_SETTINGS_SME.md` §1 | Current |
| 56 | Onboarding Flow | `app/src/routes/onboarding/` | `FRONTEND_SETTINGS_SME.md` §2 | Current |
| 57 | Internationalization | `app/src/lib/i18n/` (26 locales) | `I18N_SYSTEM_SME.md` | Current |

## 11. Infrastructure & DevOps

| # | System | Crate/Module | SME Doc | Status |
|---|--------|-------------|---------|--------|
| 58 | Release Pipeline & CI/CD | `.github/workflows/release.yml` | `RELEASE.md` | Current |
| 59 | Auto-Updater | `updater/` | `BUILD_TOOLING_SME.md` §1 | Current |
| 60 | API Code Generation | `scripts/genapi/` (Go) | `BUILD_TOOLING_SME.md` §2 | Current |
| 61 | Plugin Publishing Pipeline | `scripts/publish-plugins.sh` | `BUILD_TOOLING_SME.md` §3 | Current |

---

## Existing SME Docs (not primary system docs)

These docs exist but are reference/planning documents rather than system SME docs:

| Doc | Type | Notes |
|-----|------|-------|
| `NEBO_VISION.md` | Vision | Product philosophy & roadmap |
| `JANUS_GATEWAY_PRD.md` | PRD | External service (NeboAI Janus gateway) |
| `CODE_AUDITOR.md` | Feature | Code quality automation |
| `OPENCLAW_PLUGIN_PARITY.md` | Comparison | Plugin ecosystem competitive analysis |
| `ORGCHART_COMPONENT.md` | Component | Single UI component spec |
| `APP_UX_RESTRUCTURE.md` | Planning | UX refactor tracking |
| `STARTUP_PERFORMANCE.md` | Performance | Startup optimization reference |

---

## SME Document Inventory

All 17 new SME documents created:

| # | Document | Systems Covered | Lines |
|---|----------|----------------|-------|
| 1 | `AUTH_SYSTEM_SME.md` | JWT, keyring, AES-256-GCM, AuthService | ~817 |
| 2 | `CONFIG_SYSTEM_SME.md` | YAML, env vars, settings.json, models.yaml | ~500 |
| 3 | `NAPP_FORMAT_SME.md` | .napp envelope, signing, sealing, runtime | ~1803 |
| 4 | `DATABASE_LAYER_SME.md` | Pool, store, queries, migrations, schema | ~700 |
| 5 | `COMM_FRAMEWORK_SME.md` | Wire protocol, plugin manager, NeboAI | ~650 |
| 6 | `EMBEDDING_SYSTEM_SME.md` | Vector storage, hybrid search, providers | ~400 |
| 7 | `MODEL_CATALOG_SME.md` | Model selection, fuzzy matching, fallback | ~990 |
| 8 | `PLATFORM_TOOLS_SME.md` | Organizer, music, spotlight, keychain | ~1051 |
| 9 | `DESKTOP_TOOLS_SME.md` | Desktop tool, daemon, snapshot, input sim | ~1113 |
| 10 | `TAURI_DESKTOP_SME.md` | Window mgmt, tray, hotkeys, neboapp:// | ~650 |
| 11 | `NOTIFICATION_SYSTEM_SME.md` | OS notify, in-app, WebSocket push | ~400 |
| 12 | `MIDDLEWARE_SCHEDULER_SME.md` | Axum middleware, CORS, scheduler, RunRegistry | ~596 |
| 13 | `SECRET_SCANNING_SME.md` | Redaction, regex patterns, pipeline | ~430 |
| 14 | `FRONTEND_SETTINGS_SME.md` | 21 settings pages, onboarding flow | ~639 |
| 15 | `BUILD_TOOLING_SME.md` | Updater, genapi, plugin publish, CI/CD | ~560 |
| 16 | `I18N_SYSTEM_SME.md` | 25 locales, svelte-i18n, lazy loading | ~1020 |
| 17 | `WORKFLOW_BUILDER_UI_SME.md` | Visual editor, canvas, node types | ~908 |
| 18 | `VM_SANDBOX_SME.md` | VM isolation, wire protocol, bundle mgmt, CDN | ~920 |
| | **Total new documentation** | | **~13,147** |

---

## Crate Dependency Map

```
cli / src-tauri (entry points)
  -> server (Axum HTTP + WebSocket + handlers)
    -> agent (runner, session, memory, steering, compaction)
      -> ai (provider trait: Anthropic, OpenAI, Gemini, Ollama)
      -> tools (registry, policy, domain tools, skills loader)
      -> workflow (engine, triggers, activities)
    -> auth (JWT, keyring, AES-256-GCM encryption)
    -> db (SQLite, migrations, connection pool)
    -> config (YAML loading from etc/nebo.yaml)
    -> types (error enum, constants)
    -> comm (NeboAI, wire protocol, plugins)
    -> mcp (Model Context Protocol bridge)
    -> napp (package format, signing, runtime)
    -> browser (CDP automation)
    -> voice (TTS/STT pipeline)
    -> notify (OS notifications)
    -> updater (auto-update)
    -> proto (gRPC definitions)
    -> a2ui (accessibility-to-UI protocol)
    -> vm (VM sandbox — Virtualization.framework / QEMU)
      -> vm-daemon (guest daemon, cross-compiled musl)
```

## Statistics

| Metric | Count |
|--------|-------|
| Rust workspace crates | 23 (+ 3 a2ui sub-crates) |
| Agent modules | 37 |
| Server handler modules | 23 |
| Domain tools | 10 (STRAP) + meta-tools |
| Database migrations | 92 |
| Frontend routes | 51 |
| Frontend components | 59 |
| Frontend stores | 17 |
| i18n locales | 26 |
| Proto files | 9 |
| Total SME docs | 50 |
| Missing SME docs | 0 |
